//! Rebuild a complete message from a streamed (server-sent events) response.

use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// What could be rebuilt from a response body.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Rebuilt {
    /// The complete message, if the body held one.
    pub message: Option<serde_json::Value>,
    /// The API message id (`msg_...`), when known.
    pub message_id: Option<String>,
    /// An error message reported by the body, if any.
    pub error: Option<String>,
    /// Event names, delta kinds and unexpected shapes that were not applied
    /// to the message, for example `future_event`, `content_block_delta:orphan`
    /// or `message_start:bad_data`.
    pub unknown_events: Vec<String>,
}

/// Rebuild the message from a response body.
///
/// An event stream (`text/event-stream`, matched ignoring case) is replayed
/// event by event. A JSON body is taken as the message itself. Anything else
/// gives an empty result. Anything unexpected in a stream is noted in
/// `unknown_events` rather than dropped silently.
pub fn rebuild_message(content_type: Option<&str>, body: &str) -> Rebuilt {
    let is_stream = content_type.is_some_and(|c| {
        c.trim_start()
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
    });
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
/// multiple `data:` lines with a newline. An event with no `event:` line has
/// an empty name.
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
    let mut json_buffers: BTreeMap<usize, String> = BTreeMap::new();

    for (name, data) in parse_events(body) {
        let parsed: Option<Value> = serde_json::from_str(&data).ok();
        let object = parsed.as_ref().filter(|v| v.is_object());
        match name.as_str() {
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
            "message_start" => {
                match object
                    .and_then(|d| d.get("message"))
                    .filter(|m| m.is_object())
                {
                    Some(m) => message = Some(m.clone()),
                    None => out.unknown_events.push("message_start:bad_data".into()),
                }
            }
            "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta" => {
                let Some(d) = object else {
                    out.unknown_events.push(format!("{name}:bad_data"));
                    continue;
                };
                if message.is_none() {
                    out.unknown_events
                        .push(format!("{name}:before_message_start"));
                    continue;
                }
                match name.as_str() {
                    "content_block_start" => block_start(d, &mut message, &mut out),
                    "content_block_delta" => {
                        block_delta(d, &mut message, &mut json_buffers, &mut out);
                    }
                    "content_block_stop" => {
                        block_stop(d, &mut message, &mut json_buffers, &mut out);
                    }
                    _ => message_delta(d, &mut message),
                }
            }
            "" => out.unknown_events.push("<unnamed>".into()),
            other => out.unknown_events.push(other.to_string()),
        }
    }

    // A stream cut short can leave tool input that never got its stop event.
    for (index, buffer) in json_buffers {
        if buffer.is_empty() {
            continue;
        }
        if let Some(block) = block_mut(&mut message, index) {
            block.insert("input".to_string(), Value::String(buffer));
        }
        out.unknown_events.push("truncated_input_json".into());
    }

    out.message_id = message
        .as_ref()
        .and_then(|m| m.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    out.message = message;
    out
}

fn block_start(d: &Value, message: &mut Option<Value>, out: &mut Rebuilt) {
    let (Some(index), Some(block)) = (index_of(d), d.get("content_block")) else {
        out.unknown_events
            .push("content_block_start:bad_data".into());
        return;
    };
    let Some(content) = content_mut(message) else {
        out.unknown_events
            .push("content_block_start:bad_data".into());
        return;
    };
    // Only the next block or an existing one is valid. This also bounds memory.
    match index.cmp(&content.len()) {
        Ordering::Less => content[index] = block.clone(),
        Ordering::Equal => content.push(block.clone()),
        Ordering::Greater => out
            .unknown_events
            .push("content_block_start:bad_index".into()),
    }
}

fn block_delta(
    d: &Value,
    message: &mut Option<Value>,
    json_buffers: &mut BTreeMap<usize, String>,
    out: &mut Rebuilt,
) {
    let Some(index) = index_of(d) else {
        out.unknown_events
            .push("content_block_delta:bad_data".into());
        return;
    };
    let Some(block) = block_mut(message, index) else {
        out.unknown_events.push("content_block_delta:orphan".into());
        return;
    };
    let delta = d.get("delta").unwrap_or(&Value::Null);
    let kind = delta.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "input_json_delta" => {
            if let Some(part) = delta.get("partial_json").and_then(Value::as_str) {
                json_buffers.entry(index).or_default().push_str(part);
            }
        }
        "text_delta" => append_str(block, "text", delta.get("text")),
        "thinking_delta" => append_str(block, "thinking", delta.get("thinking")),
        "signature_delta" => {
            if let Some(sig) = delta.get("signature") {
                block.insert("signature".to_string(), sig.clone());
            }
        }
        "citations_delta" => {
            if let Some(citation) = delta.get("citation") {
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

fn block_stop(
    d: &Value,
    message: &mut Option<Value>,
    json_buffers: &mut BTreeMap<usize, String>,
    out: &mut Rebuilt,
) {
    let Some(index) = index_of(d) else {
        out.unknown_events
            .push("content_block_stop:bad_data".into());
        return;
    };
    let Some(block) = block_mut(message, index) else {
        out.unknown_events.push("content_block_stop:orphan".into());
        return;
    };
    let Some(buffer) = json_buffers.remove(&index).filter(|b| !b.is_empty()) else {
        return;
    };
    let input = match serde_json::from_str::<Value>(&buffer) {
        Ok(value) => value,
        Err(_) => {
            out.unknown_events
                .push("content_block_stop:bad_json".into());
            Value::String(buffer)
        }
    };
    block.insert("input".to_string(), input);
}

fn message_delta(d: &Value, message: &mut Option<Value>) {
    let Some(Value::Object(msg)) = message.as_mut() else {
        return;
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

/// Append `part` to the string field `field` of `block`.
fn append_str(block: &mut Map<String, Value>, field: &str, part: Option<&Value>) {
    let Some(part) = part.and_then(Value::as_str) else {
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

    const STREAM: &str = "text/event-stream";

    /// A stream header: message_start with an empty content array and one text block.
    const START: &str = "event: message_start\ndata: {\"message\":{\"id\":\"msg_t\",\"content\":[]}}\n\n\
event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";

    fn run(events: &str) -> Rebuilt {
        rebuild_message(Some(STREAM), &format!("{START}{events}"))
    }

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

    #[test]
    fn content_type_is_matched_ignoring_case() {
        let r = rebuild_message(Some("Text/Event-Stream; charset=utf-8"), SSE);
        assert_eq!(r.message_id.as_deref(), Some("msg_test1"));
    }

    #[test]
    fn text_and_citation_deltas_are_applied() {
        let r = run(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi \"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"there\"}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"citations_delta\",\"citation\":{\"n\":1}}}\n\n",
        );
        let m = r.message.expect("message");
        assert_eq!(m["content"][0]["text"], json!("Hi there"));
        assert_eq!(m["content"][0]["citations"], json!([{ "n": 1 }]));
        assert!(r.unknown_events.is_empty());
    }

    #[test]
    fn defensive_paths_are_marked_not_hidden() {
        // (events appended after the text block start, expected markers)
        let cases: [(&str, &[&str]); 8] = [
            (
                "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"new_delta\"}}\n\n",
                &["content_block_delta:new_delta"],
            ),
            (
                "event: content_block_start\ndata: {\"index\":4000000000000,\"content_block\":{\"type\":\"text\"}}\n\n",
                &["content_block_start:bad_index"],
            ),
            (
                "event: content_block_delta\ndata: [1,2]\n\n",
                &["content_block_delta:bad_data"],
            ),
            (
                "event: content_block_delta\ndata: not json\n\n",
                &["content_block_delta:bad_data"],
            ),
            (
                "event: content_block_delta\ndata: {\"index\":5,\"delta\":{\"type\":\"text_delta\",\"text\":\"x\"}}\n\n",
                &["content_block_delta:orphan"],
            ),
            (
                "event: content_block_stop\ndata: {\"index\":5}\n\n",
                &["content_block_stop:orphan"],
            ),
            ("data: {\"type\":\"mystery\"}\n\n", &["<unnamed>"]),
            (
                "event: message_start\ndata: oops\n\n",
                &["message_start:bad_data"],
            ),
        ];
        for (events, expected) in cases {
            let r = run(events);
            assert_eq!(r.unknown_events, expected, "for {events}");
            // The earlier message is never discarded or grown by a bad event.
            let m = r.message.expect("message kept");
            assert_eq!(m["id"], json!("msg_t"));
            assert_eq!(
                m["content"].as_array().map(Vec::len),
                Some(1),
                "for {events}"
            );
        }
    }

    #[test]
    fn huge_index_does_not_allocate() {
        let r = run(
            "event: content_block_start\ndata: {\"index\":18446744073709551615,\"content_block\":{\"type\":\"text\"}}\n\n",
        );
        assert_eq!(r.unknown_events, ["content_block_start:bad_index"]);
    }

    #[test]
    fn events_before_message_start_are_marked() {
        let body = "event: content_block_start\ndata: {\"index\":0,\"content_block\":{}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{}}\n\n\
event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"x\"}}\n\n";
        let r = rebuild_message(Some(STREAM), body);
        assert_eq!(
            r.unknown_events,
            [
                "content_block_start:before_message_start",
                "content_block_delta:before_message_start",
                "message_delta:before_message_start"
            ]
        );
        assert_eq!(r.message, None);
    }

    #[test]
    fn bad_tool_input_json_is_kept_as_text_and_marked() {
        let r = rebuild_message(
            Some(STREAM),
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_t\"}}\n\n\
event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"input\":{}}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":\"}}\n\n\
event: content_block_stop\ndata: {\"index\":0}\n\n",
        );
        assert_eq!(r.unknown_events, ["content_block_stop:bad_json"]);
        assert_eq!(
            r.message.expect("message")["content"][0]["input"],
            json!("{\"a\":")
        );
    }

    #[test]
    fn truncated_stream_keeps_partial_input() {
        let r = rebuild_message(
            Some(STREAM),
            "event: message_start\ndata: {\"message\":{\"id\":\"msg_t\"}}\n\n\
event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"input\":{}}}\n\n\
event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pa\"}}\n\n",
        );
        assert_eq!(r.unknown_events, ["truncated_input_json"]);
        assert_eq!(
            r.message.expect("message")["content"][0]["input"],
            json!("{\"pa")
        );
    }

    #[test]
    fn multi_line_data_is_joined_with_a_newline() {
        // JSON allows a newline between tokens, so a split object still parses.
        let r = rebuild_message(
            Some(STREAM),
            "event: message_start\ndata: {\"message\":\ndata: {\"id\":\"msg_multi\"}}\n\n",
        );
        assert_eq!(r.message_id.as_deref(), Some("msg_multi"));
        assert!(r.unknown_events.is_empty());
    }
}
