//! Tests the diagram model against the hand-written schema example in
//! `fixtures/trace-core/example-trace.jsonl`: one prompt with parallel tool
//! calls, a subagent, and a compaction marker.

#![allow(clippy::expect_used)]

mod common;

use std::path::PathBuf;

use common::{assert_ids_unique, example_trace, ids, kinds, to_json};
use serde_json::Value;
use trace_core::Node;
use trace_view::{NodeKind, Status, build_session, node_detail};

#[test]
fn session_has_run_info_and_one_prompt() {
    let diagram = build_session(&example_trace());
    assert_eq!(diagram.run_id.as_deref(), Some("run-1"));
    assert_eq!(diagram.title.as_deref(), Some("Add a greeting"));
    assert_eq!(diagram.harness.as_deref(), Some("example-harness"));
    assert_eq!(diagram.started_at_ms, Some(1767225600000));
    assert_eq!(diagram.duration_ms, None, "the run has no end time");
    assert!(diagram.markers.is_empty());
    assert_eq!(diagram.prompts.len(), 1);

    let prompt = &diagram.prompts[0];
    assert_eq!(prompt.index, 0);
    assert_eq!(prompt.turn_id, "turn-1");
    assert_eq!(
        prompt.prompt_preview,
        "Add a greeting to hello.txt and check the docs folder."
    );
    assert_eq!(prompt.root.kind, NodeKind::Prompt);
    assert_eq!(prompt.root.id, "prompt:turn-1");
    assert_eq!(prompt.root.trace_ids, ["turn-1"]);
}

#[test]
fn prompt_children_are_model_calls_and_markers_in_order() {
    let diagram = build_session(&example_trace());
    let root = &diagram.prompts[0].root;
    assert_eq!(
        ids(&root.children),
        [
            "model_call:mc-1",
            "model_call:mc-2",
            "marker:mk-1",
            "model_call:mc-4"
        ]
    );
    let marker = &root.children[2];
    assert_eq!(
        marker.label,
        "Earlier context was summarised to save space."
    );
    assert_eq!(marker.detail_label.as_deref(), Some("compaction"));
    assert_eq!(marker.status, Status::None);
    assert!(root.children[3].children.is_empty());
}

#[test]
fn parallel_tool_calls_are_wrapped_in_a_group() {
    let diagram = build_session(&example_trace());
    let mc1 = &diagram.prompts[0].root.children[0];
    assert_eq!(kinds(&mc1.children), ["parallel_group"]);
    let group = &mc1.children[0];
    assert_eq!(group.id, "parallel_group:mc-1");
    assert_eq!(group.label, "2 tool calls in parallel");
    assert_eq!(group.detail_label.as_deref(), Some("Read, ListFiles"));
    assert!(!group.collapsed_by_default);
    assert_eq!(group.trace_ids, ["tc-1", "tc-2"]);
    assert_eq!(ids(&group.children), ["tool_call:tc-1", "tool_call:tc-2"]);
    assert_eq!(group.children[0].label, "Read(hello.txt)");
    assert_eq!(group.children[1].label, "ListFiles(docs/*)");
    assert_eq!(group.children[0].duration_ms, Some(100));
    assert_eq!(group.children[1].duration_ms, Some(50));
    // From the first start to the last end.
    assert_eq!(group.duration_ms, Some(100));
    assert_eq!(group.status, Status::Ok);
}

#[test]
fn subagent_hangs_under_its_tool_call_and_starts_collapsed() {
    let diagram = build_session(&example_trace());
    let mc2 = &diagram.prompts[0].root.children[1];
    assert_eq!(kinds(&mc2.children), ["tool_call"], "one tool, no group");
    let tool = &mc2.children[0];
    assert_eq!(tool.label, "Subagent(Review docs/intro.md)");
    assert!(!tool.collapsed_by_default);
    assert_eq!(kinds(&tool.children), ["subagent"]);

    let sub = &tool.children[0];
    assert_eq!(sub.id, "subagent:run-2");
    assert_eq!(sub.label, "Review docs/intro.md");
    assert!(sub.collapsed_by_default);
    assert_eq!(sub.trace_ids, ["run-2", "turn-2"], "single turn is inlined");
    assert_eq!(ids(&sub.children), ["model_call:mc-3"]);
    assert_eq!(
        sub.detail_label.as_deref(),
        Some("1 model call, 0 tool calls")
    );
    assert_eq!(sub.context_tokens, Some(500));
    assert_eq!(sub.output_tokens, Some(8));
}

