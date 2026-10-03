//! Tests the schema against the hand-written example trace in `fixtures/`.

// Test-only file: panicking on a bad fixture is the right failure mode here.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use trace_core::{ContentBlock, Node, StopReason, Trace, TraceError, TraceEvent, Usage};

fn load_events() -> Vec<TraceEvent> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("trace-core")
        .join("example-trace.jsonl");
    let text = std::fs::read_to_string(&path).expect("read fixture");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse event"))
        .collect()
}

fn load_trace() -> Trace {
    let mut trace = Trace::new();
    for event in load_events() {
        trace.apply(event).expect("apply event");
    }
    trace
}

fn child_ids(trace: &Trace, id: &str) -> Vec<String> {
    trace.children(id).map(|e| e.id.clone()).collect()
}

#[test]
fn every_event_round_trips_through_json() {
    for event in load_events() {
        let json = serde_json::to_string(&event).expect("serialise");
        let back: TraceEvent = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(event, back);
    }
}

#[test]
fn builds_the_expected_tree() {
    let trace = load_trace();
    assert_eq!(trace.len(), 12, "re-sent nodes must not be counted twice");

    let roots: Vec<_> = trace.roots().map(|e| e.id.clone()).collect();
    assert_eq!(roots, ["run-1"]);
    assert_eq!(child_ids(&trace, "run-1"), ["turn-1"]);
    assert_eq!(
        child_ids(&trace, "turn-1"),
        ["mc-1", "mc-2", "mk-1", "mc-4"]
    );
    assert_eq!(child_ids(&trace, "mc-1"), ["tc-1", "tc-2"]);
    assert_eq!(child_ids(&trace, "tc-3"), ["run-2"]);
    assert_eq!(child_ids(&trace, "run-2"), ["turn-2"]);
}

#[test]
fn resent_node_replaces_content_but_keeps_position() {
    let trace = load_trace();
    // tc-2's result arrived before tc-1's, but tc-1 was sent first.
    assert_eq!(child_ids(&trace, "mc-1"), ["tc-1", "tc-2"]);

    let Some(Node::ToolCall(call)) = trace.get("tc-1").map(|e| &e.node) else {
        panic!("tc-1 should be a tool call");
    };
    let result = call.result.as_ref().expect("result filled in by re-send");
    assert_eq!(
        result.content,
        [ContentBlock::Text {
            text: "Hello".into()
        }]
    );
    assert_eq!(
        trace.get("tc-1").and_then(|e| e.ended_at_ms),
        Some(1767225602000)
    );
}

#[test]
fn model_call_fields_parse() {
    let trace = load_trace();
    let event = trace.get("mc-1").expect("mc-1");
    let Node::ModelCall(call) = &event.node else {
        panic!("mc-1 should be a model call");
    };
    assert_eq!(call.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(call.output.len(), 2);
    assert_eq!(call.usage.and_then(|u| u.context_tokens()), Some(2312));
    assert_eq!(event.metadata["request_id"], "req-001");
}

#[test]
fn context_tokens_handles_unknown_counts() {
    assert_eq!(Usage::default().context_tokens(), None);
    let only_input = Usage {
        input_tokens: Some(7),
        ..Usage::default()
    };
    assert_eq!(only_input.context_tokens(), Some(7));
}

#[test]
fn unknown_stop_reason_is_kept() {
    let reason: StopReason = serde_json::from_str(r#"{"other":"refusal"}"#).expect("parse");
    assert_eq!(reason, StopReason::Other("refusal".into()));
}

#[test]
fn rejects_unknown_parent() {
    let mut trace = Trace::new();
    let mut events = load_events();
    let turn = events.remove(1);
    assert_eq!(
        trace.apply(turn),
        Err(TraceError::UnknownParent {
            id: "turn-1".into(),
            parent_id: "run-1".into()
        })
    );
    assert!(trace.is_empty());
}

#[test]
fn rejects_parent_or_kind_change_on_resend() {
    let mut trace = load_trace();
    let original = trace.get("tc-1").cloned().expect("tc-1");

    let mut moved = original.clone();
    moved.parent_id = Some("mc-2".into());
    assert!(matches!(
        trace.apply(moved),
        Err(TraceError::ParentChanged { .. })
    ));

    let mut retyped = original.clone();
    retyped.node = Node::Marker(trace_core::Marker {
        kind: "x".into(),
        summary: "y".into(),
    });
    assert!(matches!(
        trace.apply(retyped),
        Err(TraceError::KindChanged { .. })
    ));

    assert_eq!(trace.get("tc-1"), Some(&original), "trace left unchanged");
}
