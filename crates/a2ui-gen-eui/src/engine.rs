//! llama.cpp inference. One worker thread owns the model; generations queue on it.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Instant;

use futures_core::Stream;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::session::{LlamaStateSeqFlags, SeqState};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::token::data::LlamaTokenData;
use llama_cpp_2::token::data_array::LlamaTokenDataArray;
use serde::Serialize;
use serde_json::Value;

use crate::constraint::{Constraint, ConstraintFactory};
use crate::sample::{Rng, draw};
use crate::stream::Chunk;
use crate::{Catalog, Error, Result, StatementKind, Validated, prompt};

const BATCH: usize = 512;
/// The same statement this often means the reply is stuck.
const MAX_REPEATS: u32 = 3;
/// Compiled grammars kept per generator.
const GRAMMARS: usize = 4;

/// How the model is loaded.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// CPU threads. Default: half the hardware threads, at most 8.
    pub threads: Option<usize>,
    /// Layers on the GPU. Default: all of them; a build without a GPU back end has none to use.
    pub gpu_layers: u32,
    /// Context size in tokens, prompt and reply together.
    pub context: u32,
    /// Fail unless the file has this sha256.
    pub sha256: Option<String>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            threads: None,
            gpu_layers: 999,
            context: 16384,
            sha256: None,
        }
    }
}

/// What restricts the sampled tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decoding {
    /// The catalog's grammar, with llguidance.
    #[default]
    Grammar,
    /// The catalog's grammar, with llama.cpp's own GBNF sampler. For comparison: it is much slower.
    Gbnf,
    /// Nothing.
    Free,
}

/// How a reply is sampled. The defaults are the prompt contract's.
#[derive(Debug, Clone)]
pub struct Options {
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: usize,
    /// More attempts after an invalid reply.
    pub retries: u32,
    pub retry_temperature: f32,
    /// Default: random.
    pub seed: Option<u64>,
    pub decoding: Decoding,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            temperature: 0.2,
            top_p: 0.8,
            max_tokens: 4096,
            retries: 2,
            retry_temperature: 0.7,
            seed: None,
            decoding: Decoding::Grammar,
        }
    }
}

/// Times of one generation, in milliseconds.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub prompt_tokens: usize,
    /// Prompt tokens restored from the cached system prompt.
    pub cached_tokens: usize,
    /// Reading the prompt.
    pub prompt_ms: f64,
    /// From the start to the first component, `root`: from here on the surface renders. Data lines may come
    /// before it.
    pub first_block_ms: Option<f64>,
    /// Sampled tokens, all attempts.
    pub tokens: usize,
    /// Sampling and decoding, all attempts.
    pub decode_ms: f64,
    /// Of which grammar masks and sampling.
    pub sample_ms: f64,
    pub total_ms: f64,
}

impl Stats {
    /// Decoding speed.
    pub fn tokens_per_second(&self) -> f64 {
        self.tokens as f64 * 1000.0 / self.decode_ms.max(1e-9)
    }
}

/// A finished, valid screen.
#[derive(Debug, Clone)]
pub struct Ui {
    /// The `<a2ui>…</a2ui>` block.
    pub express: String,
    /// The A2UI messages it compiles to.
    pub messages: Vec<Value>,
    /// Ids of dropped statements nothing reached from `root`.
    pub pruned: Vec<String>,
    /// Attempts it took.
    pub attempts: u32,
    pub stats: Stats,
}

/// An item of a [`Generation`].
#[derive(Debug, Clone)]
pub enum ExpressBlock {
    /// A statement is complete. The first component is always `root`; data lines may come before it.
    Statement { kind: StatementKind, source: String },
    /// A2UI messages that are ready for the renderer; a surface renders progressively from these alone.
    /// The first ones come with `root` and hold the data read before it.
    Messages(Vec<Value>),
    /// The attempt was invalid: discard what came so far (delete the surface); a new attempt follows.
    Restart { attempt: u32, reason: String },
    /// The valid screen. The last item.
    Complete(Ui),
}

/// A running generation: an async [`Stream`], or iterate [`Self::blocking`]. Dropping it cancels it.
pub struct Generation {
    rx: Pin<Box<async_channel::Receiver<Result<ExpressBlock>>>>,
}

