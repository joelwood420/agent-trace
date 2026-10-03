//! Helpers shared by the trace-view integration tests.

#![allow(dead_code, clippy::expect_used)]

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{Value, json};
use trace_core::{
    ContentBlock, Marker, ModelCall, Node, Run, StopReason, ToolCall, ToolResult, Trace,
    TraceEvent, Turn, Usage,
};
use trace_view::{DiagramNode, SessionDiagram};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

/// The hand-written schema example, parsed line by line with serde_json.
pub fn example_trace() -> Trace {
    let path = fixtures().join("trace-core").join("example-trace.jsonl");
    let text = std::fs::read_to_string(path).expect("read fixture");
    let mut trace = Trace::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let event: TraceEvent = serde_json::from_str(line).expect("parse event");
        trace.apply(event).expect("apply event");
    }
    trace
}

pub const CLAUDE_SESSION: &str = "00000000-0000-4000-8000-000000000002";

pub fn claude_session_path() -> PathBuf {
    fixtures()
        .join("claude-code")
        .join("basic")
        .join(format!("{CLAUDE_SESSION}.jsonl"))
}

/// The Claude Code fixture, loaded through the adapter.
pub fn claude_trace() -> Trace {
    let session = adapter_claude_code::load_session(&claude_session_path()).expect("load");
    let mut trace = Trace::new();
    for event in session.events {
        trace.apply(event).expect("apply event");
    }
    trace
}

/// Every box in the session diagram, depth first.
pub fn all_nodes(diagram: &SessionDiagram) -> Vec<&DiagramNode> {
    let mut out = Vec::new();
    for marker in &diagram.markers {
        collect(marker, &mut out);
    }
    for prompt in &diagram.prompts {
        collect(&prompt.root, &mut out);
    }
    out
}

fn collect<'a>(node: &'a DiagramNode, out: &mut Vec<&'a DiagramNode>) {
    out.push(node);
    for child in &node.children {
        collect(child, out);
    }
}

pub fn assert_ids_unique(diagram: &SessionDiagram) {
    let mut seen = HashSet::new();
    for node in all_nodes(diagram) {
        assert!(seen.insert(node.id.clone()), "duplicate id {}", node.id);
    }
}

pub fn kinds(nodes: &[DiagramNode]) -> Vec<&'static str> {
    nodes.iter().map(|n| n.kind.as_str()).collect()
}

pub fn ids(nodes: &[DiagramNode]) -> Vec<&str> {
    nodes.iter().map(|n| n.id.as_str()).collect()
}

/// Builds small traces by hand for rule tests.
pub struct TraceBuilder {
    pub trace: Trace,
    clock: i64,
}

impl TraceBuilder {
    /// A trace with run `run` and one turn `turn`.
    pub fn new() -> Self {
        let mut b = TraceBuilder {
            trace: Trace::new(),
            clock: 1_000_000,
        };
        b.add(
            "run",
            None,
            Node::Run(Run {
                harness: "test-harness".to_string(),
                title: Some("Test run".to_string()),
            }),
            None,
        );
        b.add(
            "turn",
            Some("run"),
            Node::Turn(Turn {
                prompt: vec![ContentBlock::Text {
                    text: "Do the thing".to_string(),
                }],
            }),
            None,
        );
        b
    }

    pub fn add(&mut self, id: &str, parent: Option<&str>, node: Node, length_ms: Option<i64>) {
        let start = self.clock;
        self.clock += 1_000;
        self.trace
            .apply(TraceEvent {
                id: id.to_string(),
                parent_id: parent.map(str::to_string),
                node,
                started_at_ms: Some(start),
                ended_at_ms: length_ms.map(|l| start + l),
                raw: Vec::new(),
                metadata: Default::default(),
            })
            .expect("apply");
    }

    /// A finished model call under `parent` with the given tool calls.
    /// Each tool is `(name, result)` where result is `Some(is_error)` or
    /// `None` for still running.
    pub fn model_call(&mut self, id: &str, parent: &str, tools: &[(&str, Option<bool>)]) {
        self.add(
            id,
            Some(parent),
            Node::ModelCall(ModelCall {
                model: Some("test-model".to_string()),
                output: Vec::new(),
                stop_reason: Some(StopReason::ToolUse),
                usage: Some(Usage {
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                }),
            }),
            Some(500),
        );
        for (n, (name, result)) in tools.iter().enumerate() {
            let tool_id = format!("{id}-tool{n}");
            self.add(
                &tool_id,
                Some(id),
                Node::ToolCall(ToolCall {
                    name: name.to_string(),
                    input: json!({"path": format!("file-{n}.txt")}),
                    result: result.map(|is_error| ToolResult {
                        content: Vec::new(),
                        is_error,
                    }),
                }),
                result.map(|_| 100),
            );
        }
    }

    pub fn marker(&mut self, id: &str, parent: &str) {
        self.add(
            id,
            Some(parent),
            Node::Marker(Marker {
                kind: "hook".to_string(),
                summary: "A hook ran".to_string(),
            }),
            None,
        );
    }
}

pub fn to_json(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("serialise")
}
