//! Turns Claude Code transcript lines into trace events, one line at a time.
//!
//! The parser is incremental: each call to [`Parser::push_line`] returns the
//! events that line produced. A node that changes (for example a tool call
//! whose result arrives) is sent again with the same id, as the schema allows.

use std::collections::HashMap;

use serde_json::{Map, Value};
use trace_core::{
    ContentBlock, Marker, ModelCall, Node, RawSource, Run, StopReason, ToolCall, ToolResult,
    TraceEvent, Turn, Usage,
};

use crate::time::parse_rfc3339_ms;

/// Harness name written on every `Run`.
pub const HARNESS: &str = "claude-code";

/// Top-level line types that carry nothing for the trace and are ignored on
/// purpose. Anything not handled and not listed here is reported as skipped.
const IGNORED_TYPES: &[&str] = &[
    "mode",
    "permission-mode",
    "last-prompt",
    "file-history-snapshot",
    "file-history-delta",
    "atis-latch",
    "queue-operation",
    "pr-link",
    "cost-state",
    "bridge-session",
];

/// A line the parser could not use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Source name, as in [`RawSource::source`].
    pub source: String,
    /// 1-based line number.
    pub line: u64,
    /// Why the line was skipped.
    pub reason: String,
}

/// A subagent started by a tool call, found in a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentLink {
    /// Claude Code's agent id; the transcript is `agent-<id>.jsonl`.
    pub agent_id: String,
    /// Trace id of the tool call that started it.
    pub tool_node_id: String,
}

/// Which kind of local (non-model) user action a marker came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalKind {
    SlashCommand,
    Shell,
}

/// Incremental parser for one transcript file (a session or a subagent).
pub struct Parser {
    source: String,
    run_id: String,
    is_subagent: bool,
    nodes: HashMap<String, TraceEvent>,
    run_sent: bool,
    current_turn: Option<String>,
    seen_prompt: bool,
    last_local_marker: Option<String>,
    chain: HashMap<String, ChainLink>,
    last_timestamp: Option<i64>,
    subagents: Vec<SubagentLink>,
    skipped: Vec<Skipped>,
}

impl Parser {
    /// A parser for a main session transcript. `source` names the file in
    /// raw records; `session_id` is the transcript's file stem.
    pub fn for_session(source: &str, session_id: &str) -> Self {
        Self::new(source, format!("run:{session_id}"), None, false)
    }

    /// A parser for a subagent transcript, nested under the tool call that
    /// started it.
    pub fn for_subagent(source: &str, link: &SubagentLink, title: Option<String>) -> Self {
        let mut parser = Self::new(
            source,
            format!("run:agent-{}", link.agent_id),
            Some(link.tool_node_id.clone()),
            true,
        );
        if let Some(Node::Run(run)) = parser.nodes.get_mut(&parser.run_id).map(|e| &mut e.node) {
            run.title = title;
        }
        parser
    }

