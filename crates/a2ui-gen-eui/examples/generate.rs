//! cargo run --release --example generate -- MODEL.gguf "A login screen" [data.json]

use a2ui_gen_eui::{ExpressBlock, Generator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .ok_or("usage: generate MODEL.gguf REQUEST [DATA.json]")?;
    let request = args.next().ok_or("no request")?;
    let data = match args.next() {
        Some(path) => Some(serde_json::from_str(&std::fs::read_to_string(path)?)?),
        None => None,
    };
    let generator = Generator::load(model)?;
    eprintln!("{}", generator.description());
    for block in generator.generate(&request, data.as_ref(), None)?.blocking() {
        match block? {
            ExpressBlock::Statement { source, .. } => println!("{source}"),
            ExpressBlock::Messages(_) => {}
            ExpressBlock::Restart { attempt, reason } => eprintln!("attempt {attempt}: {reason}"),
            ExpressBlock::Complete(ui) => eprintln!(
                "valid after {} attempt(s), {:.1} tokens/s, pruned {:?}, {:?}",
                ui.attempts,
                ui.stats.tokens_per_second(),
                ui.pruned,
                ui.stats
            ),
        }
    }
    Ok(())
}
