//! Streamable HTTP, the plain part of it: one JSON reply per POST to `/mcp`, no sessions, no server streams.

use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::Value;
use tiny_http::{Header, Method, Request, Response};

use crate::{Backend, Server, parse_error};

const MAX_BODY: u64 = 8 << 20;

/// A bound listener.
pub struct Listener(tiny_http::Server);

impl Listener {
    /// Binds; only loopback addresses, because there is no authentication.
    pub fn bind(addr: SocketAddr) -> Result<Self, String> {
        if !addr.ip().is_loopback() {
            return Err(format!("{addr} is not a loopback address"));
        }
        tiny_http::Server::http(addr)
            .map(Listener)
            .map_err(|e| e.to_string())
    }

    /// The bound port.
    pub fn port(&self) -> u16 {
        self.0.server_addr().to_ip().map_or(0, |a| a.port())
    }

    /// Serves forever.
    pub fn serve<B: Backend>(&self, server: Arc<Server<B>>) {
        for request in self.0.incoming_requests() {
            let server = server.clone();
            std::thread::spawn(move || answer(&server, request));
        }
    }
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    let h = request.headers().iter().find(|h| h.field.equiv(name))?;
    Some(h.value.as_str())
}

/// `host[:port]` (of an Origin or Host header) is this machine. Stops web pages, DNS rebinding included.
fn is_local(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn answer<B: Backend>(server: &Server<B>, mut request: Request) {
    let origin_ok =
        header(&request, "Origin").is_none_or(|o| o.split_once("://").is_some_and(|(_, a)| is_local(a)));
    let host_ok = header(&request, "Host").is_some_and(is_local);
    let status = |code: u16| Response::empty(code);
    if !origin_ok || !host_ok {
        let _ = request.respond(status(403));
        return;
    }
    if request.url() != "/mcp" {
        let _ = request.respond(status(404));
        return;
    }
    if request.method() != &Method::Post {
        let _ = request.respond(status(405));
        return;
    }
    let mut body = String::new();
    let read = request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body);
    if read.is_err() || body.len() as u64 > MAX_BODY {
        let _ = request.respond(status(413));
        return;
    }
    let reply = match serde_json::from_str::<Value>(&body) {
        Ok(message) => server.handle(&message, &|_| {}),
        Err(_) => Some(parse_error()),
    };
    let _ = match reply {
        None => request.respond(status(202)),
        Some(reply) => request
            .respond(Response::from_string(reply.to_string()).with_header(
                Header::from_bytes("Content-Type", "application/json").expect("a valid header"),
            )),
    };
}
