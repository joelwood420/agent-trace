//! Tests the grouping and summary rules on small hand-built traces.

#![allow(clippy::expect_used)]

mod common;

use common::{TraceBuilder, assert_ids_unique, ids, kinds};
use trace_core::Trace;
use trace_view::{NodeKind, SUMMARY_MIN_CALLS, Status, build_session};

const OK: Option<bool> = Some(false);
const ERR: Option<bool> = Some(true);
const RUNNING: Option<bool> = None;

/// One model call per entry, each with one tool call of that name and result.
fn single_tool_calls(calls: &[(&str, Option<bool>)]) -> TraceBuilder {
    let mut b = TraceBuilder::new();
    for (i, call) in calls.iter().enumerate() {
        b.model_call(&format!("mc{i}"), "turn", &[*call]);
    }
    b
}

fn root_kinds(trace: &Trace) -> Vec<&'static str> {
    kinds(&build_session(trace).prompts[0].root.children)
}

#[test]
fn five_single_reads_collapse_into_one_summary() {
    let b = single_tool_calls(&[("Read", OK); 5]);
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(kinds(&root.children), ["summary"]);
    let summary = &root.children[0];
    assert_eq!(summary.label, "5 x Read");
    assert_eq!(summary.detail_label.as_deref(), Some("5 model calls"));
    assert!(summary.collapsed_by_default);
    assert_eq!(summary.id, "summary:model_call:mc0");
    assert_eq!(summary.trace_ids, ["mc0", "mc1", "mc2", "mc3", "mc4"]);
    assert_eq!(
        ids(&summary.children),
        [
            "model_call:mc0",
            "model_call:mc1",
            "model_call:mc2",
            "model_call:mc3",
            "model_call:mc4"
        ]
    );
    // Each original model call keeps its tool call.
    assert_eq!(summary.children[4].children[0].label, "Read(file-0.txt)");
    assert_eq!(summary.context_tokens, Some(100));
    assert_eq!(summary.output_tokens, Some(50));
    // Model call 0 starts at t, its tool call (the 10th event) ends at
    // t + 9000 + 100.
    assert_eq!(summary.duration_ms, Some(9_100));
    assert_eq!(summary.status, Status::Ok);
    assert_ids_unique(&diagram);
}

#[test]
fn exactly_the_threshold_collapses() {
    let b = single_tool_calls(&[("Read", OK); SUMMARY_MIN_CALLS]);
    assert_eq!(root_kinds(&b.trace), ["summary"]);
}

#[test]
fn one_below_the_threshold_does_not_collapse() {
    let b = single_tool_calls(&[("Read", OK); SUMMARY_MIN_CALLS - 1]);
    assert_eq!(
        root_kinds(&b.trace),
        vec!["model_call"; SUMMARY_MIN_CALLS - 1]
    );
}

#[test]
fn a_different_tool_breaks_the_run() {
    let b = single_tool_calls(&[
        ("Read", OK),
        ("Read", OK),
        ("Grep", OK),
        ("Read", OK),
        ("Read", OK),
    ]);
    assert_eq!(root_kinds(&b.trace), vec!["model_call"; 5]);
}

#[test]
fn runs_of_different_tools_each_collapse() {
    let b = single_tool_calls(&[
        ("Read", OK),
        ("Read", OK),
        ("Read", OK),
        ("Grep", OK),
        ("Grep", OK),
        ("Grep", OK),
    ]);
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(kinds(&root.children), ["summary", "summary"]);
    assert_eq!(root.children[0].label, "3 x Read");
    assert_eq!(root.children[1].label, "3 x Grep");
}

#[test]
fn an_error_breaks_the_run_and_shows_on_the_prompt() {
    let b = single_tool_calls(&[
        ("Read", OK),
        ("Read", OK),
        ("Read", ERR),
        ("Read", OK),
        ("Read", OK),
    ]);
    let diagram = build_session(&b.trace);
    let prompt = &diagram.prompts[0];
    assert_eq!(kinds(&prompt.root.children), vec!["model_call"; 5]);
    assert_eq!(prompt.root.children[2].children[0].status, Status::Error);
    assert_eq!(prompt.root.status, Status::Error);
    assert_eq!(prompt.totals.errors, 1);
    assert_eq!(
        prompt.root.detail_label.as_deref(),
        Some("5 model calls, 5 tool calls, 1 error")
    );
}

#[test]
fn markers_inside_a_run_are_kept_but_not_at_its_edges() {
    let mut b = TraceBuilder::new();
    b.marker("before", "turn");
    b.model_call("mc0", "turn", &[("Read", OK)]);
    b.marker("between", "turn");
    b.model_call("mc1", "turn", &[("Read", OK)]);
    b.model_call("mc2", "turn", &[("Read", OK)]);
    b.marker("after", "turn");
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(kinds(&root.children), ["marker", "summary", "marker"]);
    let summary = &root.children[1];
    assert_eq!(summary.label, "3 x Read");
    assert_eq!(
        summary.detail_label.as_deref(),
        Some("3 model calls, 1 marker")
    );
    assert_eq!(
        kinds(&summary.children),
        ["model_call", "marker", "model_call", "model_call"]
    );
}

#[test]
fn model_calls_with_no_tool_or_two_tools_are_not_similar() {
    let mut b = TraceBuilder::new();
    b.model_call("mc0", "turn", &[("Read", OK)]);
    b.model_call("mc1", "turn", &[("Read", OK), ("Read", OK)]);
    b.model_call("mc2", "turn", &[("Read", OK)]);
    b.model_call("mc3", "turn", &[]);
    b.model_call("mc4", "turn", &[("Read", OK)]);
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(kinds(&root.children), vec!["model_call"; 5]);
    assert_eq!(kinds(&root.children[1].children), ["parallel_group"]);
    assert_eq!(root.children[1].children[0].id, "parallel_group:mc1");
    assert_eq!(
        root.children[1].children[0].detail_label.as_deref(),
        Some("Read x2")
    );
}

#[test]
fn running_tools_show_as_running_and_are_not_summarised() {
    let mut b = single_tool_calls(&[("Read", OK), ("Read", OK)]);
    b.model_call("mc2", "turn", &[("Read", RUNNING)]);
    b.model_call("mc3", "turn", &[("Grep", OK), ("Grep", RUNNING)]);
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(kinds(&root.children), vec!["model_call"; 4]);
    assert_eq!(root.children[2].children[0].status, Status::Running);
    let group = &root.children[3].children[0];
    assert_eq!(group.kind, NodeKind::ParallelGroup);
    assert_eq!(group.status, Status::Running);
    assert_eq!(group.duration_ms, None, "unknown while a tool runs");
    assert_eq!(root.status, Status::Running);
}

#[test]
fn errors_beat_running_in_container_status() {
    let mut b = TraceBuilder::new();
    b.model_call("mc0", "turn", &[("Read", ERR), ("Read", RUNNING)]);
    let diagram = build_session(&b.trace);
    let root = &diagram.prompts[0].root;
    assert_eq!(root.children[0].children[0].status, Status::Error);
    assert_eq!(root.status, Status::Error);
}

#[test]
fn a_trace_without_a_run_gives_an_empty_session() {
    let diagram = build_session(&Trace::new());
    assert!(diagram.run_id.is_none());
    assert!(diagram.prompts.is_empty());
    assert!(diagram.markers.is_empty());
}