    fn new(source: &str, run_id: String, parent_id: Option<String>, is_subagent: bool) -> Self {
        let run = TraceEvent {
            id: run_id.clone(),
            parent_id,
            node: Node::Run(Run {
                harness: HARNESS.to_string(),
                title: None,
            }),
            started_at_ms: None,
            ended_at_ms: None,
            raw: Vec::new(),
            metadata: Default::default(),
        };
        let mut nodes = HashMap::new();
        nodes.insert(run_id.clone(), run);
        Self {
            source: source.to_string(),
            run_id,
            is_subagent,
            nodes,
            run_sent: false,
            current_turn: None,
            seen_prompt: false,
            last_local_marker: None,
            chain: HashMap::new(),
            last_timestamp: None,
            subagents: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Subagents found so far.
    pub fn subagents(&self) -> &[SubagentLink] {
        &self.subagents
    }

    /// Lines skipped so far.
    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// Takes the skipped-line list, leaving it empty.
    pub fn take_skipped(&mut self) -> Vec<Skipped> {
        std::mem::take(&mut self.skipped)
    }

    /// Call once the whole file has been read. Sends the `Run` again with its
    /// end time set to the last timestamp seen.
    pub fn finish(&mut self) -> Vec<TraceEvent> {
        let mut out = Vec::new();
        self.ensure_run(&mut out, None);
        let last = self.last_timestamp;
        if let Some(run) = self.nodes.get_mut(&self.run_id) {
            run.ended_at_ms = last;
            out.push(run.clone());
        }
        out
    }

    /// Parses one complete line. `line_no` is 1-based.
    pub fn push_line(&mut self, line_no: u64, text: &str) -> Vec<TraceEvent> {
        let mut out = Vec::new();
        if text.trim().is_empty() {
            return out;
        }
        let value: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(err) => {
                self.skip(line_no, format!("invalid JSON: {err}"));
                return out;
            }
        };
        let Some(obj) = value.as_object() else {
            self.skip(line_no, "line is not a JSON object".to_string());
            return out;
        };
        let raw = RawSource {
            source: self.source.clone(),
            line: line_no,
            text: text.to_string(),
        };
        let line = Line {
            obj,
            raw,
            ts: obj
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_ms),
        };
        if line.ts.is_some() {
            self.last_timestamp = line.ts;
        }
        if let Some(uuid) = str_field(obj, "uuid") {
            let link = ChainLink {
                ts: line.ts,
                parent: str_field(obj, "parentUuid").map(str::to_string),
                is_attachment: str_field(obj, "type") == Some("attachment"),
            };
            self.chain.insert(uuid.to_string(), link);
        }
        self.ensure_run(&mut out, line.ts);

        let kind = str_field(obj, "type").unwrap_or("");
        match kind {
            "user" => self.on_user(&line, &mut out),
            "assistant" => self.on_assistant(&line, &mut out),
            "system" => self.on_system(&line, &mut out),
            "attachment" => self.on_attachment(&line, &mut out),
            "ai-title" => self.on_title(&line, &mut out),
            "agent-name" => self.run_metadata(&line, "agentName", "agent_name", &mut out),
            "agent-setting" => self.run_metadata(&line, "agentSetting", "agent_setting", &mut out),
            "continued-in" => self.run_metadata(
                &line,
                "continuedInSessionId",
                "continued_in_session",
                &mut out,
            ),
            k if IGNORED_TYPES.contains(&k) => {}
            "" => self.skip(line_no, "line has no type".to_string()),
            other => self.skip(line_no, format!("unknown line type {other:?}")),
        }
        out
    }

    // ----- line handlers -----

    fn on_user(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        let Some(message) = line.obj.get("message") else {
            self.skip(line.raw.line, "user line without message".to_string());
            return;
        };
        let content = message.get("content");

        // Tool results come back as user lines.
        if let Some(blocks) = content.and_then(Value::as_array) {
            let results: Vec<&Value> = blocks
                .iter()
                .filter(|b| str_at(b, "type") == Some("tool_result"))
                .collect();
            if !results.is_empty() {
                for block in results {
                    self.on_tool_result(line, block, out);
                }
                return;
            }
        }

        let text = match content {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };

        // Local actions typed by the user that never reach the model.
        if let Some(tag) = leading_tag(text) {
            match tag {
                "command-name" => {
                    return self.local_marker(line, LocalKind::SlashCommand, text, out);
                }
                "bash-input" => return self.local_marker(line, LocalKind::Shell, text, out),
                "local-command-stdout" | "local-command-stderr" | "bash-stdout" | "bash-stderr" => {
                    return self.extend_local_marker(line, out);
                }
                "local-command-caveat" => return,
                _ => {}
            }
        }

        let is_meta = line.obj.get("isMeta").and_then(Value::as_bool) == Some(true);
        // A subagent's task arrives as a meta line; it is still the prompt.
        let is_subagent_task = self.is_subagent && !self.seen_prompt;
        if is_meta && !is_subagent_task {
            return;
        }
        self.start_turn(line, content, out);
    }