#[test]
fn token_and_duration_numbers() {
    let diagram = build_session(&example_trace());
    let prompt = &diagram.prompts[0];
    let mc1 = &prompt.root.children[0];
    assert_eq!(mc1.label, "Model call");
    assert_eq!(
        mc1.detail_label.as_deref(),
        Some("example-model-1, stop: tool_use")
    );
    assert_eq!(mc1.context_tokens, Some(12 + 2000 + 300));
    assert_eq!(mc1.output_tokens, Some(40));
    assert_eq!(mc1.duration_ms, Some(1800));
    assert_eq!(mc1.started_at_ms, Some(1767225600100));

    let t = &prompt.totals;
    assert_eq!(t.model_calls, 4, "includes the subagent's call");
    assert_eq!(t.tool_calls, 3);
    assert_eq!(t.errors, 0);
    // mc-2 has 30 + 2300 + 60. The subagent's 500 is its own context.
    assert_eq!(t.max_context_tokens, Some(2390));
    assert_eq!(t.output_tokens, Some(40 + 25 + 8 + 12));
    assert_eq!(prompt.root.context_tokens, Some(2390));
    assert_eq!(prompt.root.output_tokens, Some(85));
    assert_eq!(prompt.duration_ms, None, "the turn has no end time");
}

#[test]
fn ids_are_unique_and_stable() {
    let first = build_session(&example_trace());
    let second = build_session(&example_trace());
    assert_eq!(first, second);
    assert_eq!(to_json(&first), to_json(&second));
    assert_ids_unique(&first);
}

#[test]
fn node_detail_gives_full_content() {
    let trace = example_trace();
    let detail = node_detail(&trace, "tc-1").expect("detail");
    assert_eq!(detail.kind, "tool_call");
    assert_eq!(detail.parent_id.as_deref(), Some("mc-1"));
    assert_eq!(detail.duration_ms, Some(100));
    let Node::ToolCall(tool) = &detail.node else {
        panic!("not a tool call")
    };
    assert_eq!(tool.input["path"], "hello.txt");
    assert!(tool.result.is_some(), "the latest version has the result");

    let mc = node_detail(&trace, "mc-1").expect("detail");
    assert_eq!(mc.context_tokens, Some(2312));
    assert_eq!(mc.metadata["request_id"], "req-001");
    let json = to_json(&mc);
    assert_eq!(json["node"]["type"], "model_call");
    assert_eq!(json["node"]["output"][1]["text"], "Let me look at both.");

    assert!(node_detail(&trace, "no-such-id").is_none());
}

/// The JSON the UI receives for the example trace must match the checked-in
/// snapshot, so the contract cannot change without a visible diff. To
/// accept an intended change, run the tests with the environment variable
/// `SNITCHCRAFT_UPDATE_SNAPSHOTS=1` and review the snapshot diff.
#[test]
fn json_shape_matches_snapshot() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("example-trace.diagram.json");
    let actual = to_json(&build_session(&example_trace()));
    if std::env::var_os("SNITCHCRAFT_UPDATE_SNAPSHOTS").is_some() {
        let mut text = serde_json::to_string_pretty(&actual).expect("serialise");
        text.push('\n');
        std::fs::write(&path, text).expect("write snapshot");
    }
    let expected: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read snapshot"))
            .expect("parse snapshot");
    assert_eq!(actual, expected, "diagram JSON differs from {path:?}");
}

#[test]
fn json_uses_snake_case_and_always_has_every_field() {
    let json = to_json(&build_session(&example_trace()));
    let root = &json["prompts"][0]["root"];
    let mut keys: Vec<&str> = root
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "children",
            "collapsed_by_default",
            "context_tokens",
            "detail_label",
            "duration_ms",
            "id",
            "kind",
            "label",
            "output_tokens",
            "started_at_ms",
            "status",
            "trace_ids"
        ]
    );
    assert_eq!(root["kind"], "prompt");
    assert_eq!(root["children"][0]["children"][0]["kind"], "parallel_group");
    // Unknown values are null, not missing.
    assert_eq!(root["children"][2]["context_tokens"], Value::Null);
}
