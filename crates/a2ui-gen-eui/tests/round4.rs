//! The grammars against what round 4's models wrote with them (Qwen3.5 4B, 2B, 0.8B on eUI with the
//! `at_exponential_express` extension), and against round 4's training targets
//! (`fixtures/round4/targets.jsonl`: synthetic screens and patches of every kind).

mod gbnf;

use a2ui_gen_eui::constraint::{Constraint, ConstraintFactory, accepts};
use a2ui_gen_eui::{Catalog, grammar};
use serde_json::{Value, json};

const TARGETS: &str = include_str!("fixtures/round4/targets.jsonl");

/// eUI's catalog, declaring the extension as round 4's did.
fn document() -> Value {
    let mut doc = Catalog::eui().document().clone();
    doc["metadata"]["extensions"]["at_exponential_express"] = json!({"version": 1});
    doc
}

struct Grammars {
    lark: Constraint,
    gbnf: gbnf::Gbnf,
}

impl Grammars {
    fn new(lark: &str, gbnf: &str) -> Grammars {
        let env = toktrie::ApproximateTokEnv::single_byte_env();
        Grammars {
            lark: ConstraintFactory::new(&env).unwrap().compile(lark).unwrap(),
            gbnf: gbnf::Gbnf::parse(gbnf),
        }
    }

    fn screen() -> Grammars {
        let doc = document();
        Grammars::new(&grammar::lark(&doc), &grammar::gbnf(&doc))
    }

    fn patch() -> Grammars {
        let doc = document();
        Grammars::new(
            &grammar::lark_patch(&doc).unwrap(),
            &grammar::gbnf_patch(&doc).unwrap(),
        )
    }

    fn accept(&self, text: &str) {
        if let Err(e) = accepts(&self.lark, text) {
            panic!("Lark refuses:\n{text}\n{e}");
        }
        assert!(self.gbnf.accepts(text), "GBNF refuses:\n{text}");
    }

    fn refuse(&self, text: &str) {
        assert!(accepts(&self.lark, text).is_err(), "Lark takes:\n{text}");
        assert!(!self.gbnf.accepts(text), "GBNF takes:\n{text}");
    }
}

fn screen(body: &str) -> String {
    format!("<a2ui>\nsurface(\"main\")\n{body}\n</a2ui>")
}

fn patch(body: &str) -> String {
    format!("<a2ui>\n{body}\n</a2ui>")
}

#[test]
fn accepts_the_round_4_targets() {
    let (screens, patches) = (Grammars::screen(), Grammars::patch());
    let catalog = Catalog::from_json(document()).unwrap();
    let mut kinds = Vec::new();
    for line in TARGETS.lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        let (kind, target) = (row["kind"].as_str().unwrap(), row["target"].as_str().unwrap());
        if kind.starts_with("patch") {
            patches.accept(&patch(target));
        } else {
            let reply = screen(target);
            screens.accept(&reply);
            catalog
                .validate(&reply)
                .unwrap_or_else(|e| panic!("{}: {e}", row["id"]));
        }
        if !kinds.iter().any(|k| k == kind) {
            kinds.push(kind.to_owned());
        }
    }
    kinds.sort();
    assert_eq!(kinds, ["generate", "patch", "patch-theme", "theme-variant"]);
}

/// 16 of 80 screens: a full-width `＝`, `（` or `，` where Express wants ASCII, which the compiler cannot
/// read. Unicode stays welcome inside strings.
#[test]
fn syntax_is_ascii() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    for bad in [
        "root = Stack(children=[a])\na＝Text(text=\"x\")",
        "root = Stack(children=[a])\na = Text（text=\"x\")",
        "root = Stack(children=[a，b])\na = Text(text=\"x\")\nb = Text(text=\"y\")",
        "root = Stack(children=[a])\na = Text(text=\"x\"，variant=\"caption\")",
        "root = Button(label=\"x\", on={press：Event(\"a\")})",
    ] {
        assert!(
            catalog.validate(&screen(bad)).is_err(),
            "the compiler takes {bad}"
        );
        g.refuse(&screen(bad));
    }
    // UAX 31 names the compiler takes; the grammar keeps to ASCII, where the runaways began
    for bad in [
        "root = Stack(children=[Text文本1])\nText文本1 = Text(text=\"x\")",
        "root = Stack(children=[Icon名称3])\nIcon名称3 = Icon(name=\"user\")",
    ] {
        g.refuse(&screen(bad));
    }
    let ok = screen("root = Text(text=\"Größe＝（1，2）文本 🎉\")");
    g.accept(&ok);
    catalog.validate(&ok).unwrap();
    Grammars::patch().refuse(&patch("a＝Text(text=\"x\")"));
}