    fn start_turn(&mut self, line: &Line, content: Option<&Value>, out: &mut Vec<TraceEvent>) {
        let id = format!("turn:{}", self.line_id(line));
        let mut metadata = Map::new();
        if let Some(kind) = line.obj.get("origin").and_then(|o| str_at(o, "kind")) {
            metadata.insert("origin".into(), Value::String(kind.to_string()));
        }
        if let Some(source) = str_field(line.obj, "promptSource") {
            metadata.insert("prompt_source".into(), Value::String(source.to_string()));
        }
        let event = TraceEvent {
            id: id.clone(),
            parent_id: Some(self.run_id.clone()),
            node: Node::Turn(Turn {
                prompt: content.map(content_blocks).unwrap_or_default(),
            }),
            started_at_ms: line.ts,
            ended_at_ms: None,
            raw: vec![line.raw.clone()],
            metadata: metadata.into_iter().collect(),
        };
        self.seen_prompt = true;
        self.current_turn = Some(id);
        self.last_local_marker = None;
        self.emit(event, out);
    }

    fn on_tool_result(&mut self, line: &Line, block: &Value, out: &mut Vec<TraceEvent>) {
        let Some(tool_use_id) = str_at(block, "tool_use_id") else {
            self.skip(line.raw.line, "tool_result without tool_use_id".to_string());
            return;
        };
        let node_id = format!("tool:{tool_use_id}");
        let Some(mut event) = self.nodes.get(&node_id).cloned() else {
            self.skip(
                line.raw.line,
                format!("tool_result for unknown tool use {tool_use_id}"),
            );
            return;
        };
        let Node::ToolCall(call) = &mut event.node else {
            return;
        };
        call.result = Some(ToolResult {
            content: block.get("content").map(content_blocks).unwrap_or_default(),
            is_error: block.get("is_error").and_then(Value::as_bool) == Some(true),
        });
        event.ended_at_ms = line.ts;
        event.raw.push(line.raw.clone());

        // A tool that started a subagent reports its agent id here.
        let rich = line.obj.get("toolUseResult");
        if let Some(agent_id) = rich.and_then(|r| str_at(r, "agentId")) {
            event
                .metadata
                .insert("agent_id".into(), Value::String(agent_id.to_string()));
            if let Some(status) = rich.and_then(|r| str_at(r, "status")) {
                event
                    .metadata
                    .insert("agent_status".into(), Value::String(status.to_string()));
            }
            self.subagents.push(SubagentLink {
                agent_id: agent_id.to_string(),
                tool_node_id: node_id,
            });
        }
        self.emit(event, out);
    }

    fn on_assistant(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        let Some(message) = line.obj.get("message") else {
            self.skip(line.raw.line, "assistant line without message".to_string());
            return;
        };
        // One API response is spread over several lines sharing message.id.
        let Some(message_id) = str_at(message, "id") else {
            self.skip(
                line.raw.line,
                "assistant line without message id".to_string(),
            );
            return;
        };
        let model_id = format!("model:{message_id}");
        let blocks: &[Value] = message
            .get("content")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let mut event = match self.nodes.get(&model_id).cloned() {
            Some(existing) => existing,
            None => self.new_model_call(line, &model_id, message),
        };
        if let Node::ModelCall(call) = &mut event.node {
            for block in blocks {
                if str_at(block, "type") != Some("tool_use") {
                    call.output.push(content_block(block));
                }
            }
            if let Some(reason) = str_at(message, "stop_reason") {
                call.stop_reason = Some(stop_reason(reason));
            }
            // Usage is repeated on every line and can be a mid-stream
            // snapshot, so keep the largest output count seen.
            if let Some(usage) = message.get("usage").map(usage_from) {
                let better = match call.usage {
                    Some(old) => usage.output_tokens >= old.output_tokens,
                    None => true,
                };
                if better {
                    call.usage = Some(usage);
                }
            }
        }
        if !event.raw.iter().any(|r| r.line == line.raw.line) {
            event.raw.push(line.raw.clone());
        }
        event.ended_at_ms = line.ts.or(event.ended_at_ms);
        self.emit(event, out);

        for block in blocks {
            if str_at(block, "type") == Some("tool_use") {
                self.new_tool_call(line, &model_id, block, out);
            }
        }
    }

