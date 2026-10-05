//! Compare two captured requests: what changed in the messages, system
//! prompt, tools, settings and beta flags.

use serde::Serialize;
use serde_json::Value;

use crate::record::CaptureRecord;
use crate::request::{MESSAGES_KEY, SYSTEM_KEY, TOOLS_KEY, betas, content_hash, tool_names};

const PREVIEW_CHARS: usize = 160;

/// A short description of one message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageSummary {
    /// Position in its own request.
    pub index: usize,
    /// The message role, or `?`.
    pub role: String,
    /// The `type` of each content block (a string content gives `text`).
    pub block_types: Vec<String>,
    /// Length of the content as JSON text.
    pub chars: usize,
    /// The first text found, cut to 160 characters, newlines as spaces.
    pub preview: String,
}

/// One setting whose value differs between two requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingChange {
    /// The body key.
    pub key: String,
    /// Value in the previous request, `None` if absent.
    pub before: Option<Value>,
    /// Value in the next request, `None` if absent.
    pub after: Option<Value>,
}

/// What changed between a request and the one after it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestDiff {
    /// Number of leading messages equal in both (ignoring `cache_control`).
    pub shared_prefix: usize,
    /// Previous messages from the shared prefix on.
    pub removed: Vec<MessageSummary>,
    /// Next messages from the shared prefix on.
    pub added: Vec<MessageSummary>,
    /// The system prompt differs.
    pub system_changed: bool,
    /// The tool set differs.
    pub tools_changed: bool,
    /// Tool names only in the next request.
    pub tools_added: Vec<String>,
    /// Tool names only in the previous request.
    pub tools_removed: Vec<String>,
    /// Settings whose value differs.
    pub settings_changed: Vec<SettingChange>,
    /// Beta flags only in the next request.
    pub betas_added: Vec<String>,
    /// Beta flags only in the previous request.
    pub betas_removed: Vec<String>,
}

/// Compare two requests. `None` if either body is not a JSON object.
pub fn diff(previous: &CaptureRecord, next: &CaptureRecord) -> Option<RequestDiff> {
    let prev = previous.request.body.json()?.as_object()?;
    let next_body = next.request.body.json()?.as_object()?;

    let empty = Vec::new();
    let prev_msgs = prev
        .get(MESSAGES_KEY)
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let next_msgs = next_body
        .get(MESSAGES_KEY)
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let shared_prefix = prev_msgs
        .iter()
        .zip(next_msgs)
        .take_while(|(a, b)| strip_cache_control(a) == strip_cache_control(b))
        .count();

    let summarise_from = |msgs: &[Value]| {
        msgs.iter()
            .enumerate()
            .skip(shared_prefix)
            .map(|(i, m)| summarise_message(i, m))
            .collect::<Vec<_>>()
    };

    let hash_of = |v: Option<&Value>| v.map(content_hash);
    let prev_names = tool_names(prev.get(TOOLS_KEY));
    let next_names = tool_names(next_body.get(TOOLS_KEY));

    let mut keys: Vec<&String> = Vec::new();
    for k in prev.keys().chain(next_body.keys()) {
        if ![SYSTEM_KEY, TOOLS_KEY, MESSAGES_KEY].contains(&k.as_str()) && !keys.contains(&k) {
            keys.push(k);
        }
    }
    let settings_changed = keys
        .into_iter()
        .filter(|k| prev.get(*k) != next_body.get(*k))
        .map(|k| SettingChange {
            key: k.clone(),
            before: prev.get(k).cloned(),
            after: next_body.get(k).cloned(),
        })
        .collect();

    let prev_betas = betas(previous);
    let next_betas = betas(next);

    Some(RequestDiff {
        shared_prefix,
        removed: summarise_from(prev_msgs),
        added: summarise_from(next_msgs),
        system_changed: hash_of(prev.get(SYSTEM_KEY)) != hash_of(next_body.get(SYSTEM_KEY)),
        tools_changed: hash_of(prev.get(TOOLS_KEY)) != hash_of(next_body.get(TOOLS_KEY)),
        tools_added: only_in(&next_names, &prev_names),
        tools_removed: only_in(&prev_names, &next_names),
        settings_changed,
        betas_added: only_in(&next_betas, &prev_betas),
        betas_removed: only_in(&prev_betas, &next_betas),
    })
}

fn only_in(items: &[String], other: &[String]) -> Vec<String> {
    items
        .iter()
        .filter(|i| !other.contains(i))
        .cloned()
        .collect()
}

fn strip_cache_control(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| k.as_str() != "cache_control")
                .map(|(k, v)| (k.clone(), strip_cache_control(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_cache_control).collect()),
        other => other.clone(),
    }
}

/// Describe one message. `index` is its position in its request.
pub fn summarise_message(index: usize, message: &Value) -> MessageSummary {
    let content = message.get("content");
    let block_types = match content {
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| {
                b.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string()
            })
            .collect(),
        Some(Value::String(_)) => vec!["text".to_string()],
        _ => Vec::new(),
    };
    let chars = content
        .map(|c| serde_json::to_string(c).unwrap_or_default().chars().count())
        .unwrap_or(0);
    let preview = content
        .and_then(first_text)
        .unwrap_or_default()
        .chars()
        .take(PREVIEW_CHARS)
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    MessageSummary {
        index,
        role: message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        block_types,
        chars,
        preview,
    }
}

