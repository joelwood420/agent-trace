//! Writes the invented capture fixture `fixtures/captures/basic/calls.jsonl`
//! for the sanitised sample session in `fixtures/claude-code/basic`.
//!
//! ```powershell
//! cargo run -p capture --example make_fixture
//! ```
//!
//! Every record is built from the sample session's trace (model call ids,
//! models, timing, and the already sanitised prompts, outputs and tool
//! results) plus invented system prompts, tools and headers. Nothing is read
//! from a real `.claude` folder.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use adapter_claude_code::{SESSION_HEADER, load_session};
use capture_core::{
    Body, CaptureRecord, CapturedRequest, CapturedResponse, Header, rebuild_message,
};
use serde_json::{Value, json};
use trace_core::{ContentBlock, ModelCall, Node, StopReason, Trace, TraceEvent};
use trace_view::model_calls_by_run;

const SESSION_ID: &str = "00000000-0000-4000-8000-000000000002";
const STREAM_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8";
const JSON_CONTENT_TYPE: &str = "application/json";
const FORBIDDEN: [&str; 3] = ["Users", "AppData", "Bearer"];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let transcript = root.join(format!("fixtures/claude-code/basic/{SESSION_ID}.jsonl"));
    let session = load_session(&transcript).expect("load the sample session");
    let mut trace = Trace::new();
    for event in session.events {
        trace.apply(event).expect("fixture events form a tree");
    }

    let mut records = Vec::new();
    for (run_index, (run_id, _)) in model_calls_by_run(&trace).iter().enumerate() {
        let is_main = run_index == 0;
        let steps = run_steps(&trace, run_id);
        let mut call_index = 0;
        for (position, step) in steps.iter().enumerate() {
            let Step::Call(call) = step else { continue };
            let tools = if is_main && call_index >= 3 {
                tools_with_web_fetch()
            } else {
                base_tools()
            };
            let system = if is_main {
                main_system()
            } else {
                subagent_system()
            };
            records.push(model_call_record(
                &trace,
                call,
                system,
                tools,
                &steps[..position],
            ));
            call_index += 1;
        }
    }
    records.push(title_record(&trace));
    records.sort_by_key(|r| r.started_at_ms);
    for (n, record) in records.iter_mut().enumerate() {
        record.id = format!("fixture-{:03}", n + 1);
        if let Some(response) = record.response.as_mut() {
            response
                .headers
                .push(header("request-id", &format!("req_test_{}", n + 1)));
        }
    }

    let mut out = String::new();
    for record in &records {
        check_rebuilds(record);
        out.push_str(&serde_json::to_string(record).expect("serialise a record"));
        out.push('\n');
    }
    for word in FORBIDDEN {
        assert!(!out.contains(word), "the fixture must not contain {word:?}");
    }
    let target = root.join("fixtures/captures/basic/calls.jsonl");
    fs::create_dir_all(target.parent().expect("a parent folder")).expect("create the folder");
    fs::write(&target, &out).expect("write the fixture");
    println!("wrote {} records ({} bytes)", records.len(), out.len());
}

/// One step of a run's conversation: a prompt, or a model call (with its tool calls).
enum Step<'a> {
    Prompt(&'a TraceEvent),
    Call(&'a TraceEvent),
}

/// The run's prompts and model calls in order, not descending into subagents.
fn run_steps<'a>(trace: &'a Trace, run_id: &str) -> Vec<Step<'a>> {
    let mut steps = Vec::new();
    for turn in trace.children(run_id) {
        if !matches!(turn.node, Node::Turn(_)) {
            continue;
        }
        steps.push(Step::Prompt(turn));
        for child in trace.children(&turn.id) {
            if matches!(child.node, Node::ModelCall(_)) {
                steps.push(Step::Call(child));
            }
        }
    }
    steps
}

fn model_call(event: &TraceEvent) -> &ModelCall {
    match &event.node {
        Node::ModelCall(call) => call,
        _ => panic!("{} is not a model call", event.id),
    }
}

fn message_id(event: &TraceEvent) -> String {
    event
        .id
        .strip_prefix("model:")
        .expect("model call ids start with model:")
        .to_string()
}

fn texts(blocks: &[ContentBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Content blocks of a model call's reply: its text, then one `tool_use` per tool call.
fn reply_content(trace: &Trace, call: &TraceEvent) -> Vec<Value> {
    let mut content: Vec<Value> = texts(&model_call(call).output)
        .into_iter()
        .map(|text| json!({ "type": "text", "text": text }))
        .collect();
    for child in trace.children(&call.id) {
        if let Node::ToolCall(tool) = &child.node {
            content.push(json!({
                "type": "tool_use",
                "id": tool_use_id(child),
                "name": tool.name,
                "input": tool.input,
            }));
        }
    }
    content
}

fn tool_use_id(event: &TraceEvent) -> String {
    event
        .id
        .strip_prefix("tool:")
        .expect("tool call ids start with tool:")
        .to_string()
}

/// The `tool_result` blocks for a model call's finished tool calls.
fn tool_results(trace: &Trace, call: &TraceEvent) -> Vec<Value> {
    let mut results = Vec::new();
    for child in trace.children(&call.id) {
        let Node::ToolCall(tool) = &child.node else {
            continue;
        };
        let Some(result) = &tool.result else { continue };
        results.push(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id(child),
            "content": texts(&result.content).join("\n"),
            "is_error": result.is_error,
        }));
    }
    results
}