    fn new_model_call(&self, line: &Line, model_id: &str, message: &Value) -> TraceEvent {
        let started = str_field(line.obj, "parentUuid")
            .and_then(|p| self.request_start(p))
            .or(line.ts);
        let mut metadata = Map::new();
        if let Some(request_id) = str_field(line.obj, "requestId") {
            metadata.insert("request_id".into(), Value::String(request_id.to_string()));
        }
        TraceEvent {
            id: model_id.to_string(),
            parent_id: Some(self.turn_or_run()),
            node: Node::ModelCall(ModelCall {
                model: str_at(message, "model").map(str::to_string),
                output: Vec::new(),
                stop_reason: None,
                usage: None,
            }),
            started_at_ms: started,
            ended_at_ms: line.ts,
            raw: Vec::new(),
            metadata: metadata.into_iter().collect(),
        }
    }

    fn new_tool_call(
        &mut self,
        line: &Line,
        model_id: &str,
        block: &Value,
        out: &mut Vec<TraceEvent>,
    ) {
        let Some(tool_use_id) = str_at(block, "id") else {
            self.skip(line.raw.line, "tool_use without id".to_string());
            return;
        };
        let event = TraceEvent {
            id: format!("tool:{tool_use_id}"),
            parent_id: Some(model_id.to_string()),
            node: Node::ToolCall(ToolCall {
                name: str_at(block, "name").unwrap_or("unknown").to_string(),
                input: block.get("input").cloned().unwrap_or(Value::Null),
                result: None,
            }),
            started_at_ms: line.ts,
            ended_at_ms: None,
            raw: vec![line.raw.clone()],
            metadata: Default::default(),
        };
        self.emit(event, out);
    }

