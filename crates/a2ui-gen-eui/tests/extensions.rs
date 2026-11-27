//! The grammars of a catalog that declares a2ui-express's `at_exponential_express` extension: they take
//! the extension's forms, replayed from a2ui-express's `conformance/extensions/*.json`, and refuse what
//! its compiler refuses where a context-free grammar can tell.

mod gbnf;

use a2ui_express::compile::Compiler;
use a2ui_express::decompile::{DecompileOptions, Order, decompile};
use a2ui_express::schema::Catalog as Schema;
use a2ui_gen_eui::constraint::{Constraint, ConstraintFactory, accepts};
use a2ui_gen_eui::{Catalog, grammar};
use serde_json::Value;

const CATALOG: &str = include_str!("fixtures/extensions/catalog.json");
const FIXTURES: &[(&str, &str)] = &[
    ("hex", include_str!("fixtures/extensions/hex.json")),
    ("motion", include_str!("fixtures/extensions/motion.json")),
    ("patch", include_str!("fixtures/extensions/patch.json")),
    ("style", include_str!("fixtures/extensions/style.json")),
    ("theme", include_str!("fixtures/extensions/theme.json")),
];

fn document() -> Value {
    serde_json::from_str(CATALOG).unwrap()
}

fn compiler() -> Compiler {
    Compiler::new(vec![Schema::from_json(document()).unwrap()], None).unwrap()
}

fn lark(text: &str) -> Constraint {
    let env = toktrie::ApproximateTokEnv::single_byte_env();
    ConstraintFactory::new(&env).unwrap().compile(text).unwrap()
}

/// Both spellings of one grammar.
struct Grammars {
    lark: Constraint,
    gbnf: gbnf::Gbnf,
}

impl Grammars {
    fn screen() -> Grammars {
        Grammars {
            lark: lark(&grammar::lark(&document())),
            gbnf: gbnf::Gbnf::parse(&grammar::gbnf(&document())),
        }
    }

    fn patch() -> Grammars {
        Grammars {
            lark: lark(&grammar::lark_patch(&document()).unwrap()),
            gbnf: gbnf::Gbnf::parse(&grammar::gbnf_patch(&document()).unwrap()),
        }
    }

    fn accept(&self, text: &str) {
        if let Err(e) = accepts(&self.lark, text) {
            panic!("Lark refuses:\n{text}\n{e}");
        }
        assert!(self.gbnf.accepts(text), "GBNF refuses:\n{text}");
    }

    /// Refused by Lark, and by GBNF unless `gbnf_too` is false.
    fn refuse(&self, text: &str, gbnf_too: bool) {
        assert!(accepts(&self.lark, text).is_err(), "Lark takes:\n{text}");
        if gbnf_too {
            assert!(!self.gbnf.accepts(text), "GBNF takes:\n{text}");
        }
    }
}

fn cases(action: &str) -> Vec<Value> {
    FIXTURES
        .iter()
        .flat_map(|(_, f)| {
            let doc: Value = serde_json::from_str(f).unwrap();
            doc["cases"].as_array().unwrap().clone()
        })
        .filter(|c| c["action"] == action)
        .collect()
}

fn screen(body: &str) -> String {
    format!("<a2ui>\nsurface(\"main\")\n{body}\n</a2ui>")
}

/// Every compile case that compiles, in the spelling the grammar writes: compiled, then decompiled
/// top-down with keyword arguments (the extension's sugar, `theme(...)` and `style(...)` included). It is
/// one grammar reply, and it compiles to the case's messages.
#[test]
fn accepts_the_conformance_screens() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    let compiler = compiler();
    let options = DecompileOptions {
        keyword_args: true,
        order: Order::TopDown,
    };
    let mut skipped = Vec::new();
    let mut replayed = 0;
    for case in cases("compile") {
        let name = case["name"].as_str().unwrap();
        if case.get("expect_error").is_some() {
            continue;
        }
        let messages = compiler.compile(case["input"].as_str().unwrap(), "main").unwrap();
        assert_eq!(Value::Array(messages.clone()), case["expect"], "{name}");
        let express = decompile(compiler.catalogs(), &messages, &options).unwrap();
        // A reply builds one new surface `main`, `root` first: a case that changes a surface that
        // exists or sets other surface extensions is not one.
        let body = express
            .strip_prefix("surface(\"main\")\n")
            .filter(|b| b.contains("root = "));
        let Some(body) = body.filter(|b| !b.contains("surface(")) else {
            skipped.push(name.to_owned());
            continue;
        };
        let reply = screen(body);
        g.accept(&reply);
        assert_eq!(catalog.validate(&reply).unwrap(), messages, "{name}");
        replayed += 1;
    }
    assert_eq!(
        skipped,
        [
            "theme_on_an_existing_surface_calls_set_theme",
            "theme_alone_on_an_existing_surface",
            "theme_beside_other_surface_extensions",
        ]
    );
    assert_eq!(replayed, 27);
}