impl Generation {
    /// Iterates on this thread.
    pub fn blocking(self) -> impl Iterator<Item = Result<ExpressBlock>> {
        std::iter::from_fn(move || self.rx.recv_blocking().ok())
    }

    /// Waits for the result.
    pub fn wait(self) -> Result<Ui> {
        for block in self.blocking() {
            if let ExpressBlock::Complete(ui) = block? {
                return Ok(ui);
            }
        }
        Err(Error::Cancelled)
    }
}

impl Stream for Generation {
    type Item = Result<ExpressBlock>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.as_mut().poll_next(cx)
    }
}

type Tx = async_channel::Sender<Result<ExpressBlock>>;

struct Job {
    catalog: Catalog,
    user: String,
    options: Options,
    tx: Tx,
}

/// A loaded model. Generations run one at a time, in order; dropping it cancels those still running.
pub struct Generator {
    /// `None` only while it is dropped.
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    description: String,
}

impl Generator {
    /// Loads a GGUF file with the default options. No network access.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with(path, LoadOptions::default())
    }

    /// Loads a GGUF file.
    pub fn load_with(path: impl AsRef<Path>, options: LoadOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(expected) = &options.sha256 {
            crate::model::verify(&path, expected)?;
        }
        let (jobs, queue) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("a2ui-gen-eui".into())
                .spawn(move || worker(path, options, queue, ready_tx, stop))?
        };
        let description = ready_rx
            .recv()
            .map_err(|_| Error::Model("the worker died while loading".into()))??;
        Ok(Generator {
            jobs: Some(jobs),
            worker: Some(worker),
            stop,
            description,
        })
    }

    /// The model's name and size.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Builds a new screen. `data` is what it must show; `catalog` defaults to eui's.
    pub fn generate(
        &self,
        request: &str,
        data: Option<&Value>,
        catalog: Option<&Catalog>,
    ) -> Result<Generation> {
        self.generate_with(request, data, catalog, Options::default())
    }

    /// [`Self::generate`] with sampling options.
    pub fn generate_with(
        &self,
        request: &str,
        data: Option<&Value>,
        catalog: Option<&Catalog>,
        options: Options,
    ) -> Result<Generation> {
        self.start(prompt::generate_text(request, data), catalog, options)
    }

    /// Changes a screen; the reply is the whole new screen.
    pub fn refine(&self, current: &str, change: &str, catalog: Option<&Catalog>) -> Result<Generation> {
        self.refine_with(current, change, catalog, Options::default())
    }

    /// [`Self::refine`] with sampling options.
    pub fn refine_with(
        &self,
        current: &str,
        change: &str,
        catalog: Option<&Catalog>,
        options: Options,
    ) -> Result<Generation> {
        self.start(prompt::refine_text(current, change), catalog, options)
    }

    fn start(&self, user: String, catalog: Option<&Catalog>, options: Options) -> Result<Generation> {
        let (tx, rx) = async_channel::unbounded();
        self.jobs
            .as_ref()
            .expect("set until drop")
            .send(Job {
                catalog: catalog.cloned().unwrap_or_else(Catalog::eui),
                // Like the chat template.
                user: user.trim().to_owned(),
                options,
                tx,
            })
            .map_err(|_| Error::Inference("the worker is gone".into()))?;
        Ok(Generation { rx: Box::pin(rx) })
    }
}

/// Waits for the worker: a process that exits while llama.cpp computes would crash.
impl Drop for Generator {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            let mut b = LlamaBackend::init().map_err(|e| e.to_string())?;
            if std::env::var_os("A2UI_GEN_EUI_LOG").is_none() {
                b.void_logs();
            }
            Ok(b)
        })
        .as_ref()
        .map_err(|e| Error::Model(e.clone()))
}

