//! Reading a reply while it is written.

use a2ui_express::response::{Part, ResponseStream};
use serde_json::Value;

use crate::{Catalog, Error, Result};

/// What a statement is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementKind {
    /// `surface("main")`
    Surface,
    /// `$/path = value`
    Data { path: String },
    /// `id = Component(…)`
    Component { id: String },
    /// Anything else (the grammar allows none).
    Other,
}

impl StatementKind {
    /// Reads the kind off a statement's source.
    pub fn of(source: &str) -> Self {
        let s = source.trim_start();
        if let Some(rest) = s.strip_prefix('$') {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '/'))
                .unwrap_or(rest.len());
            return StatementKind::Data {
                path: rest[..end].to_owned(),
            };
        }
        if s.starts_with("surface(") {
            return StatementKind::Surface;
        }
        match s.split_once('=') {
            Some((id, _)) if !id.contains(['(', '"']) => StatementKind::Component {
                id: id.trim().to_owned(),
            },
            _ => StatementKind::Other,
        }
    }
}

/// Splits Express into statements, one per line; a line break inside a string or bracket does not end one.
#[derive(Debug, Default)]
pub struct StatementSplitter {
    pending: String,
    scanned: usize,
}

impl StatementSplitter {
    /// Adds text and returns the statements it completed. The `<a2ui>` tags are dropped.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.pending.push_str(text);
        let mut out = Vec::new();
        while let Some(i) = self.pending[self.scanned..].find('\n') {
            let end = self.scanned + i;
            if is_complete(&self.pending[..end]) {
                let line: String = self.pending.drain(..=end).collect();
                self.scanned = 0;
                Self::keep(&line, &mut out);
            } else {
                self.scanned = end + 1;
            }
        }
        out
    }

    /// The rest, at the end of the text.
    pub fn finish(mut self) -> Vec<String> {
        let mut out = Vec::new();
        Self::keep(&std::mem::take(&mut self.pending), &mut out);
        out
    }

    fn keep(line: &str, out: &mut Vec<String>) {
        let s = line.trim();
        let s = s.strip_prefix("<a2ui>").unwrap_or(s);
        let s = s.strip_suffix("</a2ui>").unwrap_or(s).trim();
        if !s.is_empty() {
            out.push(s.to_owned());
        }
    }
}

/// Whether every string and bracket in `s` is closed.
fn is_complete(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' => {
                let raw = i > 0
                    && matches!(b[i - 1], b'r' | b'R')
                    && !(i > 1 && (b[i - 2].is_ascii_alphanumeric() || b[i - 2] == b'_'));
                let quote: &[u8] = if b[i..].starts_with(b"\"\"\"") {
                    b"\"\"\""
                } else {
                    b"\""
                };
                i += quote.len();
                loop {
                    if i >= b.len() {
                        return false;
                    }
                    if !raw && b[i] == b'\\' {
                        i += 2;
                    } else if b[i..].starts_with(quote) {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += quote.len() - 1;
            }
            _ => {}
        }
        i += 1;
    }
    depth <= 0
}

/// What [`ReplyStream::push`] made ready.
#[derive(Debug, Clone, PartialEq)]
pub enum Chunk {
    /// A statement is complete.
    Statement(String),
    /// A2UI messages to hand to the renderer.
    Messages(Vec<Value>),
}

/// Compiles a reply as it arrives.
pub struct ReplyStream {
    stream: ResponseStream,
    splitter: StatementSplitter,
}

impl ReplyStream {
    pub(crate) fn new(stream: ResponseStream) -> Self {
        ReplyStream {
            stream,
            splitter: StatementSplitter::default(),
        }
    }

    /// Adds text. An error means the compiler rejects the reply, whatever follows.
    pub fn push(&mut self, text: &str) -> Result<Vec<Chunk>> {
        let mut out: Vec<Chunk> = self
            .splitter
            .push(text)
            .into_iter()
            .map(Chunk::Statement)
            .collect();
        for part in self
            .stream
            .push(text)
            .map_err(|e| Error::Invalid(e.to_string()))?
        {
            match part {
                Part::A2ui(m) if !m.is_empty() => out.push(Chunk::Messages(m)),
                Part::Error(e) => return Err(Error::Invalid(e.to_string())),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// `reply` without the statements that define `id`.
pub(crate) fn without_statement(reply: &str, id: &str) -> Option<String> {
    let mut splitter = StatementSplitter::default();
    let mut statements = splitter.push(reply);
    statements.extend(splitter.finish());
    let before = statements.len();
    statements.retain(|s| !matches!(StatementKind::of(s), StatementKind::Component { id: i } if i == id));
    (statements.len() < before).then(|| format!("<a2ui>\n{}\n</a2ui>", statements.join("\n")))
}

impl Catalog {
    /// Opens a stream on a reply.
    pub fn open_stream(&self) -> ReplyStream {
        ReplyStream::new(self.parser().open_stream())
    }
}
