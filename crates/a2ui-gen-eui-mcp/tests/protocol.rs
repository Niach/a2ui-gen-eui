use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};

use a2ui_gen_eui_mcp::{Backend, Call, Outcome, Server};
use serde_json::{Value, json};

struct Fake;

impl Backend for Fake {
    fn run(&self, call: Call, progress: &mut dyn FnMut(&str) -> bool) -> Result<Outcome, String> {
        let express = match call {
            Call::Generate {
                request,
                data,
                catalog,
            } => {
                assert!(catalog.is_none());
                if request == "fail" {
                    return Err("no valid reply".into());
                }
                format!("{request}|{}", data.unwrap_or_default())
            }
            Call::Refine { current, change, .. } => format!("{current}|{change}"),
        };
        progress("root = Text()");
        Ok(Outcome {
            express,
            attempts: 1,
            pruned: vec![],
        })
    }
}

#[derive(Clone, Default)]
struct Out(Arc<Mutex<Vec<u8>>>);
impl Write for Out {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn talk(messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let out = Out::default();
    Arc::new(Server::new(Fake))
        .serve(Cursor::new(input), out.clone())
        .unwrap();
    let bytes = out.0.lock().unwrap().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn handshake_and_tools() {
    let got = talk(&[
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}),
    ]);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0]["result"]["protocolVersion"], "2025-03-26");
    let names: Vec<&str> = got[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["generate_ui", "refine_ui"]);
    assert_eq!(got[2]["error"]["code"], -32601);
}

fn call(args: Value, name: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": "c", "method": "tools/call",
        "params": {"name": name, "arguments": args, "_meta": {"progressToken": 7}}})
}

#[test]
fn calls() {
    let got = talk(&[call(json!({"request": "r", "data": {"a": 1}}), "generate_ui")]);
    assert_eq!(got[0]["method"], "notifications/progress");
    assert_eq!(got[0]["params"]["progressToken"], 7);
    assert_eq!(got[1]["result"]["structuredContent"]["express"], "r|{\"a\":1}");
    let got = talk(&[call(json!({"current": "ui", "change": "c"}), "refine_ui")]);
    assert_eq!(got[1]["result"]["content"][0]["text"], "ui|c");
    let got = talk(&[call(json!({"request": "fail"}), "generate_ui")]);
    assert_eq!(got[0]["result"]["isError"], true);
    let got = talk(&[call(json!({}), "generate_ui")]);
    assert_eq!(got[0]["error"]["code"], -32602);
}

#[cfg(feature = "http")]
#[test]
fn http() {
    use a2ui_gen_eui_mcp::http::Listener;
    use std::io::Read;

    assert!(Listener::bind("0.0.0.0:0".parse().unwrap()).is_err());
    let listener = Listener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.port();
    std::thread::spawn(move || listener.serve(Arc::new(Server::new(Fake))));
    let post = |extra: &str, host: &str| {
        let body = call(json!({"request": "r"}), "generate_ui").to_string();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "POST /mcp HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        reply
    };
    assert!(post("", &format!("127.0.0.1:{port}")).contains("\"express\":\"r|null\""));
    assert!(post("Origin: http://localhost:3000\r\n", "localhost").starts_with("HTTP/1.1 200"));
    assert!(post("Origin: https://evil.example\r\n", "localhost").starts_with("HTTP/1.1 403"));
    assert!(post("", "evil.example").starts_with("HTTP/1.1 403"));
}