/// The request's `messages`: every step before this call, as a conversation.
fn messages(trace: &Trace, earlier: &[Step]) -> Vec<Value> {
    let mut out = Vec::new();
    for step in earlier {
        match step {
            Step::Prompt(turn) => {
                let Node::Turn(t) = &turn.node else { continue };
                let mut text = texts(&t.prompt).join("\n");
                if text.is_empty() {
                    text = "(empty prompt)".to_string();
                }
                out.push(json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": text }],
                }));
            }
            Step::Call(call) => {
                let content = reply_content(trace, call);
                if content.is_empty() {
                    continue;
                }
                out.push(json!({ "role": "assistant", "content": content }));
                let results = tool_results(trace, call);
                if !results.is_empty() {
                    out.push(json!({ "role": "user", "content": results }));
                }
            }
        }
    }
    // Like Claude Code, mark the newest message as the cache breakpoint.
    if let Some(block) = out
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|blocks| blocks.last_mut())
        .and_then(Value::as_object_mut)
    {
        block.insert("cache_control".into(), json!({ "type": "ephemeral" }));
    }
    out
}

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn request_headers() -> Vec<Header> {
    vec![
        header(SESSION_HEADER, SESSION_ID),
        header("anthropic-beta", "test-beta-1,test-beta-2"),
        header("user-agent", "test-harness/0.0.0"),
        header("content-type", JSON_CONTENT_TYPE),
    ]
}

fn model_call_record(
    trace: &Trace,
    call: &TraceEvent,
    system: Value,
    tools: Value,
    earlier: &[Step],
) -> CaptureRecord {
    let details = model_call(call);
    let model = details.model.clone().unwrap_or_else(|| "test-model".into());
    let body = json!({
        "model": model,
        "max_tokens": 32000,
        "system": system,
        "tools": tools,
        "messages": messages(trace, earlier),
        "stream": true,
    });
    let id = message_id(call);
    let stream = sse_stream(&id, &model, details, &reply_content(trace, call));
    let rebuilt = rebuild_message(Some(STREAM_CONTENT_TYPE), &stream);
    let started = call
        .started_at_ms
        .expect("fixture model calls have a start");
    CaptureRecord {
        id: String::new(),
        started_at_ms: started,
        first_byte_at_ms: Some(started + 800),
        ended_at_ms: Some(call.ended_at_ms.unwrap_or(started + 2000)),
        request: CapturedRequest {
            method: "POST".into(),
            path: "/v1/messages?beta=true".into(),
            headers: request_headers(),
            body: Body::Json(body),
        },
        response: Some(CapturedResponse {
            status: 200,
            headers: vec![header("content-type", STREAM_CONTENT_TYPE)],
            stream: Some(stream),
            message: rebuilt.message,
        }),
        message_id: Some(id),
        error: None,
    }
}

fn stop_reason(reason: Option<&StopReason>) -> Value {
    match reason {
        None => Value::Null,
        Some(StopReason::EndTurn) => json!("end_turn"),
        Some(StopReason::ToolUse) => json!("tool_use"),
        Some(StopReason::MaxTokens) => json!("max_tokens"),
        Some(StopReason::StopSequence) => json!("stop_sequence"),
        Some(StopReason::Other(other)) => json!(other),
    }
}

fn event(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

/// A server-sent event stream that rebuilds into the given reply.
fn sse_stream(id: &str, model: &str, call: &ModelCall, content: &[Value]) -> String {
    let usage = call.usage.unwrap_or_default();
    let mut out = event(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": usage.input_tokens.unwrap_or(0),
                    "cache_creation_input_tokens": usage.cache_write_tokens.unwrap_or(0),
                    "cache_read_input_tokens": usage.cache_read_tokens.unwrap_or(0),
                    "output_tokens": 1,
                },
            },
        }),
    );
    out.push_str(&event("ping", json!({ "type": "ping" })));
    for (index, block) in content.iter().enumerate() {
        let (start, delta) = if block["type"] == "tool_use" {
            (
                json!({ "type": "tool_use", "id": block["id"], "name": block["name"], "input": {} }),
                json!({ "type": "input_json_delta", "partial_json": block["input"].to_string() }),
            )
        } else {
            (
                json!({ "type": "text", "text": "" }),
                json!({ "type": "text_delta", "text": block["text"] }),
            )
        };
        out.push_str(&event(
            "content_block_start",
            json!({ "type": "content_block_start", "index": index, "content_block": start }),
        ));
        out.push_str(&event(
            "content_block_delta",
            json!({ "type": "content_block_delta", "index": index, "delta": delta }),
        ));
        out.push_str(&event(
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": index }),
        ));
    }
    out.push_str(&event(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason(call.stop_reason.as_ref()), "stop_sequence": null },
            "usage": { "output_tokens": usage.output_tokens.unwrap_or(0) },
        }),
    ));
    out.push_str(&event("message_stop", json!({ "type": "message_stop" })));
    out
}

