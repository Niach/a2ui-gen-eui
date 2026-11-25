use a2ui_gen_eui::stream::{Chunk, StatementSplitter};
use a2ui_gen_eui::{Catalog, Error, StatementKind};

const ORDERS: &str = include_str!("fixtures/orders.a2ui");

#[test]
fn statements_stream_in_any_chunk_size() {
    let want: Vec<String> = ORDERS
        .trim()
        .lines()
        .filter(|l| !l.contains("a2ui>"))
        .map(str::to_owned)
        .collect();
    for size in [1, 2, 7, 1000] {
        let mut s = StatementSplitter::default();
        let chars: Vec<char> = ORDERS.trim().chars().collect();
        let mut got = Vec::new();
        for c in chars.chunks(size) {
            got.extend(s.push(&c.iter().collect::<String>()));
        }
        got.extend(s.finish());
        assert_eq!(got, want);
    }
}

#[test]
fn a_line_break_in_a_string_does_not_end_a_statement() {
    let mut s = StatementSplitter::default();
    assert_eq!(
        s.push("<a2ui>\nroot = Text(text=\"\"\"a \"b\n"),
        Vec::<String>::new()
    );
    assert_eq!(s.push("c)\"\"\")\n"), ["root = Text(text=\"\"\"a \"b\nc)\"\"\")"]);
    assert_eq!(s.push("b = Text(text=r\"\\\")\n"), ["b = Text(text=r\"\\\")"]);
}

#[test]
fn kinds() {
    assert_eq!(StatementKind::of("surface(\"main\")"), StatementKind::Surface);
    assert_eq!(
        StatementKind::of("$/a/b = [1]"),
        StatementKind::Data { path: "/a/b".into() }
    );
    assert_eq!(
        StatementKind::of("root = Page(title=\"=\")"),
        StatementKind::Component { id: "root".into() }
    );
}

#[test]
fn messages_arrive_progressively_and_add_up() {
    let eui = Catalog::eui();
    let mut stream = eui.open_stream();
    let mut seen_statements = 0;
    let mut created_after = None;
    let mut batches = 0;
    for c in ORDERS.trim().chars() {
        for chunk in stream.push(&c.to_string()).unwrap() {
            match chunk {
                Chunk::Statement(_) => seen_statements += 1,
                Chunk::Messages(m) => {
                    batches += 1;
                    if m.iter().any(|m| m.get("createSurface").is_some()) {
                        created_after.get_or_insert(seen_statements);
                    }
                }
            }
        }
    }
    assert!(batches > 3, "{batches}");
    // The surface exists long before the last statement.
    assert!(created_after.unwrap() < seen_statements - 3);
}

#[test]
fn drops_unreachable_statements() {
    let eui = Catalog::eui();
    let reply = "<a2ui>\nsurface(\"main\")\nroot = Stack(children=[a])\na = Text(text=\"a\")\n\
                 lost = Stack(children=[deep])\ndeep = Text(text=\"\"\"two\nlines\"\"\")\n</a2ui>";
    assert!(matches!(eui.validate(reply), Err(Error::Invalid(_))));
    let v = eui.validate_pruned(reply).unwrap();
    let mut pruned = v.pruned.clone();
    pruned.sort();
    assert_eq!(pruned, ["deep", "lost"]);
    assert_eq!(
        v.express,
        "<a2ui>\nsurface(\"main\")\nroot = Stack(children=[a])\na = Text(text=\"a\")\n</a2ui>"
    );
    assert_eq!(eui.validate(&v.express).unwrap(), v.messages);
}

#[test]
fn rejects_what_pruning_cannot_fix() {
    let eui = Catalog::eui();
    for body in ["root = Stack(children=[missing])", "root = Text(text=\"cut"] {
        let reply = format!("<a2ui>\nsurface(\"main\")\n{body}\n</a2ui>");
        assert!(eui.validate_pruned(&reply).is_err(), "{body}");
    }
    assert!(eui.validate_pruned("no block").is_err());
    assert!(
        eui.validate_pruned("<a2ui>\nsurface(\"main\")\nroot = Text(text=\"a\")\n")
            .is_err()
    );
}

#[test]
fn messages_are_a2ui_1_0() {
    let login = include_str!("fixtures/login.a2ui");
    let messages = Catalog::eui().validate(login.trim()).unwrap();
    assert!(messages.iter().all(|m| m["version"] == "1.0"));
    let parts: Vec<_> = messages
        .iter()
        .flat_map(|m| ["createSurface", "updateComponents"].map(|k| &m[k]["components"]))
        .filter_map(|c| c.as_array())
        .flatten()
        .collect();
    let email = parts.iter().find(|c| c["id"] == "email").unwrap();
    assert_eq!(email["@checks"].as_array().unwrap().len(), 2);
    assert_eq!(email["value"]["@path"], "/form/email");
    for c in parts {
        for bare in ["checks", "catalogId", "metadata", "accessibility"] {
            assert!(c.get(bare).is_none(), "{c}");
        }
    }
}
