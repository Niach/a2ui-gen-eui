use a2ui_gen_eui::constraint::{Constraint, ConstraintFactory, accepts};
use a2ui_gen_eui::{Catalog, grammar};
use serde_json::json;

fn constraint(catalog: &Catalog) -> Constraint {
    let env = toktrie::ApproximateTokEnv::single_byte_env();
    ConstraintFactory::new(&env)
        .unwrap()
        .compile(catalog.lark())
        .unwrap()
}

const SCREENS: &[&str] = &[
    include_str!("fixtures/login.a2ui"),
    include_str!("fixtures/orders.a2ui"),
    include_str!("fixtures/empty.a2ui"),
];

#[test]
fn accepts_canonical_screens_that_compile() {
    let eui = Catalog::eui();
    let g = constraint(&eui);
    for s in SCREENS {
        eui.validate(s.trim()).unwrap();
        accepts(&g, s.trim()).unwrap();
    }
}

#[test]
fn refuses_what_the_catalog_does_not_define() {
    let g = constraint(&Catalog::eui());
    let reply = |body: &str| format!("<a2ui>\nsurface(\"main\")\n{body}\n</a2ui>");
    accepts(&g, &reply("root = Text(text=\"a\")")).unwrap();
    for bad in [
        "root = Txt(text=\"a\")",                    // unknown component
        "root = Text(txt=\"a\")",                    // unknown property
        "root = Text(text=1)",                       // wrong type
        "root = Text(text=\"a\", variant=\"huge\")", // not in the enum
        "root = Text(text=nope(1))",                 // unknown function
        "a = Text(text=\"a\")",                      // root is not first
        "root = Text(text=\"a)",                     // open string
        "root = Text(text=$/a-b)",                   // not a path
        "root = Text(text=\"a\"); b = Text(text=\"b\")",
    ] {
        assert!(accepts(&g, &reply(bad)).is_err(), "{bad}");
    }
    assert!(accepts(&g, "<a2ui>\nsurface(\"other\")\nroot = Text(text=\"a\")\n</a2ui>").is_err());
    assert!(accepts(&g, "Sure! <a2ui>").is_err());
}

#[test]
fn strings_in_all_four_forms() {
    let g = constraint(&Catalog::eui());
    for s in [
        r#""""#,
        r#""a \"b\" \\ ü 🎉""#,
        "\"\"\"two\nlines \"quoted\" x\"\"\"",
        r#"r"^[0-9]{5}$""#,
        "r\"\"\"a\nb\"\"\"",
    ] {
        accepts(
            &g,
            &format!("<a2ui>\nsurface(\"main\")\nroot = Text(text={s})\n</a2ui>"),
        )
        .unwrap();
    }
}

#[test]
fn follows_a_custom_catalog() {
    let doc = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://example.com/catalogs/tiny.json",
        "catalogId": "https://example.com/catalogs/tiny.json",
        "components": {
            "Headline": {
                "type": "object",
                "allOf": [{"$ref": "common_types.json#/$defs/ComponentCommon"}],
                "properties": {
                    "component": {"const": "Headline"},
                    "words": {"$ref": "common_types.json#/$defs/DynamicString"},
                    "size": {"type": "string", "enum": ["big", "small"]}
                },
                "required": ["component", "words"]
            }
        },
        "functions": {}
    });
    let catalog = Catalog::from_json(doc).unwrap();
    assert!(catalog.system_prompt().contains("Headline"));
    assert!(
        catalog
            .system_prompt()
            .starts_with("You build user interfaces as A2UI Express. Reply with")
    );
    let g = constraint(&catalog);
    let ok = "<a2ui>\nsurface(\"main\")\nroot = Headline(words=\"hi\", size=\"big\")\n</a2ui>";
    accepts(&g, ok).unwrap();
    catalog.validate(ok).unwrap();
    assert!(accepts(&g, &ok.replace("Headline", "Text")).is_err());
    assert!(catalog.validate(&ok.replace("words=\"hi\", ", "")).is_err());
}

#[test]
fn gbnf_has_the_same_rules() {
    let catalog = Catalog::eui();
    let gbnf = grammar::gbnf(catalog.document());
    assert!(gbnf.starts_with("root ::= \"<a2ui>\\nsurface(\\\"main\\\")\\n\""));
    let components = catalog.document()["components"].as_object().unwrap();
    for name in components.keys() {
        assert!(gbnf.contains(&format!("\"{name}(\"")), "{name}");
    }
    // Every rule that is referred to exists.
    let names: std::collections::HashSet<&str> = gbnf
        .lines()
        .filter_map(|l| Some(l.split_once(" ::= ")?.0))
        .collect();
    for word in ["kv", "kw", "enum", "map", "list"] {
        assert!(names.iter().any(|n| n.starts_with(word)), "{word}");
    }
}