/// The first text in a content value: a string, a `text` block, or the first
/// text inside a `tool_result`.
fn first_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => blocks.iter().find_map(|b| {
            if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                b.get("content").and_then(first_text)
            } else {
                b.get("text").and_then(Value::as_str).map(String::from)
            }
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Body, CaptureRecord, CapturedRequest, Header};
    use serde_json::{Value, json};

    fn record(body: Value, betas: &str) -> CaptureRecord {
        CaptureRecord {
            id: "x".into(),
            started_at_ms: 0,
            first_byte_at_ms: None,
            ended_at_ms: None,
            request: CapturedRequest {
                method: "POST".into(),
                path: "/v1/messages".into(),
                headers: vec![Header {
                    name: "anthropic-beta".into(),
                    value: betas.into(),
                }],
                body: Body::Json(body),
            },
            response: None,
            message_id: None,
            error: None,
        }
    }

    fn user(text: &str) -> Value {
        json!({ "role": "user", "content": [{ "type": "text", "text": text }] })
    }
    fn assistant(text: &str) -> Value {
        json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] })
    }
    fn with_cache(mut m: Value) -> Value {
        m["content"][0]["cache_control"] = json!({ "type": "ephemeral" });
        m
    }

    fn body(messages: Vec<Value>, tools: &[&str], max_tokens: u64) -> Value {
        json!({
            "model": "test-model",
            "max_tokens": max_tokens,
            "system": [{ "type": "text", "text": "You are a test agent." }],
            "tools": tools.iter().map(|n| json!({ "name": n, "description": "d", "input_schema": { "type": "object" } })).collect::<Vec<_>>(),
            "messages": messages,
        })
    }

    #[test]
    fn plain_growth_adds_messages() {
        let a = record(body(vec![user("one")], &["Read"], 100), "b1");
        let b = record(
            body(
                vec![user("one"), assistant("two"), user("three")],
                &["Read"],
                100,
            ),
            "b1",
        );
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 1);
        assert!(d.removed.is_empty());
        assert_eq!(
            d.added
                .iter()
                .map(|m| m.preview.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert!(!d.system_changed && !d.tools_changed && d.settings_changed.is_empty());
    }

    #[test]
    fn cache_marker_moves_do_not_count_as_changes() {
        let a = record(
            body(
                vec![user("one"), with_cache(assistant("two"))],
                &["Read"],
                100,
            ),
            "b1",
        );
        let b = record(
            body(
                vec![user("one"), assistant("two"), with_cache(user("three"))],
                &["Read"],
                100,
            ),
            "b1",
        );
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 2);
        assert_eq!(d.added.len(), 1);
    }

    #[test]
    fn compaction_replaces_history_with_a_summary() {
        let a = record(
            body(
                vec![user("one"), assistant("two"), user("three")],
                &["Read"],
                100,
            ),
            "b1",
        );
        let b = record(
            body(
                vec![user("Summary of the conversation so far"), user("four")],
                &["Read"],
                100,
            ),
            "b1",
        );
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 0);
        assert_eq!(d.removed.len(), 3);
        assert_eq!(d.added[0].preview, "Summary of the conversation so far");
    }

    #[test]
    fn tools_settings_and_betas_changes_are_listed() {
        let a = record(body(vec![user("one")], &["Read", "Grep"], 100), "b1, b2");
        let b = record(body(vec![user("one")], &["Read", "Write"], 200), "b2,b3");
        let d = diff(&a, &b).expect("diff");
        assert!(d.tools_changed);
        assert_eq!(d.tools_added, ["Write"]);
        assert_eq!(d.tools_removed, ["Grep"]);
        assert_eq!(
            d.settings_changed,
            [SettingChange {
                key: "max_tokens".into(),
                before: Some(json!(100)),
                after: Some(json!(200))
            }]
        );
        assert_eq!(d.betas_added, ["b3"]);
        assert_eq!(d.betas_removed, ["b1"]);
    }

    #[test]
    fn non_json_bodies_give_no_diff() {
        let mut a = record(json!({}), "");
        a.request.body = Body::Text("x".into());
        let b = record(body(vec![], &[], 1), "");
        assert_eq!(diff(&a, &b), None);
    }

    #[test]
    fn message_summary_handles_strings_tool_results_and_long_text() {
        let s = summarise_message(3, &json!({ "role": "user", "content": "a\nb" }));
        assert_eq!((s.index, s.preview.as_str()), (3, "a b"));
        assert_eq!(s.block_types, ["text"]);
        let r = summarise_message(
            0,
            &json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t", "content": [{ "type": "text", "text": "out" }] }] }),
        );
        assert_eq!(r.block_types, ["tool_result"]);
        assert_eq!(r.preview, "out");
        let long = summarise_message(0, &user(&"e".repeat(300)));
        assert_eq!(long.preview.chars().count(), 160);
    }
}