/// An invented title-generation call with no model call in the trace.
fn title_record(trace: &Trace) -> CaptureRecord {
    let first_turn = trace
        .roots()
        .flat_map(|run| trace.children(&run.id))
        .find(|e| matches!(e.node, Node::Turn(_)))
        .expect("the session has a prompt");
    let started = first_turn.started_at_ms.expect("prompts have a time") + 200;
    let body = json!({
        "model": "test-small-model",
        "max_tokens": 512,
        "messages": [{
            "role": "user",
            "content": "Write a title of at most five words for this request: List the PowerShell scripts in this folder.",
        }],
    });
    let reply = json!({
        "id": "msg_title_test",
        "type": "message",
        "role": "assistant",
        "model": "test-small-model",
        "content": [{ "type": "text", "text": "Listing PowerShell scripts" }],
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": { "input_tokens": 41, "output_tokens": 6 },
    });
    let rebuilt = rebuild_message(Some(JSON_CONTENT_TYPE), &reply.to_string());
    CaptureRecord {
        id: String::new(),
        started_at_ms: started,
        first_byte_at_ms: Some(started + 400),
        ended_at_ms: Some(started + 450),
        request: CapturedRequest {
            method: "POST".into(),
            path: "/v1/messages?beta=true".into(),
            headers: request_headers(),
            body: Body::Json(body),
        },
        response: Some(CapturedResponse {
            status: 200,
            headers: vec![header("content-type", JSON_CONTENT_TYPE)],
            stream: None,
            message: rebuilt.message,
        }),
        message_id: rebuilt.message_id,
        error: None,
    }
}

/// Every record's response must rebuild to its own message id.
fn check_rebuilds(record: &CaptureRecord) {
    let response = record
        .response
        .as_ref()
        .expect("every record has a response");
    let content_type = response
        .headers
        .iter()
        .find(|h| h.name == "content-type")
        .map(|h| h.value.as_str());
    let body = match &response.stream {
        Some(stream) => stream.clone(),
        None => response.message.as_ref().expect("a message").to_string(),
    };
    let rebuilt = rebuild_message(content_type, &body);
    assert!(
        rebuilt.unknown_events.is_empty(),
        "{:?}",
        rebuilt.unknown_events
    );
    assert_eq!(
        rebuilt.message_id, record.message_id,
        "record {}",
        record.id
    );
    assert_eq!(rebuilt.message, response.message, "record {}", record.id);
}

fn main_system() -> Value {
    json!([
        { "type": "text", "text": "test-harness system header: invented for the fixture." },
        {
            "type": "text",
            "text": "You are an invented coding assistant used to test Snitchcraft. Answer briefly. Use the tools to look at files before you answer, and run commands only when the user asks.",
            "cache_control": { "type": "ephemeral" },
        },
        {
            "type": "text",
            "text": "Environment: an invented project folder on an invented Windows machine. Shell: PowerShell. Today is an invented date.",
            "cache_control": { "type": "ephemeral" },
        },
    ])
}

fn subagent_system() -> Value {
    json!([
        { "type": "text", "text": "test-harness system header: invented for the fixture." },
        {
            "type": "text",
            "text": "You are an invented helper agent started by another agent. Do the one task you are given and report the result in a few lines.",
            "cache_control": { "type": "ephemeral" },
        },
        {
            "type": "text",
            "text": "Environment: an invented project folder on an invented Windows machine. Shell: PowerShell.",
            "cache_control": { "type": "ephemeral" },
        },
    ])
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    })
}

fn base_tools() -> Value {
    json!([
        tool(
            "Read",
            "Read an invented file and return its lines.",
            json!({ "file_path": { "type": "string", "description": "The file to read." } }),
            &["file_path"],
        ),
        tool(
            "Grep",
            "Search invented files for a pattern.",
            json!({
                "pattern": { "type": "string", "description": "The pattern to look for." },
                "path": { "type": "string", "description": "Where to search." },
            }),
            &["pattern"],
        ),
        tool(
            "PowerShell",
            "Run an invented PowerShell command.",
            json!({ "command": { "type": "string", "description": "The command to run." } }),
            &["command"],
        ),
        tool(
            "Bash",
            "Run an invented shell command.",
            json!({ "command": { "type": "string", "description": "The command to run." } }),
            &["command"],
        ),
        tool(
            "Skill",
            "Start an invented skill by name.",
            json!({ "skill": { "type": "string", "description": "The skill name." } }),
            &["skill"],
        ),
    ])
}

fn tools_with_web_fetch() -> Value {
    let mut tools = base_tools();
    if let Some(list) = tools.as_array_mut() {
        list.push(tool(
            "WebFetch",
            "Fetch an invented web page.",
            json!({ "url": { "type": "string", "description": "The page to fetch." } }),
            &["url"],
        ));
    }
    tools
}