    fn on_system(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        match str_field(line.obj, "subtype") {
            Some("turn_duration") => {
                let Some(turn_id) = self.current_turn.clone() else {
                    return;
                };
                let Some(mut turn) = self.nodes.get(&turn_id).cloned() else {
                    return;
                };
                turn.ended_at_ms = line.ts;
                if let Some(ms) = line.obj.get("durationMs").cloned() {
                    turn.metadata.insert("duration_ms".into(), ms);
                }
                turn.raw.push(line.raw.clone());
                self.emit(turn, out);
            }
            Some("stop_hook_summary") => {
                let count = line
                    .obj
                    .get("hookCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let mut metadata = Map::new();
                for key in ["hookCount", "preventedContinuation", "hasOutput"] {
                    if let Some(v) = line.obj.get(key) {
                        metadata.insert(key.into(), v.clone());
                    }
                }
                let summary = format!("Stop hooks ran ({count})");
                self.marker(line, "hook", summary, metadata, out);
            }
            Some("local_command") => {
                let text = str_field(line.obj, "content").unwrap_or("");
                match leading_tag(text) {
                    Some("command-name") => {
                        self.local_marker(line, LocalKind::SlashCommand, text, out)
                    }
                    _ => self.extend_local_marker(line, out),
                }
            }
            Some("informational") => {
                let text = str_field(line.obj, "content").unwrap_or("");
                let mut metadata = Map::new();
                if let Some(level) = line.obj.get("level") {
                    metadata.insert("level".into(), level.clone());
                }
                self.marker(line, "info", text.to_string(), metadata, out);
            }
            // A recap shown to the user on returning to a session, not a step.
            Some("away_summary") => {}
            Some(other) => self.skip(line.raw.line, format!("unknown system subtype {other:?}")),
            None => self.skip(line.raw.line, "system line without subtype".to_string()),
        }
    }

    fn on_attachment(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        let Some(attachment) = line.obj.get("attachment") else {
            return;
        };
        let kind = str_at(attachment, "type").unwrap_or("");
        // Most attachments are context injected into the prompt and are not
        // steps in the run. Hook results are steps, so they become markers.
        if !kind.starts_with("hook_") {
            return;
        }
        let event = str_at(attachment, "hookEvent").unwrap_or("unknown");
        let mut metadata = Map::new();
        metadata.insert("hook_event".into(), Value::String(event.to_string()));
        metadata.insert("attachment_type".into(), Value::String(kind.to_string()));
        if let Some(tool_use_id) = str_at(attachment, "toolUseID") {
            metadata.insert("tool_use_id".into(), Value::String(tool_use_id.to_string()));
        }
        let outcome = kind.trim_start_matches("hook_").replace('_', " ");
        self.marker(
            line,
            "hook",
            format!("{event} hook: {outcome}"),
            metadata,
            out,
        );
    }

    /// Copies a session-level string field onto the Run's metadata.
    fn run_metadata(&mut self, line: &Line, field: &str, key: &str, out: &mut Vec<TraceEvent>) {
        let Some(value) = line.obj.get(field).cloned() else {
            return;
        };
        let Some(mut run) = self.nodes.get(&self.run_id).cloned() else {
            return;
        };
        if run.metadata.get(key) == Some(&value) {
            return;
        }
        run.metadata.insert(key.to_string(), value);
        run.raw.push(line.raw.clone());
        self.emit(run, out);
    }

    fn on_title(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        let Some(title) = str_field(line.obj, "aiTitle") else {
            return;
        };
        let Some(mut run) = self.nodes.get(&self.run_id).cloned() else {
            return;
        };
        if let Node::Run(r) = &mut run.node {
            if r.title.as_deref() == Some(title) {
                return;
            }
            r.title = Some(title.to_string());
        }
        self.emit(run, out);
    }

    /// When the request for a response was sent: the time of the line that
    /// triggered it (a prompt or a tool result). Attachment lines are written
    /// when the response arrives, not when the request goes out, so they are
    /// stepped over.
    fn request_start(&self, parent_uuid: &str) -> Option<i64> {
        let mut uuid = parent_uuid;
        // Bounded walk, in case a malformed transcript has a cycle.
        for _ in 0..64 {
            let link = self.chain.get(uuid)?;
            if !link.is_attachment {
                return link.ts;
            }
            uuid = link.parent.as_deref()?;
        }
        None
    }

    // ----- markers -----

    fn marker(
        &mut self,
        line: &Line,
        kind: &str,
        summary: String,
        metadata: Map<String, Value>,
        out: &mut Vec<TraceEvent>,
    ) {
        let event = TraceEvent {
            id: format!("marker:{}", self.line_id(line)),
            parent_id: Some(self.turn_or_run()),
            node: Node::Marker(Marker {
                kind: kind.to_string(),
                summary,
            }),
            started_at_ms: line.ts,
            ended_at_ms: None,
            raw: vec![line.raw.clone()],
            metadata: metadata.into_iter().collect(),
        };
        self.emit(event, out);
    }

    fn local_marker(
        &mut self,
        line: &Line,
        kind: LocalKind,
        text: &str,
        out: &mut Vec<TraceEvent>,
    ) {
        let (marker_kind, label) = match kind {
            LocalKind::SlashCommand => ("local_command", "Local command"),
            LocalKind::Shell => ("user_shell", "User shell command"),
        };
        let inner = tag_inner(text).unwrap_or_default();
        let summary = if inner.is_empty() {
            label.to_string()
        } else {
            format!("{label}: {inner}")
        };
        self.marker(line, marker_kind, summary, Map::new(), out);
        self.last_local_marker = Some(format!("marker:{}", self.line_id(line)));
    }

    fn extend_local_marker(&mut self, line: &Line, out: &mut Vec<TraceEvent>) {
        let Some(id) = self.last_local_marker.clone() else {
            self.skip(
                line.raw.line,
                "command output without a command".to_string(),
            );
            return;
        };
        if let Some(mut event) = self.nodes.get(&id).cloned() {
            event.raw.push(line.raw.clone());
            event.ended_at_ms = line.ts;
            self.emit(event, out);
        }
    }

    // ----- helpers -----

    fn ensure_run(&mut self, out: &mut Vec<TraceEvent>, ts: Option<i64>) {
        let Some(run) = self.nodes.get_mut(&self.run_id) else {
            return;
        };
        let needs_time = run.started_at_ms.is_none() && ts.is_some();
        if needs_time {
            run.started_at_ms = ts;
        }
        if !self.run_sent || needs_time {
            self.run_sent = true;
            out.push(run.clone());
        }
    }

    fn turn_or_run(&self) -> String {
        self.current_turn
            .clone()
            .unwrap_or_else(|| self.run_id.clone())
    }

    /// A stable id for a line: its uuid, or its position if it has none.
    fn line_id(&self, line: &Line) -> String {
        match str_field(line.obj, "uuid") {
            Some(uuid) => uuid.to_string(),
            None => format!("{}:{}", self.source, line.raw.line),
        }
    }

    fn emit(&mut self, event: TraceEvent, out: &mut Vec<TraceEvent>) {
        self.nodes.insert(event.id.clone(), event.clone());
        out.push(event);
    }

    fn skip(&mut self, line: u64, reason: String) {
        tracing::warn!(source = %self.source, line, %reason, "skipped transcript line");
        self.skipped.push(Skipped {
            source: self.source.clone(),
            line,
            reason,
        });
    }
}

/// What the parser remembers about each line for following the uuid chain.
struct ChainLink {
    ts: Option<i64>,
    parent: Option<String>,
    is_attachment: bool,
}

/// One parsed line with its raw text and timestamp.
struct Line<'a> {
    obj: &'a Map<String, Value>,
    raw: RawSource,
    ts: Option<i64>,
}

