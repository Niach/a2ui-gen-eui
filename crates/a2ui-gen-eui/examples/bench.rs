//! cargo run --release --example bench -- MODEL.gguf [THREADS] [MAX_TOKENS] [grammar,gbnf,free] [Component,…]
//!
//! With a list of components the catalog is cut down to them and goes into the prompt, for a model that does
//! not know eui's.
//!
//! Tokens/s, time to the first block and validity, with and without grammar, on five requests.

use a2ui_gen_eui::{Catalog, Decoding, Error, ExpressBlock, Generator, LoadOptions, Options};

const REQUESTS: [&str; 5] = [
    "A login screen with email, password, a forgot-password link and a sign-in button.",
    "A dashboard for an online shop: revenue, orders and returns as tiles, and the latest five orders below.",
    "Notification settings: switches for email, push and weekly summary, and a save button.",
    "An empty project list with a button to create the first project.",
    "A compact card that confirms a restaurant booking for four people on Friday at 19:30.",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .ok_or("usage: bench MODEL.gguf [THREADS] [MAX_TOKENS] [grammar,gbnf,free]")?;
    let threads: usize = args.next().map_or(Ok(8), |a| a.parse())?;
    let max_tokens: usize = args.next().map_or(Ok(512), |a| a.parse())?;
    let only = args.next().unwrap_or_else(|| "grammar,gbnf,free".into());
    let catalog = match args.next() {
        None => Catalog::eui(),
        Some(names) => {
            let names: Vec<&str> = names.split(',').collect();
            let mut doc = Catalog::eui().document().clone();
            doc["components"]
                .as_object_mut()
                .expect("a map")
                .retain(|k, _| names.contains(&k.as_str()));
            Catalog::from_json(doc)?
        }
    };
    let load = std::time::Instant::now();
    let generator = Generator::load_with(
        model,
        LoadOptions {
            threads: Some(threads),
            ..Default::default()
        },
    )?;
    println!(
        "{}, {threads} threads, loaded in {:?}",
        generator.description(),
        load.elapsed()
    );
    println!(
        "decoding | tokens/s | sampling ms/token | prompt ms | first block ms (median) | valid / complete / requests"
    );
    for decoding in [Decoding::Grammar, Decoding::Gbnf, Decoding::Free] {
        if !only.contains(&format!("{decoding:?}").to_lowercase()) {
            continue;
        }
        let (mut first, mut valid, mut complete) = (Vec::new(), 0, 0);
        let (mut tokens, mut decode, mut sample, mut prompt) = (0, 0.0, 0.0, Vec::new());
        // The first request warms up: grammar compilation, system prompt cache.
        for (i, request) in REQUESTS.iter().take(1).chain(&REQUESTS).enumerate() {
            let options = Options {
                decoding,
                max_tokens,
                retries: 0,
                seed: Some(1),
                ..Default::default()
            };
            let started = std::time::Instant::now();
            let mut first_at = None;
            for block in generator
                .generate_with(request, None, Some(&catalog), options)?
                .blocking()
            {
                match block {
                    Ok(ExpressBlock::Statement { kind, .. })
                        if matches!(kind, a2ui_gen_eui::StatementKind::Component { .. }) =>
                    {
                        first_at.get_or_insert(started.elapsed().as_secs_f64() * 1e3);
                    }
                    Ok(ExpressBlock::Complete(ui)) if i > 0 => {
                        valid += 1;
                        complete += 1;
                        tokens += ui.stats.tokens;
                        decode += ui.stats.decode_ms;
                        sample += ui.stats.sample_ms;
                        prompt.push(ui.stats.prompt_ms);
                    }
                    Err(Error::NoValidReply {
                        message,
                        reply,
                        stats,
                        ..
                    }) if i > 0 => {
                        complete += usize::from(reply.ends_with("</a2ui>"));
                        tokens += stats.tokens;
                        decode += stats.decode_ms;
                        sample += stats.sample_ms;
                        prompt.push(stats.prompt_ms);
                        let head: String = reply.chars().take(200).collect();
                        eprintln!("{decoding:?} {i}: {message}\n{head}");
                    }
                    Err(Error::NoValidReply { .. }) => {}
                    Err(e) => return Err(e.into()),
                    _ => {}
                }
            }
            if i > 0 {
                first.extend(first_at);
            }
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
        };
        println!(
            "{decoding:?} | {:.1} | {:.2} | {:.0} | {:.0} (of {}) | {valid} / {complete} / {}",
            f64::from(tokens as u32) * 1000.0 / decode,
            sample / f64::from(tokens as u32),
            median(&mut prompt),
            median(&mut first),
            first.len(),
            REQUESTS.len(),
        );
    }
    Ok(())
}
