//! The sample session's context grows (or holds) from call to call.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use adapter_claude_code::{context_rules, load_session};
use trace_core::{Node, Trace};

fn collect_calls(trace: &Trace, id: &str, out: &mut Vec<String>) {
    for child in trace.children(id) {
        match child.node {
            // Subagent runs are measured separately.
            Node::Run(_) => {}
            Node::ModelCall(_) => {
                out.push(child.id.clone());
                collect_calls(trace, &child.id, out);
            }
            _ => collect_calls(trace, &child.id, out),
        }
    }
}

#[test]
fn main_run_context_never_shrinks() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude-code/basic/00000000-0000-4000-8000-000000000002.jsonl");
    let session = load_session(&path).expect("load");
    let mut trace = Trace::new();
    for event in session.events {
        trace.apply(event).expect("tree");
    }
    let measures = insights::measure_transcript_all(&trace, &context_rules());
    let root = trace
        .roots()
        .find(|e| matches!(e.node, Node::Run(_)))
        .expect("a root run");
    let mut calls = Vec::new();
    collect_calls(&trace, &root.id, &mut calls);
    assert!(calls.len() > 1);
    let mut previous = 0;
    for id in &calls {
        let total = measures
            .get(id)
            .expect("every call is measured")
            .total_chars();
        assert!(total >= previous, "{id}: {total} < {previous}");
        previous = total;
    }
    assert!(previous > 0);
}
