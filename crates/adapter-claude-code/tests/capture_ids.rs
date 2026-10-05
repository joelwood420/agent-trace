//! Checks the capture id helpers agree with the ids the parser produces.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use adapter_claude_code::{load_session, model_call_id};
use trace_core::Node;

#[test]
fn fixture_model_call_ids_use_the_capture_prefix() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("claude-code")
        .join("basic")
        .join("00000000-0000-4000-8000-000000000002.jsonl");
    let session = load_session(&path).expect("load");
    let ids: Vec<&str> = session
        .events
        .iter()
        .filter(|e| matches!(e.node, Node::ModelCall(_)))
        .map(|e| e.id.as_str())
        .collect();
    assert!(!ids.is_empty());
    for id in ids {
        let message_id = id.strip_prefix("model:").expect("model: prefix");
        assert_eq!(model_call_id(message_id), id);
    }
}
