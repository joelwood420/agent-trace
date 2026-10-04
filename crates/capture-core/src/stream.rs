//! Rebuild a complete message from a streamed (server-sent events) response.

use serde_json::{Map, Value};
use std::collections::HashMap;

/// What could be rebuilt from a response body.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Rebuilt {
    /// The complete message, if the body held one.
    pub message: Option<serde_json::Value>,
    /// The API message id (`msg_...`), when known.
    pub message_id: Option<String>,
    /// An error message reported by the body, if any.
    pub error: Option<String>,
    /// Event names or delta kinds that were not understood.
    pub unknown_events: Vec<String>,
}

/// Rebuild the message from a response body.
///
/// An event stream (`text/event-stream`) is replayed event by event. A JSON
/// body is taken as the message itself. Anything else gives an empty result.
pub fn rebuild_message(content_type: Option<&str>, body: &str) -> Rebuilt {
    let is_stream = content_type.is_some_and(|c| c.trim_start().starts_with("text/event-stream"));
    if is_stream {
        return rebuild_stream(body);
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Rebuilt::default();
    };
    let message_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("msg_"))
        .map(str::to_string);
    let error = if value.get("type").and_then(Value::as_str) == Some("error") {
        value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };
    Rebuilt {
        message: Some(value),
        message_id,
        error,
        unknown_events: Vec::new(),
    }
}

