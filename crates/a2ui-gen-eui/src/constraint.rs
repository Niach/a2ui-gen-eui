//! Grammar-constrained decoding with llguidance.

use llguidance::{Matcher, ParserFactory, api::TopLevelGrammar};
use toktrie::TokEnv;

use crate::{Error, Result};

/// Builds matchers for one vocabulary.
pub struct ConstraintFactory {
    factory: ParserFactory,
}

/// A compiled grammar over one vocabulary; make one [`Matcher`] per reply.
pub struct Constraint {
    matcher: Matcher,
}

impl ConstraintFactory {
    /// For the vocabulary of `tok_env`.
    pub fn new(tok_env: &TokEnv) -> Result<Self> {
        let mut factory = ParserFactory::new_simple(tok_env).map_err(|e| Error::Grammar(e.to_string()))?;
        factory.quiet();
        Ok(ConstraintFactory { factory })
    }

    /// Compiles a Lark grammar ([`crate::grammar::lark`]).
    pub fn compile(&self, lark: &str) -> Result<Constraint> {
        let parser = self
            .factory
            .create_parser(TopLevelGrammar::from_lark(lark.to_owned()));
        let matcher = Matcher::new(parser);
        match matcher.get_error() {
            Some(e) => Err(Error::Grammar(e)),
            None => Ok(Constraint { matcher }),
        }
    }
}

impl Constraint {
    /// A matcher at the start of the grammar.
    pub fn matcher(&self) -> Matcher {
        self.matcher.deep_clone()
    }
}

/// Whether `text` is in the language of `lark`. For tests and tools: it matches byte by byte.
pub fn accepts(constraint: &Constraint, text: &str) -> std::result::Result<(), String> {
    let mut m = constraint.matcher();
    for (i, b) in text.bytes().enumerate() {
        if m.is_stopped() {
            return Err(format!(
                "the grammar ended before: {}",
                &text[i.min(text.len())..]
            ));
        }
        if m.consume_token(u32::from(b)).is_err() {
            let seen = String::from_utf8_lossy(&text.as_bytes()[i.saturating_sub(80)..=i]).into_owned();
            return Err(format!("rejected at byte {i}: …{seen}"));
        }
    }
    match m.is_accepting() {
        Ok(true) => Ok(()),
        _ if m.is_stopped() => Ok(()),
        _ => Err("incomplete".into()),
    }
}