fn worker(
    path: PathBuf,
    options: LoadOptions,
    queue: mpsc::Receiver<Job>,
    ready: mpsc::Sender<Result<String>>,
    stop: Arc<AtomicBool>,
) {
    let model = match load_model(&path, &options) {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut session = match Session::new(&model, &options, stop) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let name = model.meta_val_str("general.name").unwrap_or_default();
    let _ = ready.send(Ok(format!(
        "{name} ({:.2} B parameters, {} MB)",
        model.n_params() as f64 / 1e9,
        model.size() >> 20
    )));
    for job in queue {
        if job.tx.is_closed() || session.stop.load(Ordering::Relaxed) {
            continue;
        }
        let tx = job.tx.clone();
        let result = session.run(job);
        let _ = tx.send_blocking(result.map(ExpressBlock::Complete));
    }
}

fn load_model(path: &Path, options: &LoadOptions) -> Result<LlamaModel> {
    let params = LlamaModelParams::default().with_n_gpu_layers(options.gpu_layers);
    LlamaModel::load_from_file(backend()?, path, &params).map_err(|e| {
        Error::Model(format!(
            "{}: {e} (A2UI_GEN_EUI_LOG=1 shows llama.cpp's log)",
            path.display()
        ))
    })
}

struct Grammar {
    key: usize,
    constraint: Constraint,
    /// Keeps the catalog, and with it the key, alive.
    _catalog: Catalog,
}

struct Session<'m> {
    model: &'m LlamaModel,
    ctx: LlamaContext<'m>,
    n_ctx: usize,
    constraints: ConstraintFactory,
    grammars: Vec<Grammar>,
    /// State after the last system prompt, so the next request with the same catalog starts there.
    system: Option<(Vec<LlamaToken>, SeqState)>,
    stop: Arc<AtomicBool>,
}