/// Split a stream into `(event name, data)` pairs. Handles CRLF and joins
/// multiple `data:` lines with a newline.
fn parse_events(body: &str) -> Vec<(String, String)> {
    let text = body.replace("\r\n", "\n");
    let mut events = Vec::new();
    for block in text.split("\n\n") {
        let mut name = String::new();
        let mut data: Vec<&str> = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event:") {
                name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data.push(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        if !name.is_empty() || !data.is_empty() {
            events.push((name, data.join("\n")));
        }
    }
    events
}

fn rebuild_stream(body: &str) -> Rebuilt {
    let mut out = Rebuilt::default();
    let mut message: Option<Value> = None;
    let mut json_buffers: HashMap<usize, String> = HashMap::new();

    for (name, data) in parse_events(body) {
        let parsed: Option<Value> = serde_json::from_str(&data).ok();
        match name.as_str() {
            "message_start" => {
                message = parsed.and_then(|d| d.get("message").cloned());
            }
            "content_block_start" => {
                let Some(d) = parsed else { continue };
                let (Some(index), Some(block)) = (index_of(&d), d.get("content_block")) else {
                    continue;
                };
                if let Some(content) = content_mut(&mut message) {
                    while content.len() <= index {
                        content.push(Value::Null);
                    }
                    content[index] = block.clone();
                }
            }
            "content_block_delta" => {
                let Some(d) = parsed else { continue };
                let Some(index) = index_of(&d) else { continue };
                let delta = d.get("delta").cloned().unwrap_or(Value::Null);
                let kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
                match kind {
                    "input_json_delta" => {
                        if let Some(part) = delta.get("partial_json").and_then(Value::as_str) {
                            json_buffers.entry(index).or_default().push_str(part);
                        }
                    }
                    "text_delta" => append_str(&mut message, index, "text", &delta, "text"),
                    "thinking_delta" => {
                        append_str(&mut message, index, "thinking", &delta, "thinking");
                    }
                    "signature_delta" => {
                        if let (Some(block), Some(sig)) =
                            (block_mut(&mut message, index), delta.get("signature"))
                        {
                            block.insert("signature".to_string(), sig.clone());
                        }
                    }
                    "citations_delta" => {
                        if let (Some(block), Some(citation)) =
                            (block_mut(&mut message, index), delta.get("citation"))
                        {
                            let entry = block
                                .entry("citations".to_string())
                                .or_insert_with(|| Value::Array(Vec::new()));
                            if let Some(list) = entry.as_array_mut() {
                                list.push(citation.clone());
                            }
                        }
                    }
                    other => out
                        .unknown_events
                        .push(format!("content_block_delta:{other}")),
                }
            }
            "content_block_stop" => {
                let Some(index) = parsed.as_ref().and_then(index_of) else {
                    continue;
                };
                let Some(buffer) = json_buffers.remove(&index).filter(|b| !b.is_empty()) else {
                    continue;
                };
                let input = match serde_json::from_str::<Value>(&buffer) {
                    Ok(value) => value,
                    Err(_) => {
                        out.unknown_events
                            .push("content_block_stop:bad_json".to_string());
                        Value::String(buffer)
                    }
                };
                if let Some(block) = block_mut(&mut message, index) {
                    block.insert("input".to_string(), input);
                }
            }
            "message_delta" => {
                let Some(d) = parsed else { continue };
                let Some(Value::Object(msg)) = message.as_mut() else {
                    continue;
                };
                if let Some(delta) = d.get("delta").and_then(Value::as_object) {
                    for (k, v) in delta {
                        msg.insert(k.clone(), v.clone());
                    }
                }
                if let Some(usage) = d.get("usage").and_then(Value::as_object) {
                    let target = msg
                        .entry("usage".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(target) = target.as_object_mut() {
                        for (k, v) in usage {
                            target.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            "message_stop" | "ping" => {}
            "error" => {
                out.error = Some(
                    parsed
                        .as_ref()
                        .and_then(|d| d.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .map_or(data, str::to_string),
                );
            }
            other => out.unknown_events.push(other.to_string()),
        }
    }

    out.message_id = message
        .as_ref()
        .and_then(|m| m.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    out.message = message;
    out
}

fn index_of(data: &Value) -> Option<usize> {
    data.get("index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
}

fn content_mut(message: &mut Option<Value>) -> Option<&mut Vec<Value>> {
    message
        .as_mut()?
        .as_object_mut()?
        .entry("content".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
}

fn block_mut(message: &mut Option<Value>, index: usize) -> Option<&mut Map<String, Value>> {
    content_mut(message)?.get_mut(index)?.as_object_mut()
}

/// Append `delta[delta_key]` to the string field `field` of the block at `index`.
fn append_str(
    message: &mut Option<Value>,
    index: usize,
    field: &str,
    delta: &Value,
    delta_key: &str,
) {
    let Some(part) = delta.get(delta_key).and_then(Value::as_str) else {
        return;
    };
    let Some(block) = block_mut(message, index) else {
        return;
    };
    let entry = block
        .entry(field.to_string())
        .or_insert_with(|| Value::String(String::new()));
    if let Value::String(text) = entry {
        text.push_str(part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SSE: &str = "event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test-model\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\r\n\r\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"look.\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{}}}\n\n\
event: ping\ndata: {\"type\":\"ping\"}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.txt\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: future_event\ndata: {\"type\":\"future_event\"}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":42}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn rebuilds_a_streamed_message() {
        let r = rebuild_message(Some("text/event-stream; charset=utf-8"), SSE);
        assert_eq!(r.message_id.as_deref(), Some("msg_test1"));
        assert_eq!(r.unknown_events, ["future_event"]);
        let m = r.message.expect("message");
        assert_eq!(m["stop_reason"], json!("tool_use"));
        assert_eq!(
            m["usage"],
            json!({ "input_tokens": 10, "output_tokens": 42 })
        );
        assert_eq!(
            m["content"][0],
            json!({ "type": "thinking", "thinking": "Let me look.", "signature": "sig" })
        );
        assert_eq!(m["content"][1]["input"], json!({ "path": "a.txt" }));
    }

    #[test]
    fn json_error_body_is_reported() {
        let r = rebuild_message(
            Some("application/json"),
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert_eq!(r.error.as_deref(), Some("Overloaded"));
        assert_eq!(r.message_id, None);
    }

    #[test]
    fn stream_error_event_is_reported() {
        let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"Boom\"}}\n\n";
        assert_eq!(
            rebuild_message(Some("text/event-stream"), body)
                .error
                .as_deref(),
            Some("Boom")
        );
    }

    #[test]
    fn non_json_body_gives_nothing() {
        assert_eq!(
            rebuild_message(Some("text/plain"), "hello"),
            Rebuilt::default()
        );
    }
}
