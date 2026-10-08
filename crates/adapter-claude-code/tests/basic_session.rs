//! Tests the adapter against the sanitised fixture in `fixtures/claude-code/basic`.
//!
//! The expected shape below was worked out by hand from the fixture lines:
//! five turns (four typed prompts and one task notification), a forked-skill
//! subagent, parallel tool calls, hook markers, and one failing tool call.

// Test-only file: panicking on a bad fixture is the right failure mode here.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use adapter_claude_code::load_session;
use trace_core::{ContentBlock, Node, StopReason, Trace, TraceEvent};

const SESSION: &str = "00000000-0000-4000-8000-000000000002";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("claude-code")
        .join("basic")
}

fn load() -> (Trace, Vec<TraceEvent>) {
    let session = load_session(&fixture_dir().join(format!("{SESSION}.jsonl"))).expect("load");
    assert!(
        session.skipped.is_empty(),
        "every fixture line type should be understood: {:#?}",
        session.skipped
    );
    let mut trace = Trace::new();
    for event in &session.events {
        trace
            .apply(event.clone())
            .expect("events must form a valid tree");
    }
    (trace, session.events)
}

/// Children that are steps; hidden-context updates are checked separately.
fn steps<'a>(trace: &'a Trace, id: &str) -> impl Iterator<Item = &'a TraceEvent> {
    trace
        .children(id)
        .filter(|e| !matches!(e.node, Node::ContextUpdate(_)))
}

fn kinds(trace: &Trace, id: &str) -> Vec<&'static str> {
    steps(trace, id).map(|e| e.node.kind_name()).collect()
}

fn turns(trace: &Trace, run_id: &str) -> Vec<TraceEvent> {
    trace
        .children(run_id)
        .filter(|e| matches!(e.node, Node::Turn(_)))
        .cloned()
        .collect()
}

fn prompt_text(turn: &TraceEvent) -> String {
    let Node::Turn(t) = &turn.node else {
        panic!("not a turn")
    };
    t.prompt
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.as_str(),
            _ => "",
        })
        .collect()
}

