//! The prompt contract of the fine-tuned models. It must match their training data byte for byte.

use serde::Serialize;
use serde_json::{Value, ser::PrettyFormatter};

/// System prompt of the default models, which know eui's catalog by heart.
pub const SYSTEM: &str = "You build user interfaces as A2UI Express with the eui core catalog. Reply with exactly one <a2ui>…</a2ui> block that builds a single surface whose id is `main`.";

/// Start of the system prompt for a custom catalog; the catalog's signatures follow.
pub const CATALOG_HEADER: &str = "You build user interfaces as A2UI Express. Reply with exactly one <a2ui>…</a2ui> block that builds a single surface whose id is `main`, using only the components and functions below.\n\n";

/// What every reply starts with.
pub const REPLY_START: &str = "<a2ui>\nsurface(\"main\")\n";
/// What every reply ends with.
pub const REPLY_END: &str = "</a2ui>";

/// User message of a new screen: the request, then the caller's data.
pub fn generate_text(request: &str, data: Option<&Value>) -> String {
    match data {
        None => request.to_owned(),
        Some(data) => format!("{request}\n\nData:\n```json\n{}\n```", json_indent_1(data)),
    }
}

/// User message of a change: the current screen, then the instruction.
pub fn refine_text(current: &str, change: &str) -> String {
    let current = current.trim();
    if current.contains("<a2ui>") {
        format!("Current UI:\n{current}\n\nChange: {change}")
    } else {
        format!("Current UI:\n<a2ui>\n{current}\n</a2ui>\n\nChange: {change}")
    }
}

/// `json.dumps(value, ensure_ascii=False, indent=1)`.
fn json_indent_1(value: &Value) -> String {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, PrettyFormatter::with_indent(b" "));
    value.serialize(&mut ser).expect("a JSON value serializes");
    String::from_utf8(out).expect("JSON is UTF-8")
}

/// Qwen's chat template (as in the GGUF) up to the user's text. Holds control tokens.
pub fn chat_head() -> &'static str {
    "<|im_start|>system\n"
}

/// Between system and user text.
pub fn chat_middle() -> &'static str {
    "<|im_end|>\n<|im_start|>user\n"
}

/// After the user's text: thinking is off, so the turn starts with an empty think block.
pub fn chat_tail() -> &'static str {
    "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_texts_match_training() {
        assert_eq!(generate_text("Show it", None), "Show it");
        assert_eq!(
            generate_text("Show it", Some(&json!({"a": [1, 2.5], "ü": {}, "e": []}))),
            "Show it\n\nData:\n```json\n{\n \"a\": [\n  1,\n  2.5\n ],\n \"ü\": {},\n \"e\": []\n}\n```"
        );
        let ui = "<a2ui>\nsurface(\"main\")\nroot = Text(text=\"x\")\n</a2ui>";
        assert_eq!(
            refine_text(ui, "bold"),
            format!("Current UI:\n{ui}\n\nChange: bold")
        );
        assert_eq!(
            refine_text("surface(\"main\")\nroot = Text(text=\"x\")", "bold"),
            refine_text(ui, "bold")
        );
    }
}
