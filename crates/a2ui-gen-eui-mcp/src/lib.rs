//! The MCP protocol part, independent of the model behind it.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use a2ui_gen_eui::Catalog;
use serde_json::{Value, json};

#[cfg(feature = "http")]
pub mod http;

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// A tool call.
pub enum Call {
    Generate {
        request: String,
        data: Option<Value>,
        catalog: Option<Catalog>,
    },
    Refine {
        current: String,
        change: String,
        catalog: Option<Catalog>,
    },
}

/// A valid screen.
pub struct Outcome {
    pub express: String,
    pub attempts: u32,
    pub pruned: Vec<String>,
}

/// What answers tool calls.
pub trait Backend: Send + Sync + 'static {
    /// Runs a call. `progress` is told each statement as it completes, and returns false to cancel.
    fn run(&self, call: Call, progress: &mut dyn FnMut(&str) -> bool) -> Result<Outcome, String>;
}

/// Sends a notification to the client.
pub type Notify<'a> = &'a (dyn Fn(Value) + Sync);

/// Handles JSON-RPC messages.
pub struct Server<B> {
    backend: B,
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B) -> Self {
        Server {
            backend,
            running: Mutex::default(),
        }
    }

    /// Handles one message; `None` when it needs no reply.
    pub fn handle(&self, message: &Value, notify: Notify) -> Option<Value> {
        let method = message.get("method").and_then(Value::as_str);
        let params = message.get("params").unwrap_or(&Value::Null);
        let Some(id) = message.get("id").filter(|id| !id.is_null()) else {
            if method == Some("notifications/cancelled") {
                let running = self.running.lock().unwrap();
                if let Some(flag) = params
                    .get("requestId")
                    .and_then(|id| running.get(&id.to_string()))
                {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            return None;
        };
        let result = match method? {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => self.call(id, params, notify),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        })
    }

    fn call(&self, id: &Value, params: &Value, notify: Notify) -> Result<Value, (i32, String)> {
        let call = parse_call(params).map_err(|e| (-32602, e))?;
        let token = params.pointer("/_meta/progressToken").cloned();
        let cancelled = Arc::new(AtomicBool::new(false));
        let key = id.to_string();
        self.running
            .lock()
            .unwrap()
            .insert(key.clone(), cancelled.clone());
        let mut n = 0u64;
        let outcome = self.backend.run(call, &mut |statement| {
            if let Some(token) = &token {
                n += 1;
                notify(
                    json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {
                    "progressToken": token, "progress": n, "message": statement}}),
                );
            }
            !cancelled.load(Ordering::Relaxed)
        });
        self.running.lock().unwrap().remove(&key);
        Ok(match outcome {
            Ok(o) => json!({
                "content": [{"type": "text", "text": o.express}],
                "structuredContent": {"express": o.express, "attempts": o.attempts, "pruned": o.pruned},
            }),
            Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
        })
    }

    /// Serves newline-delimited JSON-RPC until `input` ends. Calls run on their own threads.
    pub fn serve(
        self: &Arc<Self>,
        input: impl BufRead,
        output: impl Write + Send + 'static,
    ) -> std::io::Result<()> {
        let output = Arc::new(Mutex::new(output));
        let send = move |v: &Value| {
            let mut out = output.lock().unwrap();
            let _ = writeln!(out, "{v}").and_then(|()| out.flush());
        };
        let mut calls = Vec::new();
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                send(&parse_error());
                continue;
            };
            if message.get("method").and_then(Value::as_str) == Some("tools/call") {
                let (server, send) = (self.clone(), send.clone());
                calls.retain(|c: &std::thread::JoinHandle<()>| !c.is_finished());
                calls.push(std::thread::spawn(move || {
                    if let Some(reply) = server.handle(&message, &|v| send(&v)) {
                        send(&reply);
                    }
                }));
            } else if let Some(reply) = self.handle(&message, &|_| {}) {
                send(&reply);
            }
        }
        for c in calls {
            let _ = c.join();
        }
        Ok(())
    }
}

pub(crate) fn parse_error() -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = VERSIONS
        .iter()
        .find(|v| Some(**v) == asked)
        .unwrap_or(&VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "a2ui-gen-eui", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "generate_ui builds a screen as A2UI Express from a description; refine_ui changes one. The result is a validated <a2ui> block.",
    })
}

fn tools() -> Value {
    let catalog = json!({"type": "object", "description": "An A2UI 1.0 catalog schema (`protocolVersion` 1.0) to use instead of the eui core catalog. The model knows eui's catalog best."});
    let output = json!({"type": "object", "properties": {
        "express": {"type": "string", "description": "The validated <a2ui>…</a2ui> block"},
        "attempts": {"type": "integer"},
        "pruned": {"type": "array", "items": {"type": "string"}}
    }, "required": ["express", "attempts", "pruned"]});
    let annotations = json!({"readOnlyHint": true, "openWorldHint": false});
    json!([
        {
            "name": "generate_ui",
            "title": "Generate a UI",
            "description": "Builds a user interface from a description with a local model. Returns validated A2UI Express.",
            "inputSchema": {"type": "object", "properties": {
                "request": {"type": "string", "description": "What the screen is for and what it shows"},
                "data": {"description": "JSON data the screen shows; it is put in the data model and bound"},
                "catalog": catalog
            }, "required": ["request"]},
            "outputSchema": output, "annotations": annotations
        },
        {
            "name": "refine_ui",
            "title": "Change a UI",
            "description": "Changes a user interface made by generate_ui. Returns the whole new screen as validated A2UI Express.",
            "inputSchema": {"type": "object", "properties": {
                "current": {"type": "string", "description": "The current <a2ui>…</a2ui> block"},
                "change": {"type": "string", "description": "What to change"},
                "catalog": catalog
            }, "required": ["current", "change"]},
            "outputSchema": output, "annotations": annotations
        }
    ])
}

fn parse_call(params: &Value) -> Result<Call, String> {
    let args = params.get("arguments").unwrap_or(&Value::Null);
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("`{key}` is required"))
    };
    let catalog = || match args.get("catalog") {
        None | Some(Value::Null) => Ok(None),
        Some(c) => Catalog::from_json(c.clone()).map(Some).map_err(|e| e.to_string()),
    };
    match params.get("name").and_then(Value::as_str) {
        Some("generate_ui") => Ok(Call::Generate {
            request: text("request")?,
            data: args.get("data").filter(|d| !d.is_null()).cloned(),
            catalog: catalog()?,
        }),
        Some("refine_ui") => Ok(Call::Refine {
            current: text("current")?,
            change: text("change")?,
            catalog: catalog()?,
        }),
        other => Err(format!("unknown tool: {}", other.unwrap_or("(none)"))),
    }
}