#[test]
fn builds_one_run_with_session_markers_then_five_turns() {
    let (trace, _) = load();
    let roots: Vec<_> = trace.roots().map(|e| e.id.clone()).collect();
    assert_eq!(roots, [format!("run:{SESSION}")]);

    let run_id = &roots[0];
    assert_eq!(
        kinds(&trace, run_id),
        [
            "marker", "marker", "marker", "turn", "turn", "turn", "turn", "turn"
        ]
    );
    let markers: Vec<_> = trace
        .children(run_id)
        .filter_map(|e| match &e.node {
            Node::Marker(m) => Some(m.kind.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(markers, ["hook", "hook", "local_command"]);

    let run = trace.get(run_id).expect("run");
    let Node::Run(r) = &run.node else {
        panic!("not a run")
    };
    assert_eq!(r.harness, "claude-code");
    assert!(r.title.is_some(), "title comes from the ai-title line");
    assert!(run.started_at_ms.is_some() && run.ended_at_ms > run.started_at_ms);
}

#[test]
fn turns_have_the_expected_prompts_and_children() {
    let (trace, _) = load();
    let turns = turns(&trace, &format!("run:{SESSION}"));
    let prompts: Vec<_> = turns.iter().map(prompt_text).collect();
    assert_eq!(prompts[0], "List the PowerShell scripts in this folder.");
    assert_eq!(prompts[1], "Find every TODO comment in the source files.");
    assert_eq!(prompts[2], "Which file defines the greeting function?");
    assert_eq!(prompts[3], "Run the tests and review the result.");
    assert!(prompts[4].starts_with("<task-notification>"));
    assert_eq!(turns[4].metadata["origin"], "task-notification");
    assert_eq!(turns[0].metadata["origin"], "human");

    let model_calls: Vec<usize> = turns
        .iter()
        .map(|t| {
            kinds(&trace, &t.id)
                .iter()
                .filter(|k| **k == "model_call")
                .count()
        })
        .collect();
    assert_eq!(model_calls, [3, 4, 2, 6, 2]);

    // Turn 4: four PreToolUse hook errors interleaved, then the Stop summary.
    assert_eq!(
        kinds(&trace, &turns[3].id),
        [
            "model_call",
            "marker",
            "model_call",
            "marker",
            "model_call",
            "marker",
            "model_call",
            "marker",
            "model_call",
            "model_call",
            "marker"
        ]
    );

    for turn in &turns {
        assert!(
            turn.ended_at_ms > turn.started_at_ms,
            "turn_duration sets the end"
        );
        assert!(turn.metadata.contains_key("duration_ms"));
    }
}

#[test]
fn model_calls_group_lines_by_message_and_keep_usage() {
    let (trace, _) = load();
    let turns = turns(&trace, &format!("run:{SESSION}"));
    let first: Vec<_> = steps(&trace, &turns[0].id).collect();
    let Node::ModelCall(call) = &first[0].node else {
        panic!("first child should be a model call")
    };
    // Lines 33-35: thinking, text, tool_use.
    assert_eq!(
        first[0].raw.iter().map(|r| r.line).collect::<Vec<_>>(),
        [33, 34, 35]
    );
    assert!(matches!(call.output[0], ContentBlock::Thinking { .. }));
    assert!(matches!(call.output[1], ContentBlock::Text { .. }));
    assert_eq!(call.output.len(), 2, "tool uses are children, not output");
    assert_eq!(call.stop_reason, Some(StopReason::ToolUse));
    let usage = call.usage.expect("usage");
    assert_eq!(usage.output_tokens, Some(462));
    assert!(usage.context_tokens().is_some());
    assert!(call.model.is_some());
    assert!(first[0].metadata.contains_key("request_id"));

    let last = steps(&trace, &turns[0].id)
        .nth(2)
        .expect("third model call");
    let Node::ModelCall(call) = &last.node else {
        panic!("not a model call")
    };
    assert_eq!(call.stop_reason, Some(StopReason::EndTurn));
}

#[test]
fn parallel_tool_calls_share_a_model_call_even_when_interleaved() {
    let (trace, _) = load();
    let turns = turns(&trace, &format!("run:{SESSION}"));
    // Turn 2, second model call: lines 59, 60 and 62, with a tool result
    // (line 61) in between.
    let call = steps(&trace, &turns[1].id).nth(1).expect("model call");
    assert_eq!(
        call.raw.iter().map(|r| r.line).collect::<Vec<_>>(),
        [59, 60, 62]
    );
    let tools: Vec<_> = trace.children(&call.id).collect();
    assert_eq!(tools.len(), 2);
    for tool in tools {
        let Node::ToolCall(t) = &tool.node else {
            panic!("not a tool call")
        };
        assert_eq!(t.name, "Grep");
        assert!(t.result.is_some());
    }
}

#[test]
fn every_tool_call_gets_its_result_and_errors_are_flagged() {
    let (trace, events) = load();
    let mut ids: Vec<&str> = events
        .iter()
        .filter(|e| matches!(e.node, Node::ToolCall(_)))
        .map(|e| e.id.as_str())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 14, "13 in the session, 1 in the subagent");

    let mut errors = 0;
    for id in ids {
        let event = trace.get(id).expect("tool call");
        let Node::ToolCall(call) = &event.node else {
            panic!("not a tool call")
        };
        let result = call.result.as_ref().expect("every tool call finished");
        assert_eq!(event.raw.len(), 2, "tool_use line plus tool_result line");
        assert!(event.ended_at_ms >= event.started_at_ms);
        if result.is_error {
            errors += 1;
            assert_eq!(call.name, "Bash");
        }
    }
    assert_eq!(errors, 1);
}

#[test]
fn forked_skill_subagent_is_nested_under_its_tool_call() {
    let (trace, _) = load();
    let skill = trace
        .get("tool:toolu_example000000000156")
        .expect("Skill tool call");
    let Node::ToolCall(call) = &skill.node else {
        panic!("not a tool call")
    };
    assert_eq!(call.name, "Skill");
    assert_eq!(skill.metadata["agent_status"], "forked");

    let runs: Vec<_> = trace.children(&skill.id).collect();
    assert_eq!(runs.len(), 1);
    let sub = runs[0];
    let Node::Run(run) = &sub.node else {
        panic!("not a run")
    };
    assert!(
        run.title.is_some(),
        "title comes from the .meta.json description"
    );

    let sub_turns: Vec<_> = trace.children(&sub.id).collect();
    assert_eq!(
        sub_turns.len(),
        1,
        "the meta task line is the subagent's prompt"
    );
    assert_eq!(
        kinds(&trace, &sub_turns[0].id),
        ["model_call", "marker", "model_call"]
    );
    let first = steps(&trace, &sub_turns[0].id).next().expect("model call");
    assert!(first.raw[0].source.starts_with("subagents/agent-"));
}

#[test]
fn raw_lines_match_the_fixture_exactly() {
    let (_, events) = load();
    let main = std::fs::read_to_string(fixture_dir().join(format!("{SESSION}.jsonl")))
        .expect("read fixture");
    let lines: Vec<&str> = main.lines().collect();
    let mut checked = 0;
    for event in &events {
        for raw in &event.raw {
            if raw.source == format!("{SESSION}.jsonl") {
                assert_eq!(raw.text, lines[raw.line as usize - 1]);
                checked += 1;
            }
        }
    }
    assert!(checked > 50);
}

#[test]
fn only_known_node_kinds_have_raw_sources() {
    let (trace, events) = load();
    for event in &events {
        let latest = trace.get(&event.id).expect("node");
        if !matches!(latest.node, Node::Run(_)) {
            assert!(!latest.raw.is_empty(), "{} has no raw source", latest.id);
        }
    }
}

#[test]
fn model_call_start_skips_attachment_lines() {
    // Attachment lines are written when a response arrives, so using them as
    // the request start made some calls look like they took a few ms. Every
    // model call in this fixture really took more than a second.
    let (_, events) = load();
    for event in events
        .iter()
        .filter(|e| matches!(e.node, Node::ModelCall(_)))
    {
        let (Some(start), Some(end)) = (event.started_at_ms, event.ended_at_ms) else {
            panic!("{} has no timing", event.id)
        };
        assert!(
            end - start >= 1_000,
            "{} took only {}ms",
            event.id,
            end - start
        );
    }
}

#[test]
fn attachment_lines_become_context_updates_by_kind() {
    use std::collections::BTreeMap;
    use trace_core::ContextPartKind;

    let (_, events) = load();
    // Events can be sent more than once; count each node's last version.
    let mut last: BTreeMap<&str, &TraceEvent> = BTreeMap::new();
    for event in &events {
        if matches!(event.node, Node::ContextUpdate(_)) {
            last.insert(event.id.as_str(), event);
        }
    }
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut removals = 0;
    for event in last.values() {
        let Node::ContextUpdate(update) = &event.node else {
            continue;
        };
        assert_eq!(event.raw.len(), 1, "{} keeps its raw line", event.id);
        removals += update.remove.len();
        for part in &update.parts {
            let name = match &part.kind {
                ContextPartKind::SystemPrompt => "system_prompt",
                ContextPartKind::ToolDefinitions => "tool_definitions",
                ContextPartKind::Instructions => "instructions",
                ContextPartKind::Reminder => "reminder",
                ContextPartKind::Other(_) => "other",
            };
            *by_kind.entry(name).or_default() += 1;
        }
    }
    assert_eq!(removals, 0, "the fixture has no compaction");
    assert_eq!(by_kind, COUNTS.iter().copied().collect());
    assert_eq!(last.len(), UPDATES);
}

/// Expected parts by kind and number of `context_update` nodes in the fixture.
const COUNTS: [(&str, usize); 4] = [
    ("instructions", 6),
    ("reminder", 19),
    ("system_prompt", 4),
    ("tool_definitions", 2),
];
const UPDATES: usize = 25;