fn str_field<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Converts message content (a string or a list of blocks) to content blocks.
fn content_blocks(content: &Value) -> Vec<ContentBlock> {
    match content {
        Value::String(s) => vec![ContentBlock::Text { text: s.clone() }],
        Value::Array(blocks) => blocks.iter().map(content_block).collect(),
        _ => Vec::new(),
    }
}

fn content_block(block: &Value) -> ContentBlock {
    match str_at(block, "type") {
        Some("text") => ContentBlock::Text {
            text: str_at(block, "text").unwrap_or_default().to_string(),
        },
        Some("thinking") => ContentBlock::Thinking {
            text: str_at(block, "thinking").unwrap_or_default().to_string(),
        },
        Some("image") => ContentBlock::Image {
            media_type: block
                .get("source")
                .and_then(|s| str_at(s, "media_type"))
                .map(str::to_string),
        },
        Some(other) => ContentBlock::Other {
            kind: other.to_string(),
        },
        None => ContentBlock::Other {
            kind: "unknown".to_string(),
        },
    }
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        other => StopReason::Other(other.to_string()),
    }
}

fn usage_from(usage: &Value) -> Usage {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64);
    Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        cache_read_tokens: count("cache_read_input_tokens"),
        cache_write_tokens: count("cache_creation_input_tokens"),
    }
}

/// The tag name if `text` starts with `<tag>`.
fn leading_tag(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('<')?;
    let end = rest.find('>')?;
    let tag = &rest[..end];
    tag.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
        .then_some(tag)
}

