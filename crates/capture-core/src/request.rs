//! Facts about one captured request: hashes of the system prompt and tool
//! set, tool names, settings and beta flags.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::record::CaptureRecord;

/// Body key holding the system prompt.
pub const SYSTEM_KEY: &str = "system";
/// Body key holding the tool definitions.
pub const TOOLS_KEY: &str = "tools";
/// Body key holding the conversation messages.
pub const MESSAGES_KEY: &str = "messages";

/// sha256 hex of the compact JSON text of the value (key order as stored).
pub fn content_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A short description of one request body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestSummary {
    /// The `model` field.
    pub model: Option<String>,
    /// Content hash of the system prompt, when present.
    pub system_hash: Option<String>,
    /// Content hash of the tool set, when present.
    pub tools_hash: Option<String>,
    /// Each tool's name, in order.
    pub tool_names: Vec<String>,
    /// Total length of the system prompt text.
    pub system_chars: usize,
    /// Number of messages.
    pub message_count: usize,
    /// Every top-level body key except system, tools and messages, in body order.
    pub settings: Vec<(String, Value)>,
    /// Comma-separated values of the anthropic-beta request header, trimmed.
    pub betas: Vec<String>,
}

/// Summarise a record's request. `None` if the body is not a JSON object.
pub fn summarise(record: &CaptureRecord) -> Option<RequestSummary> {
    let body = record.request.body.json()?.as_object()?;
    let system = body.get(SYSTEM_KEY);
    let tools = body.get(TOOLS_KEY);
    Some(RequestSummary {
        model: body.get("model").and_then(Value::as_str).map(String::from),
        system_hash: system.map(content_hash),
        tools_hash: tools.map(content_hash),
        tool_names: tool_names(tools),
        system_chars: system.map(system_chars).unwrap_or(0),
        message_count: body
            .get(MESSAGES_KEY)
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        settings: body
            .iter()
            .filter(|(k, _)| ![SYSTEM_KEY, TOOLS_KEY, MESSAGES_KEY].contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        betas: betas(record),
    })
}

pub(crate) fn tool_names(tools: Option<&Value>) -> Vec<String> {
    tools
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|t| t.get("name").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn betas(record: &CaptureRecord) -> Vec<String> {
    record
        .header("anthropic-beta")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn system_chars(system: &Value) -> usize {
    match system {
        Value::String(s) => s.chars().count(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .map(|t| t.chars().count())
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Body, CapturedRequest, Header};
    use serde_json::json;

    #[test]
    fn hash_depends_on_content_and_order() {
        assert_eq!(
            content_hash(&json!({ "a": 1 })),
            content_hash(&json!({ "a": 1 }))
        );
        assert_ne!(
            content_hash(&json!({ "a": 1, "b": 2 })),
            content_hash(&json!({ "b": 2, "a": 1 }))
        );
        assert_eq!(content_hash(&json!(null)).len(), 64);
    }

    #[test]
    fn summarise_reads_the_request() {
        let record = CaptureRecord {
            id: "x".into(),
            started_at_ms: 0,
            first_byte_at_ms: None,
            ended_at_ms: None,
            request: CapturedRequest {
                method: "POST".into(),
                path: "/v1/messages".into(),
                headers: vec![Header {
                    name: "anthropic-beta".into(),
                    value: " b1 , b2,".into(),
                }],
                body: Body::Json(json!({
                    "model": "test-model",
                    "max_tokens": 100,
                    "system": [{ "type": "text", "text": "You are a test agent." }],
                    "tools": [{ "name": "Read" }, { "name": "Grep" }],
                    "messages": [{ "role": "user", "content": "hi" }],
                })),
            },
            response: None,
            message_id: None,
            error: None,
        };
        let s = summarise(&record).expect("summary");
        assert_eq!(s.model.as_deref(), Some("test-model"));
        assert_eq!(s.tool_names, ["Read", "Grep"]);
        assert_eq!(s.system_chars, 21);
        assert_eq!(s.message_count, 1);
        assert_eq!(
            s.settings,
            [
                ("model".to_string(), json!("test-model")),
                ("max_tokens".to_string(), json!(100))
            ]
        );
        assert_eq!(s.betas, ["b1", "b2"]);
        assert!(s.system_hash.is_some() && s.tools_hash.is_some());
    }
}
