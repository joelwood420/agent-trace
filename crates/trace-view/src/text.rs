//! Small text helpers for labels.

use serde_json::Value;
use trace_core::{ContentBlock, StopReason};

/// Longest prompt preview, in characters.
pub const PROMPT_PREVIEW_CHARS: usize = 80;
/// Longest tool input hint, in characters.
pub const INPUT_HINT_CHARS: usize = 40;
/// Longest marker label, in characters.
pub const MARKER_LABEL_CHARS: usize = 80;

/// Input keys tried first for a tool input hint. These are generic names,
/// not tied to any harness. Anything else falls back to key order.
const HINT_KEYS: [&str; 6] = ["command", "file_path", "pattern", "query", "url", "path"];

/// One line, at most `max` characters, ending in "..." if cut.
pub fn short(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
    format!("{}...", cut.trim_end())
}

/// A one-line preview of a prompt.
pub fn prompt_preview(blocks: &[ContentBlock]) -> String {
    let text = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let preview = short(&text, PROMPT_PREVIEW_CHARS);
    if !preview.is_empty() {
        return preview;
    }
    if blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
    {
        return "(image)".to_string();
    }
    "(no text)".to_string()
}

/// A short hint about what a tool was asked to do.
///
/// If the input is a string, that string. If it is an object, the first
/// non-empty string value under one of `HINT_KEYS`, else the first
/// non-empty string value by sorted key name. Sorting makes the result the
/// same whatever order the JSON map keeps its keys in.
pub fn input_hint(input: &Value) -> Option<String> {
    let found = match input {
        Value::String(s) => Some(s.as_str()),
        Value::Object(map) => {
            let preferred = HINT_KEYS
                .iter()
                .filter_map(|k| map.get(*k).and_then(Value::as_str))
                .find(|s| !s.trim().is_empty());
            preferred.or_else(|| {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                keys.into_iter()
                    .filter_map(|k| map.get(k).and_then(Value::as_str))
                    .find(|s| !s.trim().is_empty())
            })
        }
        _ => None,
    };
    found
        .map(|s| short(s, INPUT_HINT_CHARS))
        .filter(|s| !s.is_empty())
}

/// The JSON name of a stop reason, for example `"tool_use"`.
pub fn stop_reason_name(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".to_string(),
        StopReason::ToolUse => "tool_use".to_string(),
        StopReason::MaxTokens => "max_tokens".to_string(),
        StopReason::StopSequence => "stop_sequence".to_string(),
        StopReason::Other(name) => name.clone(),
    }
}

/// "1 model call" or "3 model calls".
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn short_flattens_and_cuts() {
        assert_eq!(short("a\n b   c", 10), "a b c");
        assert_eq!(short("abcdefghijkl", 8), "abcde...");
    }

    #[test]
    fn hint_prefers_generic_keys_then_sorted_keys() {
        let grep = json!({"output_mode": "content", "pattern": "TODO", "path": "src"});
        assert_eq!(input_hint(&grep).as_deref(), Some("TODO"));
        let other = json!({"zeta": "last", "alpha": "", "beta": "first"});
        assert_eq!(input_hint(&other).as_deref(), Some("first"));
        assert_eq!(input_hint(&json!({"n": 3})), None);
        assert_eq!(input_hint(&json!("plain")).as_deref(), Some("plain"));
    }

    #[test]
    fn preview_falls_back_for_non_text() {
        let image = [ContentBlock::Image { media_type: None }];
        assert_eq!(prompt_preview(&image), "(image)");
        assert_eq!(prompt_preview(&[]), "(no text)");
    }
}
