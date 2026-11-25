use std::sync::{Arc, OnceLock};

use a2ui_express::compile::Compiler;
use a2ui_express::prompt::{PromptOptions, PromptStyle, generate_prompt};
use a2ui_express::response::{ExpressParser, Part};
use a2ui_express::schema::Catalog as Schema;
use serde_json::Value;

use crate::{Error, Result, SURFACE_ID, grammar, prompt, stream};

const EUI: &str = include_str!("../vendor/eui/catalog/core.schema.json");

/// A catalog with everything made from it. Cheap to clone.
#[derive(Clone)]
pub struct Catalog {
    inner: Arc<Inner>,
}

struct Inner {
    document: Value,
    parser: ExpressParser,
    system: String,
    lark: OnceLock<String>,
}

/// A reply the compiler accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    /// The `<a2ui>…</a2ui>` block.
    pub express: String,
    /// The A2UI messages it compiles to.
    pub messages: Vec<Value>,
    /// Ids of statements dropped because nothing reaches them from `root`.
    pub pruned: Vec<String>,
}

impl Catalog {
    /// eui's core catalog, the one the default models are trained on.
    pub fn eui() -> Self {
        static CATALOG: OnceLock<Catalog> = OnceLock::new();
        CATALOG
            .get_or_init(|| {
                let document = serde_json::from_str(EUI).expect("the vendored catalog is JSON");
                Self::build(document, Some(prompt::SYSTEM)).expect("the vendored catalog loads")
            })
            .clone()
    }

    /// Any A2UI 1.0 catalog schema; one that does not declare `protocolVersion` 1.0 is refused. Its signatures go into the system prompt.
    ///
    /// The default models know eui's catalog well and no other: the grammar and the compiler still keep
    /// their replies valid, but how well they use another catalog is not something they were trained for.
    pub fn from_json(document: Value) -> Result<Self> {
        Self::build(document, None)
    }

    fn build(document: Value, system: Option<&str>) -> Result<Self> {
        let err = |e: &dyn std::fmt::Display| Error::Catalog(e.to_string());
        let schema = Schema::from_json(document.clone()).map_err(|e| err(&e))?;
        let compiler = Compiler::new(vec![schema], None).map_err(|e| err(&e))?;
        let system = match system {
            Some(s) => s.to_owned(),
            None => {
                let options = PromptOptions {
                    style: PromptStyle::Compact,
                    ..Default::default()
                };
                let full = generate_prompt(compiler.catalogs(), &options).map_err(|e| err(&e))?;
                let at = full
                    .find("## Component Signatures")
                    .ok_or_else(|| Error::Catalog("the prompt has no signatures".into()))?;
                format!("{}{}", prompt::CATALOG_HEADER, &full[at..])
            }
        };
        Ok(Catalog {
            inner: Arc::new(Inner {
                parser: ExpressParser::new(compiler, SURFACE_ID),
                document,
                system,
                lark: OnceLock::new(),
            }),
        })
    }

    /// The catalog's JSON schema.
    pub fn document(&self) -> &Value {
        &self.inner.document
    }

    /// The system prompt that goes with the catalog.
    pub fn system_prompt(&self) -> &str {
        &self.inner.system
    }

    /// The grammar for llguidance.
    pub fn lark(&self) -> &str {
        self.inner
            .lark
            .get_or_init(|| grammar::lark(&self.inner.document))
    }

    /// The grammar for llama.cpp.
    pub fn gbnf(&self) -> String {
        grammar::gbnf(&self.inner.document)
    }

    #[cfg(feature = "llama")]
    pub(crate) fn key(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    pub(crate) fn parser(&self) -> &ExpressParser {
        &self.inner.parser
    }

    /// Compiles a whole reply against the catalog.
    pub fn validate(&self, reply: &str) -> Result<Vec<Value>> {
        let reply = reply.trim();
        if !reply.starts_with("<a2ui>") || !reply.ends_with(prompt::REPLY_END) {
            return Err(Error::Invalid(
                "the reply is not one complete <a2ui> block".into(),
            ));
        }
        let parts = self
            .parser()
            .parse_response(reply, true)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let mut blocks = Vec::new();
        for part in parts {
            match part {
                Part::A2ui(m) => blocks.push(m),
                Part::Error(e) => return Err(Error::Invalid(e.to_string())),
                Part::Text(_) => {}
            }
        }
        match blocks.len() {
            1 if !blocks[0].is_empty() => Ok(blocks.remove(0)),
            1 => Err(Error::Invalid("the block is empty".into())),
            n => Err(Error::Invalid(format!("{n} blocks instead of one"))),
        }
    }

    /// [`Self::validate`], first dropping the statements the compiler calls unreachable from `root`. They
    /// never render, so dropping them changes nothing on screen; any other error is returned.
    pub fn validate_pruned(&self, reply: &str) -> Result<Validated> {
        let mut express = reply.trim().to_owned();
        let mut pruned = Vec::new();
        loop {
            match self.validate(&express) {
                Ok(messages) => {
                    return Ok(Validated {
                        express,
                        messages,
                        pruned,
                    });
                }
                Err(Error::Invalid(message)) => {
                    let next = unreachable_id(&message)
                        .or_else(|| refers_to(&message, &pruned))
                        .and_then(|id| Some((id, stream::without_statement(&express, id)?)));
                    let Some((id, shorter)) = next else {
                        return Err(Error::Invalid(message));
                    };
                    pruned.push(id.to_owned());
                    express = shorter;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// The component that refers to a dropped one. It is itself unreachable, or the dropped one would not be.
fn refers_to<'a>(message: &'a str, pruned: &[String]) -> Option<&'a str> {
    let (head, rest) = message.split_once("' references child '")?;
    let child = rest.split('\'').next()?;
    let id = &head[head.rfind("Component '")? + "Component '".len()..];
    pruned.iter().any(|p| p == child).then_some(id)
}

fn unreachable_id(message: &str) -> Option<&str> {
    let at = message.find("' is not reachable from 'root'")?;
    let start = message[..at].rfind("Component '")? + "Component '".len();
    Some(&message[start..at])
}