/// 7 of 80: a style string with a word the table does not have, which the grammar took for a class
/// name. Class names are `k1`, `k2`, …, defined in order before `root`.
#[test]
fn style_strings_hold_the_tables_words() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    for word in [
        "minwidth32",
        "fg14",
        "textMuted",
        "hh56",
        "cardForeground",
        "lw1",
        "txcenter",
        "w5388w5388",
    ] {
        let bad = screen(&format!("root = Text(text=\"x\", s=\"p8 {word}\")"));
        assert!(catalog.validate(&bad).is_err(), "the compiler takes {word}");
        g.refuse(&bad);
    }
    for word in ["k0", "k65", "kk1", "K1"] {
        g.refuse(&screen(&format!(
            "style({word}, \"p8\")\nroot = Text(text=\"x\", s={word})"
        )));
    }
    let ok = screen(
        "style(k1, \"p8 bg-card\")\nstyle(k2, \"k1 r-md hover:bg-accent\")\nroot = Text(text=\"x\", s=\"k2 maxw320 fg-mutedForeground\")",
    );
    g.accept(&ok);
    catalog.validate(&ok).unwrap();
    // a patch's classes are the screen's: any `k<n>`, words as in a screen
    let p = Grammars::patch();
    p.accept(&patch("style(k7, \"p4\")\na = Text(text=\"x\", s=k7)"));
    p.refuse(&patch("a = Text(text=\"x\", s=\"minwidth32\")"));
    p.refuse(&patch("style(card, \"p4\")"));
}

/// 17 of 80 ran to the 4096-token limit: a name of one character repeated, the same class defined
/// again and again, components added without end. Every repetition is bounded now.
#[test]
fn repetition_is_bounded() {
    let g = Grammars::screen();
    let id = |n: usize| format!("a{}", "b".repeat(n - 1));
    g.accept(&screen(&format!(
        "root = Stack(children=[{0}])\n{0} = Text(text=\"x\")",
        id(64)
    )));
    g.refuse(&screen(&format!(
        "root = Stack(children=[{0}])\n{0} = Text(text=\"x\")",
        id(65)
    )));
    // the same class twice
    g.refuse(&screen(
        "style(k1, \"p8\")\nstyle(k2, \"p4\")\nstyle(k2, \"p4\")\nroot = Text(text=\"x\", s=k1)",
    ));
    // 64 classes, not 65
    let classes = |n: usize| {
        (1..=n)
            .map(|k| format!("style(k{k}, \"p{k}\")\n"))
            .collect::<String>()
    };
    g.accept(&screen(&format!("{}root = Text(text=\"x\")", classes(64))));
    g.refuse(&screen(&format!("{}root = Text(text=\"x\")", classes(65))));
    // 128 items in a list, not 129
    let items = |n: usize| vec!["1"; n].join(", ");
    g.accept(&screen(&format!("root = Sparkline(values=[{}])", items(128))));
    g.refuse(&screen(&format!("root = Sparkline(values=[{}])", items(129))));
    // 300 statements after root, not 301
    let stmts = |n: usize| {
        let ids: Vec<String> = (0..n).map(|i| format!("t{i}")).collect();
        let defs: String = ids.iter().map(|i| format!("\n{i} = Text(text=\"x\")")).collect();
        format!("root = Stack(children=[{}]){defs}", ids[..100].join(", "))
    };
    g.accept(&screen(&stmts(299)));
    g.refuse(&screen(&stmts(301)));
    // numbers
    g.accept(&screen(&format!("root = Progress(value={})", "9".repeat(20))));
    g.refuse(&screen(&format!("root = Progress(value={})", "9".repeat(21))));
    g.refuse(&screen(&format!(
        "root = Text(text=\"x\", s=\"w{}\")",
        "5".repeat(21)
    )));
}

/// A child is written inline where an id goes, as the targets do; the models, refused that, made up
/// ids from it (`IconnamebrandAcme`) and defined a new one for every child. A screen defines `root`
/// once.
#[test]
fn children_inline_and_root_once() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    for ok in [
        "root = Stack(children=[Icon(name=\"star\"), Text(text=\"x\", s=\"p4\")])",
        "root = Row(title=\"x\", slots={leading: Icon(name=\"star\")})",
        "root = Stack(children=[rootRow])\nrootRow = Text(text=\"x\")",
        "root = Stack(children=[roo, roots])\nroo = Text(text=\"x\")\nroots = Text(text=\"y\")",
    ] {
        g.accept(&screen(ok));
        catalog
            .validate(&screen(ok))
            .unwrap_or_else(|e| panic!("{ok}\n{e}"));
    }
    // one child for `children`, as some targets write it (the compiler takes it; eUI's validation
    // does not reach a child written so)
    g.accept(&screen(
        "root = Card(children=Stack(children=[Text(text=\"x\")]))",
    ));
    g.refuse(&screen("root = Text(text=\"x\")\nroot = Text(text=\"y\")"));
    // a patch may replace root
    Grammars::patch().accept(&patch("root = Text(text=\"y\")"));
}

/// A patch may break a statement over indented lines and part statements with a blank line.
#[test]
fn a_patch_may_span_lines() {
    let p = Grammars::patch();
    p.accept(&patch(
        "$/a = 1\n\nbox = Box(\n  style={\n    padding: 8,\n    gap:4\n  },\n  children=[\n    Text(text=\"x\"),\n    Button(\n      label=\"Go\",\n      on={press: Event(\n        \"go\",\n        {id: 1}\n      )}\n    )\n  ]\n)",
    ));
    p.refuse(&patch("box = Box(\n\n  children=[]\n)"));
    p.refuse(&patch("a = Text(text=\"x\")\n\n\nb = Text(text=\"y\")"));
    // a screen keeps to one line per statement
    Grammars::screen().refuse(&screen("root = Box(\n  children=[]\n)"));
}