impl<'m> Session<'m> {
    fn new(model: &'m LlamaModel, options: &LoadOptions, stop: Arc<AtomicBool>) -> Result<Self> {
        let threads = options.threads.unwrap_or_else(|| {
            let n = std::thread::available_parallelism().map_or(4, |n| n.get());
            (n / 2).clamp(1, 8)
        }) as i32;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(options.context))
            .with_n_batch(BATCH as u32)
            .with_n_seq_max(1)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let ctx = model
            .new_context(backend()?, params)
            .map_err(|e| Error::Model(e.to_string()))?;
        let tok_env = LlamaSampler::llguidance_tok_env(model);
        Ok(Session {
            model,
            n_ctx: ctx.n_ctx() as usize,
            ctx,
            constraints: ConstraintFactory::new(&tok_env)?,
            grammars: Vec::new(),
            system: None,
            stop,
        })
    }

    fn control(&self, text: &str) -> Vec<LlamaToken> {
        self.model.vocab().tokenize(text.as_bytes(), false, true)
    }

    /// Text of the caller's: control tokens written in it stay plain text.
    fn plain(&self, text: &str) -> Vec<LlamaToken> {
        self.model.vocab().tokenize(text.as_bytes(), false, false)
    }

    fn decode(&mut self, tokens: &[LlamaToken], pos: &mut usize, tx: &Tx) -> Result<()> {
        let mut batch = LlamaBatch::new(BATCH, 1);
        for chunk in tokens.chunks(BATCH) {
            if tx.is_closed() || self.stop.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            batch.clear();
            for (i, t) in chunk.iter().enumerate() {
                batch
                    .add(*t, (*pos + i) as i32, &[0], i + 1 == chunk.len())
                    .map_err(|e| Error::Inference(e.to_string()))?;
            }
            self.ctx
                .decode(&mut batch)
                .map_err(|e| Error::Inference(e.to_string()))?;
            *pos += chunk.len();
        }
        Ok(())
    }

    fn save(&self) -> Result<SeqState> {
        self.ctx
            .state_seq_get(0, LlamaStateSeqFlags::empty())
            .map_err(|e| Error::Inference(e.to_string()))
    }

    fn restore(&mut self, state: &SeqState) -> Result<()> {
        self.ctx.clear_kv_cache();
        self.ctx
            .state_seq_set(state, 0)
            .map_err(|e| Error::Inference(e.to_string()))
    }

    /// Reads all but the last prompt token and returns the state to start attempts from.
    fn read_prompt(
        &mut self,
        system: Vec<LlamaToken>,
        rest: &[LlamaToken],
        stats: &mut Stats,
        tx: &Tx,
    ) -> Result<SeqState> {
        let mut pos = 0;
        if let Some((cached, state)) = self.system.take_if(|(cached, _)| *cached == system) {
            self.restore(&state)?;
            pos = cached.len();
            stats.cached_tokens = pos;
            self.system = Some((cached, state));
        } else {
            self.ctx.clear_kv_cache();
            self.decode(&system, &mut pos, tx)?;
            let state = self.save()?;
            self.system = Some((system, state));
        }
        self.decode(rest, &mut pos, tx)?;
        self.save()
    }

    fn matcher(&mut self, catalog: &Catalog) -> Result<llguidance::Matcher> {
        let key = catalog.key();
        if let Some(i) = self.grammars.iter().position(|g| g.key == key) {
            let used = self.grammars.remove(i);
            self.grammars.push(used);
        } else {
            let constraint = self.constraints.compile(catalog.lark())?;
            if self.grammars.len() == GRAMMARS {
                self.grammars.remove(0);
            }
            self.grammars.push(Grammar {
                key,
                constraint,
                _catalog: catalog.clone(),
            });
        }
        Ok(self.grammars.last().expect("just pushed").constraint.matcher())
    }

    fn run(&mut self, job: Job) -> Result<Ui> {
        let started = Instant::now();
        let Job {
            catalog,
            user,
            options,
            tx,
        } = job;
        let mut stats = Stats::default();

        let mut system = self.control(prompt::chat_head());
        system.extend(self.plain(catalog.system_prompt().trim()));
        system.extend(self.control(prompt::chat_middle()));
        let mut rest = self.plain(&user);
        rest.extend(self.control(prompt::chat_tail()));
        stats.prompt_tokens = system.len() + rest.len();
        if stats.prompt_tokens + options.max_tokens > self.n_ctx {
            return Err(Error::ContextFull {
                prompt: stats.prompt_tokens,
                reply: options.max_tokens,
                context: self.n_ctx,
            });
        }
        let last = rest.pop().expect("the template has tokens");
        let state = self.read_prompt(system, &rest, &mut stats, &tx)?;
        stats.prompt_ms = started.elapsed().as_secs_f64() * 1e3;

        let seed = options.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64)
        });
        let mut attempt = 0;
        loop {
            if attempt > 0 {
                self.restore(&state)?;
            }
            let temperature = match attempt {
                0 => options.temperature,
                _ => options.retry_temperature,
            };
            let mut rng = Rng::new(seed.wrapping_add(u64::from(attempt)));
            let mut reply = String::new();
            let outcome = self.attempt(
                &catalog,
                &options,
                temperature,
                &mut rng,
                (stats.prompt_tokens - 1, last),
                &mut reply,
                &mut stats,
                started,
                &tx,
            );
            attempt += 1;
            match outcome {
                Ok(v) => {
                    stats.total_ms = started.elapsed().as_secs_f64() * 1e3;
                    return Ok(Ui {
                        express: v.express,
                        messages: v.messages,
                        pruned: v.pruned,
                        attempts: attempt,
                        stats,
                    });
                }
                Err(Error::Invalid(reason)) if attempt <= options.retries => {
                    let _ = tx.send_blocking(Ok(ExpressBlock::Restart {
                        attempt: attempt + 1,
                        reason,
                    }));
                }
                Err(Error::Invalid(message)) => {
                    return Err(Error::NoValidReply {
                        attempts: attempt,
                        message,
                        reply,
                        stats: Box::new(stats),
                    });
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &mut self,
        catalog: &Catalog,
        options: &Options,
        temperature: f32,
        rng: &mut Rng,
        (mut pos, last): (usize, LlamaToken),
        reply: &mut String,
        stats: &mut Stats,
        started: Instant,
        tx: &Tx,
    ) -> Result<Validated> {
        let mut matcher = match options.decoding {
            Decoding::Grammar => Some(self.matcher(catalog)?),
            _ => None,
        };
        let mut gbnf = match options.decoding {
            Decoding::Gbnf => Some(
                LlamaSampler::grammar(self.model, &catalog.gbnf(), "root")
                    .map_err(|e| Error::Grammar(e.to_string()))?,
            ),
            _ => None,
        };
        let mut reading = catalog.open_stream();
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut emit = |reading: &mut crate::stream::ReplyStream,
                        text: &str,
                        stats: &mut Stats|
         -> Result<()> {
            for chunk in reading.push(text)? {
                let block = match chunk {
                    Chunk::Messages(m) => ExpressBlock::Messages(m),
                    Chunk::Statement(source) => {
                        // A model caught in a loop would run to the token limit.
                        let times = seen.entry(source.clone()).or_default();
                        *times += 1;
                        if *times == MAX_REPEATS {
                            return Err(Error::Invalid(format!("the reply repeats itself: {source}")));
                        }
                        let kind = StatementKind::of(&source);
                        if matches!(kind, StatementKind::Component { .. }) && stats.first_block_ms.is_none() {
                            stats.first_block_ms = Some(started.elapsed().as_secs_f64() * 1e3);
                        }
                        ExpressBlock::Statement { kind, source }
                    }
                };
                if tx.send_blocking(Ok(block)).is_err() {
                    return Err(Error::Cancelled);
                }
            }
            Ok(())
        };

        // The grammar leaves no choice in how a reply starts, so it is read along with the prompt.
        let mut pending = vec![last];
        if let Some(m) = &mut matcher {
            let start = self.plain(prompt::REPLY_START);
            let ids: Vec<u32> = start.iter().map(|t| t.0 as u32).collect();
            m.consume_tokens(&ids)
                .map_err(|e| Error::Grammar(e.to_string()))?;
            pending.extend(start);
            reply.push_str(prompt::REPLY_START);
            emit(&mut reading, prompt::REPLY_START, stats)?;
        }
        let begun = Instant::now();
        let mut bytes: Vec<u8> = Vec::new();
        let mut done = false;
        for _ in 0..options.max_tokens {
            self.decode(&pending, &mut pos, tx)?;
            let sampling = Instant::now();
            let logits = self.ctx.get_logits_ith(-1);
            let pick = match (&mut matcher, &mut gbnf) {
                (Some(m), _) => {
                    let mask = m
                        .compute_mask_or_eos()
                        .map_err(|e| Error::Grammar(e.to_string()))?;
                    let allowed = || mask.iter().map(|id| (id, logits[id as usize]));
                    draw(allowed, temperature, options.top_p, rng)
                }
                (None, Some(g)) => {
                    let all = (0i32..)
                        .zip(logits)
                        .map(|(id, l)| LlamaTokenData::new(LlamaToken(id), *l, 0.0));
                    let mut all = LlamaTokenDataArray::from_iter(all, false);
                    g.apply(&mut all);
                    let allowed = || all.data.iter().map(|d| (d.id().0 as u32, d.logit()));
                    draw(allowed, temperature, options.top_p, rng)
                }
                (None, None) => draw(
                    || (0u32..).zip(logits.iter().copied()),
                    temperature,
                    options.top_p,
                    rng,
                ),
            };
            stats.sample_ms += sampling.elapsed().as_secs_f64() * 1e3;
            let token =
                LlamaToken(pick.ok_or_else(|| Error::Inference("no token is allowed".into()))? as i32);
            stats.tokens += 1;
            if self.model.vocab().is_eog(token) {
                done = true;
                break;
            }
            if let Some(m) = &mut matcher {
                m.consume_token(token.0 as u32)
                    .map_err(|e| Error::Grammar(e.to_string()))?;
            }
            if let Some(g) = &mut gbnf {
                g.accept(token);
            }
            bytes.extend(self.model.vocab().token_to_piece(token, false, None));
            let valid = match std::str::from_utf8(&bytes) {
                Ok(_) => bytes.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => return Err(Error::Invalid("the reply is not UTF-8".into())),
            };
            if valid > 0 {
                let text: String = String::from_utf8(bytes.drain(..valid).collect()).expect("checked");
                reply.push_str(&text);
                let pushed = emit(&mut reading, &text, stats);
                if pushed.is_err() {
                    stats.decode_ms += begun.elapsed().as_secs_f64() * 1e3;
                }
                pushed?;
            }
            if reply.trim_end().ends_with(prompt::REPLY_END)
                || matcher.as_ref().is_some_and(|m| m.is_stopped())
            {
                done = true;
                break;
            }
            pending = vec![token];
        }
        stats.decode_ms += begun.elapsed().as_secs_f64() * 1e3;
        if !done {
            return Err(Error::Invalid(format!(
                "the reply is longer than {} tokens",
                options.max_tokens
            )));
        }
        catalog.validate_pruned(reply)
    }
}