/// The `theme(...)` and `style(...)` lines of the cases as they are written there (aliases, quoted
/// colours, overrides), before a `root`.
#[test]
fn accepts_the_theme_and_style_lines_as_written() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    let mut lines = Vec::new();
    for case in cases("compile").into_iter().chain(cases("patch")) {
        if case.get("expect_error").is_some() {
            continue;
        }
        for key in ["input", "screen", "patch"] {
            for line in case[key].as_str().unwrap_or("").lines() {
                if (line.starts_with("theme(") || line.starts_with("style("))
                    && !lines.contains(&line.to_owned())
                {
                    lines.push(line.to_owned());
                }
            }
        }
    }
    assert!(lines.len() >= 12, "{lines:?}");
    for line in &lines {
        // a class is used once, so the screen means the same; the grammar names it `k1`
        let (line, class) = match line.strip_prefix("style(").and_then(|l| l.split_once(',')) {
            Some((_, words)) => (format!("style(k1,{words}"), ", s=k1"),
            None => (line.clone(), ""),
        };
        let reply = screen(&format!("{line}\nroot = Text(text=\"x\"{class})"));
        g.accept(&reply);
        catalog
            .validate(&reply)
            .unwrap_or_else(|e| panic!("{reply}\n{e}"));
    }
}

/// Positional arguments of the patch cases, by keyword, and their class `card` named `k1` (the
/// grammar's spelling).
fn keyword_args(patch: &str) -> String {
    canonical_class(patch)
        .replace("Text(\"", "Text(text=\"")
        .replace("Column([", "Column(children=[")
}

fn canonical_class(text: &str) -> String {
    text.replace("style(card,", "style(k1,").replace("s=card", "s=k1")
}

#[test]
fn accepts_the_conformance_patches() {
    let g = Grammars::patch();
    let compiler = compiler();
    let mut replayed = 0;
    for case in cases("patch") {
        let name = case["name"].as_str().unwrap();
        if case.get("expect_error").is_some() {
            continue;
        }
        let screen = canonical_class(case["screen"].as_str().unwrap());
        let screen = screen.as_str();
        let patch = keyword_args(case["patch"].as_str().unwrap());
        let reply = format!("<a2ui>\n{patch}\n</a2ui>");
        g.accept(&reply);
        let out = compiler.compile_patch(screen, &reply, "main").unwrap();
        assert_eq!(Value::Array(out.messages), case["expect"], "{name}");
        replayed += 1;
    }
    assert_eq!(replayed, 10);
    // A patch is not a screen, nor a screen a patch.
    g.refuse(
        "<a2ui>\nsurface(\"other\")\nroot = Text(text=\"x\")\n</a2ui>",
        true,
    );
    g.refuse("<a2ui>\n</a2ui>", true);
    g.refuse("<a2ui>\ndel(a, b)\n</a2ui>", true);
    g.refuse("<a2ui>\ndel(\"a\")\n</a2ui>", true);
    let screen = Grammars::screen();
    screen.refuse(
        "<a2ui>\nsurface(\"main\")\nroot = Text(text=\"x\")\ndel(root)\n</a2ui>",
        true,
    );
    screen.refuse(&self::screen("title = Text(text=\"x\")"), true);
}

