//! Writes a catalog's GBNF grammars, for llama.cpp, vLLM and other engines:
//!
//! ```sh
//! cargo run -p a2ui-gen-eui --no-default-features --example grammars -- catalog.json gen.gbnf [patch.gbnf]
//! ```
//!
//! The patch grammar needs the `at_exponential_express` extension.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [catalog, screen, rest @ ..] = args.as_slice() else {
        eprintln!("usage: grammars CATALOG.json GEN.gbnf [PATCH.gbnf]");
        std::process::exit(2);
    };
    let text = std::fs::read_to_string(catalog).unwrap_or_else(|e| panic!("{catalog}: {e}"));
    let catalog: serde_json::Value = serde_json::from_str(&text).expect("a JSON catalog");
    std::fs::write(screen, a2ui_gen_eui::grammar::gbnf(&catalog)).expect("written");
    if let Some(path) = rest.first() {
        let Some(patch) = a2ui_gen_eui::grammar::gbnf_patch(&catalog) else {
            eprintln!("the catalog does not declare at_exponential_express: no patch grammar");
            std::process::exit(1);
        };
        std::fs::write(path, patch).expect("written");
    }
}
