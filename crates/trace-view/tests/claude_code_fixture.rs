//! Tests the diagram model against the sanitised Claude Code fixture, loaded
//! through the adapter. The shape is described in the adapter's
//! `tests/basic_session.rs`: five turns, parallel Grep calls, a forked-skill
//! subagent, hook markers, and one failing Bash call.

#![allow(clippy::expect_used)]

mod common;

use std::collections::HashSet;

use common::{
    CLAUDE_SESSION, all_nodes, assert_ids_unique, claude_session_path, claude_trace, kinds, to_json,
};
use trace_view::{DiagramNode, NodeKind, Status, build_session, node_detail};

fn find(nodes: &[DiagramNode], kind: NodeKind) -> Vec<&DiagramNode> {
    nodes.iter().filter(|n| n.kind == kind).collect()
}

#[test]
fn five_prompts_in_order_and_session_markers() {
    let diagram = build_session(&claude_trace());
    assert_eq!(diagram.run_id, Some(format!("run:{CLAUDE_SESSION}")));
    assert_eq!(diagram.harness.as_deref(), Some("claude-code"));
    assert!(diagram.duration_ms.is_some_and(|d| d > 0));

    let previews: Vec<&str> = diagram
        .prompts
        .iter()
        .map(|p| p.prompt_preview.as_str())
        .collect();
    assert_eq!(previews[0], "List the PowerShell scripts in this folder.");
    assert_eq!(previews[1], "Find every TODO comment in the source files.");
    assert_eq!(previews[2], "Which file defines the greeting function?");
    assert_eq!(previews[3], "Run the tests and review the result.");
    assert!(previews[4].starts_with("<task-notification>"));
    assert!(previews[4].chars().count() <= 80);
    assert_eq!(
        diagram.prompts.iter().map(|p| p.index).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );

    assert_eq!(kinds(&diagram.markers), ["marker", "marker", "marker"]);
    let marker_kinds: Vec<_> = diagram
        .markers
        .iter()
        .map(|m| m.detail_label.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(marker_kinds, ["hook", "hook", "local_command"]);
}

#[test]
fn model_calls_and_markers_in_order_two_similar_calls_stay_apart() {
    let diagram = build_session(&claude_trace());
    // Two PowerShell calls in a row: below the summary threshold.
    assert_eq!(
        kinds(&diagram.prompts[0].root.children),
        ["model_call", "model_call", "model_call", "marker"]
    );
    let totals: Vec<usize> = diagram
        .prompts
        .iter()
        .map(|p| p.totals.model_calls)
        .collect();
    assert_eq!(
        totals,
        [3, 4, 2, 8, 2],
        "prompt 4 includes the subagent's 2"
    );
    let first = &diagram.prompts[0].root.children[0];
    assert_eq!(first.label, "Model call");
    assert_eq!(first.output_tokens, Some(462));
    assert!(first.context_tokens.is_some());
    assert!(
        first
            .detail_label
            .as_deref()
            .is_some_and(|d| d.ends_with("stop: tool_use"))
    );
}

#[test]
fn parallel_greps_are_grouped() {
    let diagram = build_session(&claude_trace());
    let call = &diagram.prompts[1].root.children[1];
    assert_eq!(kinds(&call.children), ["parallel_group"]);
    let group = &call.children[0];
    assert_eq!(group.detail_label.as_deref(), Some("Grep x2"));
    assert_eq!(kinds(&group.children), ["tool_call", "tool_call"]);
    assert!(group.children.iter().all(|t| t.label.starts_with("Grep(")));

    // No other model call in the session made more than one tool call.
    let groups = all_nodes(&diagram)
        .into_iter()
        .filter(|n| n.kind == NodeKind::ParallelGroup)
        .count();
    assert_eq!(groups, 1);
}

#[test]
fn four_bash_calls_with_hooks_between_become_one_summary() {
    let diagram = build_session(&claude_trace());
    let root = &diagram.prompts[3].root;
    assert_eq!(
        kinds(&root.children),
        ["summary", "marker", "model_call", "model_call", "marker"]
    );
    let summary = &root.children[0];
    assert_eq!(summary.label, "4 x Bash");
    assert_eq!(
        summary.detail_label.as_deref(),
        Some("4 model calls, 3 markers")
    );
    assert!(summary.collapsed_by_default);
    assert_eq!(
        kinds(&summary.children),
        [
            "model_call",
            "marker",
            "model_call",
            "marker",
            "model_call",
            "marker",
            "model_call"
        ]
    );
    assert_eq!(summary.trace_ids.len(), 7);
    assert_eq!(summary.id, format!("summary:{}", summary.children[0].id));
    let outputs: u64 = summary
        .children
        .iter()
        .filter_map(|c| c.output_tokens)
        .sum();
    assert_eq!(summary.output_tokens, Some(outputs));
    assert_eq!(
        summary.context_tokens,
        summary
            .children
            .iter()
            .filter_map(|c| c.context_tokens)
            .max()
    );
}

#[test]
fn forked_skill_subagent_is_nested_and_collapsed() {
    let diagram = build_session(&claude_trace());
    let root = &diagram.prompts[3].root;
    let skill_call = &root.children[2];
    assert_eq!(kinds(&skill_call.children), ["tool_call"]);
    let tool = &skill_call.children[0];
    assert!(tool.label.starts_with("Skill"));
    assert_eq!(tool.trace_ids, ["tool:toolu_example000000000156"]);
    let subs = find(&tool.children, NodeKind::Subagent);
    assert_eq!(subs.len(), 1);
    let sub = subs[0];
    assert!(sub.collapsed_by_default);
    assert_eq!(sub.trace_ids.len(), 2, "run id and its one turn id");
    assert_eq!(kinds(&sub.children), ["model_call", "marker", "model_call"]);
    assert_eq!(sub.status, Status::Ok);
}

#[test]
fn errors_propagate_to_the_prompt() {
    let diagram = build_session(&claude_trace());
    let errors: Vec<usize> = diagram.prompts.iter().map(|p| p.totals.errors).collect();
    assert_eq!(errors, [0, 0, 0, 0, 1]);
    let statuses: Vec<Status> = diagram.prompts.iter().map(|p| p.root.status).collect();
    assert_eq!(
        statuses,
        [
            Status::Ok,
            Status::Ok,
            Status::Ok,
            Status::Ok,
            Status::Error
        ]
    );
    let failing = &diagram.prompts[4].root.children[0];
    assert_eq!(failing.status, Status::Ok, "the model call itself is fine");
    assert_eq!(failing.children[0].status, Status::Error);
    assert!(failing.children[0].label.starts_with("Bash("));
}

#[test]
fn ids_are_unique_and_stable_across_reloads() {
    let first = build_session(&claude_trace());
    let second = build_session(&claude_trace());
    assert_eq!(first, second);
    assert_eq!(
        serde_json::to_string(&first).expect("json"),
        serde_json::to_string(&second).expect("json")
    );
    assert_ids_unique(&first);
}

#[test]
fn every_trace_id_resolves_to_a_detail() {
    let trace = claude_trace();
    let diagram = build_session(&trace);
    let mut seen = HashSet::new();
    for node in all_nodes(&diagram) {
        assert!(!node.trace_ids.is_empty(), "{} has no trace ids", node.id);
        for id in &node.trace_ids {
            if seen.insert(id.clone()) {
                assert!(node_detail(&trace, id).is_some(), "no detail for {id}");
            }
        }
    }
}

#[test]
fn node_detail_returns_raw_lines() {
    let trace = claude_trace();
    let text = std::fs::read_to_string(claude_session_path()).expect("read fixture");
    let lines: Vec<&str> = text.lines().collect();

    let diagram = build_session(&trace);
    let first_call = &diagram.prompts[0].root.children[0];
    let detail = node_detail(&trace, &first_call.trace_ids[0]).expect("detail");
    assert_eq!(detail.kind, "model_call");
    assert_eq!(
        detail.raw.iter().map(|r| r.line).collect::<Vec<_>>(),
        [33, 34, 35]
    );
    for raw in &detail.raw {
        assert_eq!(raw.source, format!("{CLAUDE_SESSION}.jsonl"));
        assert_eq!(raw.text, lines[raw.line as usize - 1]);
    }
    assert!(detail.metadata.contains_key("request_id"));
    assert_eq!(detail.context_tokens, first_call.context_tokens);

    let failing_tool = &diagram.prompts[4].root.children[0].children[0];
    let detail = node_detail(&trace, &failing_tool.trace_ids[0]).expect("detail");
    assert_eq!(detail.raw.len(), 2, "tool_use line and tool_result line");
    let json = to_json(&detail);
    assert_eq!(json["node"]["result"]["is_error"], true);
}

#[test]
fn whole_session_serialises_to_json() {
    let diagram = build_session(&claude_trace());
    let json = to_json(&diagram);
    assert_eq!(json["prompts"].as_array().map(Vec::len), Some(5));
    assert_eq!(json["prompts"][3]["root"]["children"][0]["kind"], "summary");
    assert_eq!(json["prompts"][4]["root"]["status"], "error");
    assert_eq!(json["markers"][0]["status"], "none");
}

#[test]
fn every_model_call_but_the_first_of_each_run_has_a_previous_call() {
    let trace = claude_trace();
    let groups = trace_view::model_calls_by_run(&trace);
    assert!(groups.iter().any(|(_, calls)| calls.len() > 1));
    for (run, calls) in groups {
        for (i, call) in calls.iter().enumerate() {
            let previous = trace_view::previous_model_call(&trace, call);
            if i == 0 {
                assert_eq!(previous, None, "first call of {run}");
            } else {
                assert_eq!(previous.as_deref(), Some(calls[i - 1].as_str()));
            }
        }
    }
}
