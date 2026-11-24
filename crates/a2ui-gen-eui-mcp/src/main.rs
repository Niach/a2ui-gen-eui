use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use a2ui_gen_eui::{ExpressBlock, Generator, LoadOptions};
use a2ui_gen_eui_mcp::{Backend, Call, Outcome, Server};

const USAGE: &str = "\
MCP server that generates A2UI Express user interfaces with a local model.

  a2ui-gen-eui-mcp --model FILE.gguf [options]     serve on stdio
    --sha256 HEX         refuse another file
    --threads N          CPU threads
    --context N          context size in tokens (16384)
    --gpu-layers N       layers on the GPU (all)
    --http 127.0.0.1:P   serve streamable HTTP on /mcp instead of stdio

  a2ui-gen-eui-mcp download --dir DIR [--url URL --sha256 HEX]
    downloads a model and checks its hash (build with --features download);
    without --url: the base Qwen3.5 0.8B, for trying the runtime out
";

/// Loads the model once: in the background at startup, so the first call does not wait for all of it.
struct Local {
    path: PathBuf,
    options: LoadOptions,
    generator: OnceLock<Result<Generator, String>>,
}

impl Local {
    fn generator(&self) -> Result<&Generator, String> {
        self.generator
            .get_or_init(|| Generator::load_with(&self.path, self.options.clone()).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(Clone::clone)
    }
}

struct Shared(Arc<Local>);

impl Backend for Shared {
    fn run(&self, call: Call, progress: &mut dyn FnMut(&str) -> bool) -> Result<Outcome, String> {
        let generator = self.0.generator()?;
        let generation = match &call {
            Call::Generate {
                request,
                data,
                catalog,
            } => generator.generate(request, data.as_ref(), catalog.as_ref()),
            Call::Refine {
                current,
                change,
                catalog,
            } => generator.refine(current, change, catalog.as_ref()),
        }
        .map_err(|e| e.to_string())?;
        for block in generation.blocking() {
            match block.map_err(|e| e.to_string())? {
                ExpressBlock::Statement { source, .. } if !progress(&source) => {
                    return Err("cancelled".into());
                }
                ExpressBlock::Complete(ui) => {
                    return Ok(Outcome {
                        express: ui.express,
                        attempts: ui.attempts,
                        pruned: ui.pruned,
                    });
                }
                _ => {}
            }
        }
        Err("the generation ended early".into())
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("a2ui-gen-eui-mcp: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1).peekable();
    let download = args.next_if(|a| a == "download").is_some();
    let mut flags = std::collections::HashMap::new();
    while let Some(flag) = args.next() {
        if matches!(flag.as_str(), "--help" | "-h") {
            print!("{USAGE}");
            return Ok(());
        }
        let known: &[&str] = match download {
            true => &["--dir", "--url", "--sha256"],
            false => &[
                "--model",
                "--sha256",
                "--threads",
                "--context",
                "--gpu-layers",
                "--http",
            ],
        };
        if !known.contains(&flag.as_str()) {
            return Err(format!("unknown argument {flag}\n\n{USAGE}"));
        }
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        flags.insert(flag, value);
    }
    if download {
        return fetch(&flags);
    }
    let number = |flag: &str| -> Result<Option<u32>, String> {
        flags
            .get(flag)
            .map(|v| v.parse().map_err(|_| format!("{flag}: not a number: {v}")))
            .transpose()
    };
    let path = PathBuf::from(flags.get("--model").ok_or(USAGE)?);
    if !path.is_file() {
        return Err(format!("no such file: {}", path.display()));
    }
    let mut options = LoadOptions {
        sha256: flags.get("--sha256").cloned(),
        threads: number("--threads")?.map(|n| n as usize),
        ..Default::default()
    };
    options.context = number("--context")?.unwrap_or(options.context);
    options.gpu_layers = number("--gpu-layers")?.unwrap_or(options.gpu_layers);

    let local = Arc::new(Local {
        path,
        options,
        generator: OnceLock::new(),
    });
    let preload = local.clone();
    std::thread::spawn(move || match preload.generator() {
        Ok(g) => eprintln!("a2ui-gen-eui-mcp: loaded {}", g.description()),
        Err(e) => eprintln!("a2ui-gen-eui-mcp: {e}"),
    });
    let server = Arc::new(Server::new(Shared(local)));
    if let Some(addr) = flags.get("--http") {
        #[cfg(feature = "http")]
        {
            let addr = addr
                .parse()
                .map_err(|_| format!("--http: not an address: {addr}"))?;
            let listener = a2ui_gen_eui_mcp::http::Listener::bind(addr)?;
            eprintln!("a2ui-gen-eui-mcp: http://127.0.0.1:{}/mcp", listener.port());
            listener.serve(server);
            return Ok(());
        }
        #[cfg(not(feature = "http"))]
        return Err(format!("--http {addr}: built without the feature `http`"));
    }
    server
        .serve(std::io::stdin().lock(), std::io::stdout())
        .map_err(|e| e.to_string())
}

#[cfg(feature = "download")]
fn fetch(flags: &std::collections::HashMap<String, String>) -> Result<(), String> {
    use a2ui_gen_eui::model::{ModelSpec, download};

    let dir = flags.get("--dir").ok_or("--dir is required")?;
    let spec = match (flags.get("--url"), flags.get("--sha256")) {
        (Some(url), Some(sha256)) => ModelSpec::new(url, sha256),
        (None, None) => ModelSpec::base_0_8b(),
        _ => return Err("--url and --sha256 go together".into()),
    };
    let mut shown = 0;
    let path = download(&spec, dir, |done, total| {
        if done >> 26 != shown {
            shown = done >> 26;
            eprintln!("{} / {} MB", done >> 20, total.unwrap_or(0) >> 20);
        }
    })
    .map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(())
}

#[cfg(not(feature = "download"))]
fn fetch(_: &std::collections::HashMap<String, String>) -> Result<(), String> {
    Err("built without the feature `download`".into())
}