#[test]
fn refuses_what_the_compiler_refuses() {
    let g = Grammars::screen();
    let catalog = Catalog::from_json(document()).unwrap();
    let ok = |body: &str| {
        let reply = screen(body);
        g.accept(&reply);
        catalog
            .validate(&reply)
            .unwrap_or_else(|e| panic!("{reply}\n{e}"));
    };
    ok("root = Text(text=\"x\", s=\"p8 hover:bg-accent mt-8 w50% p16px maxw-auto h-control.input op0.5\")");
    ok("root = Text(text=\"x\", s=\"justify-between justify-space-between fd-col d-none grow shrink0\")");
    ok("root = Column(children=[], stagger=60, delay=0.5, enter=\"fade-in\", exit=\"slide-out-up\")");
    ok("root = Motion(ref=\"spinner\")");
    ok(
        "root = Motion(kf=\"star 8 8 5 8 4 #ffffffaa | opacity 0:0 300:100 hold\", size=[16, 16], duration=300)",
    );
    ok("style(k1, \"p8\")\nstyle(k2, \"k1 r-md\")\nroot = Text(text=\"x\", s=\"k2 bg-card\")");
    ok(
        "theme(base=\"dark\", charts=[#111111, #222222], stroke=1.5, scale=1.25, light={ring: \"#ff00ff\"}, recipes={Heading: {weight: 700}})\nroot = Text(text=\"x\")",
    );

    // The compiler takes these too; the grammar keeps to the canonical spelling.
    for bad in [
        // classes are `k1`, `k2`, … in order, before `root`
        "style(card, \"p8\")\nroot = Text(text=\"x\", s=card)",
        "style(k2, \"p8\")\nroot = Text(text=\"x\", s=k2)",
        "style(k1, \"p8\")\nstyle(k1, \"p4\")\nroot = Text(text=\"x\", s=k1)",
        "style(k1, \"p8\")\nstyle(k3, \"p4\")\nroot = Text(text=\"x\", s=k1)",
        "root = Text(text=\"x\")\nstyle(k1, \"p8\")",
        "style(k1, \"p8\")\n$/a = 1\nroot = Text(text=$/a, s=k1)",
        "root = Text(text=\"x\")\ntheme(primary=#2563eb)", // a theme goes before root
        "root = Text(text=\"x\", s=\"p8  p4\")",
        "root = Text(text=\"x\", s=\"\")",
        "style(\"card\", \"p8\")\nroot = Text(text=\"x\", s=card)",
    ] {
        g.refuse(&screen(bad), true);
    }
    for bad in [
        // colours are literals inside theme(...) only (hex_outside_theme_is_refused)
        "root = Text(text=\"x\", style={color: #ff0000})",
        "root = Text(text=#ff0000)",
        // hex_seven_digits_is_a_comment
        "theme(primary=#1234567)\nroot = Text(text=\"x\")",
        // theme_unknown_seed, theme_bad_colour, theme_id_with_seeds
        "theme(primry=#ffffff)\nroot = Text(text=\"x\")",
        "theme(primary=\"red\")\nroot = Text(text=\"x\")",
        "theme(\"orange\", primary=#000000)\nroot = Text(text=\"x\")",
        "theme(base=\"dim\")\nroot = Text(text=\"x\")",
        "theme(density=\"tight\")\nroot = Text(text=\"x\")",
        "theme(radius=-1)\nroot = Text(text=\"x\")",
        "theme(dark={primry: #ffffff})\nroot = Text(text=\"x\")",
        "theme(charts=[#000000, #000000, #000000, #000000, #000000, #000000, #000000, #000000, #000000])\nroot = Text(text=\"x\")",
        // motion_unknown_enter, motion_unknown_exit, motion_negative_stagger, motion_unknown_ref
        "root = Text(text=\"x\", enter=\"zoom\")",
        "root = Text(text=\"x\", exit=\"fade-in\")",
        "root = Column(children=[], stagger=-1)",
        "root = Motion(ref=\"nope\")",
        "root = Text(text=\"x\", ref=\"pulse-dot\")",
        // motion_bad_keyframes and friends
        "root = Motion(kf=\"blob 1 2 3\")",
        "root = Motion(kf=\"circle 1 1 #000000\")",
        "root = Motion(kf=\"circle 1 1 1 #00000G\")",
        "root = Motion(kf=\"circle 1 1 1 #ABCDEF\")",
        "root = Motion(kf=\"circle 1 1 1 $color.primary | scale 0:100\")",
        "root = Motion(kf=\"circle 1 1 1 $color.primary | spin 0:0 10:360\")",
        "root = Motion(kf=\"circle 1 1 1 $color.primary | move 0:1 10:2\")",
        "root = Motion(kf=\"circle 1 1 1 $color.primary | scale -1:0 10:100\")",
        "root = Motion(kf=\"circle 1 1 1 $color.primary | scale 0:0 10:100 bounce\")",
        // style_unknown_word, style_bad_enum, and more the table rules out (a word that is an identifier,
        // `padding16`, may name a class: whether one is defined only the compiler knows)
        "root = Text(text=\"x\", s=\"padding-16\")",
        "root = Text(text=\"x\", s=\"justify-middle\")",
        "root = Text(text=\"x\", s=\"bg-16\")",
        "root = Text(text=\"x\", s=\"grow-x\")",
        "root = Text(text=\"x\", s=\"p16.5em\")",
        "root = Text(text=\"x\", s=\"hover:\")",
        "root = Text(text=\"x\", s=\"active:p8\")",
        "root = Text(text=\"x\", s=1)",
        "root = Text(text=\"x\", s=\"p8\", s2=\"p4\")",
        // style() takes a name and words; a class may not read as a word
        "style(col, \"p8\")\nroot = Text(text=\"x\", s=col)",
        "style(hover, \"p8\")\nroot = Text(text=\"x\")",
        "style(card)\nroot = Text(text=\"x\")",
        "style(card, \"p8 nope-1\")\nroot = Text(text=\"x\")",
        // del() is a patch's
        "del(a)\nroot = Text(text=\"x\")",
    ] {
        assert!(
            catalog.validate(&screen(bad)).is_err(),
            "the compiler takes {bad}"
        );
        g.refuse(&screen(bad), true);
    }
}

