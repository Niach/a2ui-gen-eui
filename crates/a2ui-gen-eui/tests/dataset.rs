//! Every target of a training set, through the grammars of its catalog. The data stays where it is:
//! set `A2UI_DATASET` to a directory of `*.jsonl` rows (`kind`, `target`) and `A2UI_DATASET_CATALOG` to
//! the catalog, else this test does nothing. `generate` and `theme-variant` targets are screens, `patch*`
//! targets patches. `A2UI_DATASET_EXCEPT` names rows (ids, comma-separated) the grammar refuses on
//! purpose: targets the catalog's schema refuses, which a grammar of its values cannot take.

mod gbnf;

use a2ui_gen_eui::constraint::{Constraint, ConstraintFactory, accepts};
use a2ui_gen_eui::grammar;
use serde_json::Value;

#[test]
fn accepts_every_dataset_target() {
    let (Ok(dir), Ok(catalog)) = (
        std::env::var("A2UI_DATASET"),
        std::env::var("A2UI_DATASET_CATALOG"),
    ) else {
        eprintln!("A2UI_DATASET / A2UI_DATASET_CATALOG unset: skipped");
        return;
    };
    let except: Vec<String> = std::env::var("A2UI_DATASET_EXCEPT")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let catalog: Value = serde_json::from_str(&std::fs::read_to_string(catalog).unwrap()).unwrap();
    let env = toktrie::ApproximateTokEnv::single_byte_env();
    let lark = |text: &str| -> Constraint { ConstraintFactory::new(&env).unwrap().compile(text).unwrap() };
    let screen = (
        gbnf::Gbnf::parse(&grammar::gbnf(&catalog)),
        lark(&grammar::lark(&catalog)),
    );
    let patch = (
        gbnf::Gbnf::parse(&grammar::gbnf_patch(&catalog).unwrap()),
        lark(&grammar::lark_patch(&catalog).unwrap()),
    );
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    let (mut total, mut refused) = (0, Vec::new());
    for file in files {
        for line in std::fs::read_to_string(&file).unwrap().lines() {
            let row: Value = serde_json::from_str(line).unwrap();
            let (kind, target) = (row["kind"].as_str().unwrap(), row["target"].as_str().unwrap());
            let (g, reply) = if kind.starts_with("patch") {
                (&patch, format!("<a2ui>\n{target}\n</a2ui>"))
            } else {
                (&screen, format!("<a2ui>\nsurface(\"main\")\n{target}\n</a2ui>"))
            };
            total += 1;
            let id = row["id"].as_str().unwrap_or("?");
            if except.iter().any(|e| e == id) {
                assert!(g.0.check(&reply).is_err(), "GBNF takes {id}");
                assert!(accepts(&g.1, &reply).is_err(), "Lark takes {id}");
                continue;
            }
            if let Err(e) = accepts(&g.1, &reply) {
                refused.push(format!("{id} ({kind}) Lark: {e}"));
            }
            if let Err(at) = g.0.check(&reply) {
                let chars: Vec<char> = reply.chars().collect();
                let from = at.saturating_sub(60);
                let to = (at + 20).min(chars.len());
                let near: String = chars[from..to].iter().collect();
                refused.push(format!("{id} ({kind}) GBNF at {at}: {near:?}"));
            }
        }
    }
    for r in refused.iter().take(400) {
        eprintln!("{r}");
    }
    assert!(refused.is_empty(), "{} of {total} refused", refused.len());
    eprintln!("{} of {total} targets accepted", total - except.len());
}