/// The text inside the leading `<tag>...</tag>`, trimmed.
fn tag_inner(text: &str) -> Option<String> {
    let tag = leading_tag(text)?;
    let start = tag.len() + 2;
    let close = format!("</{tag}>");
    let end = text.find(&close)?;
    text.get(start..end).map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_lines(lines: &[&str]) -> (Vec<TraceEvent>, Vec<Skipped>) {
        let mut parser = Parser::for_session("test.jsonl", "s1");
        let mut events = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            events.extend(parser.push_line(i as u64 + 1, line));
        }
        events.extend(parser.finish());
        (events, parser.take_skipped())
    }

    #[test]
    fn bad_lines_are_skipped_not_fatal() {
        let (events, skipped) = run_lines(&[
            "not json",
            "[1,2,3]",
            r#"{"no_type":true}"#,
            r#"{"type":"brand-new-thing"}"#,
            r#"{"type":"system","subtype":"brand-new-subtype"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x"}]}}"#,
            r#"{"type":"assistant","message":{}}"#,
            r#"{"type":"mode","mode":"normal"}"#,
        ]);
        assert_eq!(skipped.len(), 7, "{skipped:#?}");
        // Only the Run itself (sent first, then again by finish).
        assert!(events.iter().all(|e| e.id == "run:s1"));
    }

    #[test]
    fn user_shell_command_becomes_marker_with_output() {
        let (events, skipped) = run_lines(&[
            r#"{"type":"user","uuid":"u1","message":{"content":"<bash-input>code .</bash-input>"}}"#,
            r#"{"type":"user","uuid":"u2","message":{"content":"<bash-stdout></bash-stdout><bash-stderr></bash-stderr>"}}"#,
        ]);
        assert!(skipped.is_empty(), "{skipped:#?}");
        let last = events
            .iter()
            .rev()
            .find(|e| e.id == "marker:u1")
            .expect("marker");
        assert_eq!(last.raw.len(), 2);
        let Node::Marker(m) = &last.node else {
            panic!("not a marker")
        };
        assert_eq!(m.kind, "user_shell");
        assert_eq!(m.summary, "User shell command: code .");
    }

    #[test]
    fn session_level_lines_and_system_notices() {
        let (events, skipped) = run_lines(&[
            r#"{"type":"agent-name","agentName":"example-agent","sessionId":"s1"}"#,
            r#"{"type":"continued-in","continuedInSessionId":"s2","sessionId":"s1"}"#,
            r#"{"type":"system","subtype":"local_command","uuid":"u1","content":"<command-name>/model</command-name>"}"#,
            r#"{"type":"system","subtype":"local_command","uuid":"u2","content":"<local-command-stdout>Set model</local-command-stdout>"}"#,
            r#"{"type":"system","subtype":"informational","uuid":"u3","level":"warning","content":"Example warning"}"#,
            r#"{"type":"system","subtype":"away_summary","uuid":"u4","content":"Example recap"}"#,
        ]);
        assert!(skipped.is_empty(), "{skipped:#?}");
        let run = events.iter().rev().find(|e| e.id == "run:s1").expect("run");
        assert_eq!(run.metadata["agent_name"], "example-agent");
        assert_eq!(run.metadata["continued_in_session"], "s2");

        let cmd = events
            .iter()
            .rev()
            .find(|e| e.id == "marker:u1")
            .expect("command");
        assert_eq!(cmd.raw.len(), 2, "output line joins the command marker");
        let info = events
            .iter()
            .rev()
            .find(|e| e.id == "marker:u3")
            .expect("info");
        let Node::Marker(m) = &info.node else {
            panic!("not a marker")
        };
        assert_eq!(
            (m.kind.as_str(), m.summary.as_str()),
            ("info", "Example warning")
        );
        assert!(!events.iter().any(|e| e.id == "marker:u4"));
    }

    #[test]
    fn tag_helpers() {
        assert_eq!(
            leading_tag("<command-name>/model</command-name>"),
            Some("command-name")
        );
        assert_eq!(leading_tag("plain"), None);
        assert_eq!(leading_tag("< not a tag>"), None);
        assert_eq!(
            tag_inner("<command-name>/model</command-name>").as_deref(),
            Some("/model")
        );
    }
}