/// The values of theme seeds and keyframe eases the grammar lists, which a2ui-express keeps private:
/// the compiler takes each, and refuses a neighbour.
#[test]
fn the_listed_values_are_the_compilers() {
    let catalog = Catalog::from_json(document()).unwrap();
    let g = Grammars::screen();
    for (seed, values) in [
        ("base", &["light", "dark"][..]),
        ("density", &["compact", "default", "comfortable"]),
        ("shadow", &["none", "soft", "strong"]),
        ("button", &["rounded", "pill", "square"]),
    ] {
        for v in values.iter().chain(&["nope"]) {
            let reply = screen(&format!("theme({seed}=\"{v}\")\nroot = Text(text=\"x\")"));
            assert_eq!(catalog.validate(&reply).is_ok(), *v != "nope", "{reply}");
            assert_eq!(accepts(&g.lark, &reply).is_ok(), *v != "nope", "{reply}");
        }
    }
    for ease in [
        "linear",
        "ease",
        "ease-in",
        "ease-out",
        "ease-in-out",
        "hold",
        "nope",
    ] {
        let reply = screen(&format!(
            "root = Motion(kf=\"circle 1 1 1 $color.primary | scale 0:0 10:100 {ease}\")"
        ));
        assert_eq!(catalog.validate(&reply).is_ok(), ease != "nope", "{reply}");
        assert_eq!(accepts(&g.lark, &reply).is_ok(), ease != "nope", "{reply}");
    }
}

/// Every word the compiler's prompt teaches is one the grammar takes.
#[test]
fn takes_the_prompt_words() {
    let g = Grammars::screen();
    let words: Vec<String> = a2ui_express::ext::PROMPT_WORDS
        .split_whitespace()
        .flat_map(|w| {
            // `justify-start|center|end` lists several
            let (head, rest) = w.split_once('|').map_or((w, ""), |(h, r)| (h, r));
            let prefix = &head[..head.rfind('-').map_or(0, |i| i + 1)];
            std::iter::once(head.to_owned())
                .chain(
                    rest.split('|')
                        .filter(|r| !r.is_empty())
                        .map(move |r| format!("{prefix}{r}")),
                )
                .collect::<Vec<_>>()
        })
        .collect();
    let reply = screen(&format!("root = Text(text=\"x\", s=\"{}\")", words.join(" ")));
    g.accept(&reply);
    Catalog::from_json(document()).unwrap().validate(&reply).unwrap();
}

#[test]
fn without_the_extension_there_is_none_of_it() {
    let mut doc = document();
    doc["metadata"].as_object_mut().unwrap().remove("extensions");
    assert!(grammar::lark_patch(&doc).is_none());
    assert!(grammar::gbnf_patch(&doc).is_none());
    let text = grammar::gbnf(&doc);
    for rule in ["xtheme", "xstyle", "xdel", "XSWORDS", "XKF", "\"s=\""] {
        assert!(!text.contains(rule), "{rule}");
    }
    let g = lark(&grammar::lark(&doc));
    accepts(&g, &screen("root = Text(text=\"x\")")).unwrap();
    for bad in [
        "root = Text(text=\"x\", s=\"p8\")",
        "theme(primary=#2563eb)\nroot = Text(text=\"x\")",
        "root = Text(text=\"x\", enter=\"fade-in\")",
    ] {
        assert!(accepts(&g, &screen(bad)).is_err(), "{bad}");
    }
    // the built-in catalog does not declare it
    assert!(Catalog::eui().lark_patch().is_none());
    assert!(!Catalog::eui().lark().contains("XSWORDS"));
}

#[test]
fn true_turns_it_on_without_a_registry() {
    let mut doc = document();
    doc["metadata"]["extensions"]["at_exponential_express"] = Value::Bool(true);
    let g = lark(&grammar::lark(&doc));
    accepts(&g, &screen("root = Text(text=\"x\", s=\"p8\")")).unwrap();
    accepts(
        &g,
        &screen("root = Motion(kf=\"circle 1 1 1 #000000 | scale 0:0 9:100\")"),
    )
    .unwrap();
    assert!(accepts(&g, &screen("root = Motion(ref=\"spinner\")")).is_err());
}

/// The GBNF reader of these tests reads the whole eUI grammar, and agrees with Lark on its screens.
#[test]
fn the_gbnf_reader_reads_the_eui_grammar() {
    let eui = Catalog::eui();
    let g = gbnf::Gbnf::parse(&eui.gbnf());
    for s in [
        include_str!("fixtures/login.a2ui"),
        include_str!("fixtures/orders.a2ui"),
        include_str!("fixtures/empty.a2ui"),
    ] {
        assert!(g.accepts(s.trim()));
    }
    assert!(!g.accepts(&screen("root = Txt(text=\"a\")")));
}
