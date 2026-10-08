//! Measures a call's context from the trace alone, with no captured request.

use std::collections::HashMap;

use trace_core::{ContentBlock, Node, ToolCall, Trace};

use crate::measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, SliceKind, json_chars, text_chars,
};
use crate::rules::ContextRules;

/// Measures every model call of the trace from the trace alone, one pass
/// per run. Keys are model call ids.
pub fn measure_transcript_all(
    trace: &Trace,
    rules: &ContextRules,
) -> HashMap<String, ContextMeasure> {
    let mut out = HashMap::new();
    let mut pending: Vec<String> = trace
        .roots()
        .filter(|r| matches!(r.node, Node::Run(_)))
        .map(|r| r.id.clone())
        .collect();
    pending.reverse();
    while let Some(run_id) = pending.pop() {
        let mut nested = Vec::new();
        let mut running = ContextMeasure::new(ContextSource::Transcript);
        visit(trace, &run_id, rules, &mut running, &mut out, &mut nested);
        pending.extend(nested.into_iter().rev());
    }
    out
}

/// Measures one model call from the trace alone. `None` if `model_call_id`
/// is not a model call.
pub fn measure_transcript(
    trace: &Trace,
    model_call_id: &str,
    rules: &ContextRules,
) -> Option<ContextMeasure> {
    let mut current = trace.get(model_call_id)?;
    if !matches!(current.node, Node::ModelCall(_)) {
        return None;
    }
    let run_id = loop {
        let parent = trace.get(current.parent_id.as_deref()?)?;
        if matches!(parent.node, Node::Run(_)) {
            break parent.id.clone();
        }
        current = parent;
    };
    let mut out = HashMap::new();
    let mut running = ContextMeasure::new(ContextSource::Transcript);
    visit(
        trace,
        &run_id,
        rules,
        &mut running,
        &mut out,
        &mut Vec::new(),
    );
    out.remove(model_call_id)
}

/// Walks the children of `id` depth first, growing `running` and storing a
/// clone of it at each model call. Nested runs are collected, not entered.
fn visit(
    trace: &Trace,
    id: &str,
    rules: &ContextRules,
    running: &mut ContextMeasure,
    out: &mut HashMap<String, ContextMeasure>,
    nested: &mut Vec<String>,
) {
    for child in trace.children(id) {
        match &child.node {
            Node::Run(_) => {
                nested.push(child.id.clone());
                continue;
            }
            Node::Turn(turn) => {
                let mut chars = 0;
                for block in &turn.prompt {
                    match block {
                        ContentBlock::Image { .. } => {
                            running.add(SliceKind::Conversation, "images", IMAGE_CHARS)
                        }
                        other => chars += block_chars(other),
                    }
                }
                if chars > 0 {
                    running.add(SliceKind::Conversation, "your prompts", chars);
                }
            }
            Node::ModelCall(call) => {
                out.insert(child.id.clone(), running.clone());
                let chars: u64 = call.output.iter().map(block_chars).sum();
                if chars > 0 {
                    running.add(SliceKind::Conversation, "model replies", chars);
                }
            }
            Node::ToolCall(tool) => add_tool(running, tool, rules),
            Node::Marker(marker) => {
                if marker.kind == "compaction" {
                    running.clear();
                }
            }
            // Applied in a later task.
            Node::ContextUpdate(_) => {}
        }
        visit(trace, &child.id, rules, running, out, nested);
    }
}

fn add_tool(running: &mut ContextMeasure, tool: &ToolCall, rules: &ContextRules) {
    running.add(
        SliceKind::Conversation,
        "tool inputs",
        json_chars(&tool.input),
    );
    let Some(result) = &tool.result else {
        return;
    };
    let chars: u64 = result.content.iter().map(block_chars).sum();
    let path = rules
        .file_reads
        .iter()
        .find(|r| r.tool == tool.name)
        .and_then(|r| tool.input.get(&r.path_field))
        .and_then(|v| v.as_str());
    match path {
        Some(path) => running.add(SliceKind::FilesRead, path, chars),
        None => running.add(SliceKind::ToolResults, &tool.name, chars),
    }
}

fn block_chars(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text { text } | ContentBlock::Thinking { text } => text_chars(text),
        ContentBlock::Image { .. } => IMAGE_CHARS,
        ContentBlock::Other { kind } => text_chars(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::FileReadRule;
    use serde_json::json;
    use trace_core::{Marker, ModelCall, Run, ToolResult, TraceEvent, Turn};

    fn add(trace: &mut Trace, id: &str, parent: Option<&str>, node: Node) {
        let event = TraceEvent {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            node,
            started_at_ms: None,
            ended_at_ms: None,
            raw: Vec::new(),
            metadata: Default::default(),
        };
        assert!(trace.apply(event).is_ok());
    }

    fn run() -> Node {
        Node::Run(Run {
            harness: "test".into(),
            title: None,
        })
    }

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text { text: s.into() }
    }

    fn call(reply: &str) -> Node {
        Node::ModelCall(ModelCall {
            model: None,
            output: if reply.is_empty() {
                Vec::new()
            } else {
                vec![text(reply)]
            },
            stop_reason: None,
            usage: None,
        })
    }

    fn rules() -> ContextRules {
        ContextRules {
            file_reads: vec![FileReadRule {
                tool: "ReadFile".into(),
                path_field: "path".into(),
            }],
            ..Default::default()
        }
    }

    fn sample() -> Trace {
        let mut t = Trace::new();
        add(&mut t, "R", None, run());
        let turn = Node::Turn(Turn {
            prompt: vec![text("hello")],
        });
        add(&mut t, "T1", Some("R"), turn);
        add(&mut t, "M1", Some("T1"), call("ok"));
        let tool = Node::ToolCall(ToolCall {
            name: "ReadFile".into(),
            input: json!({"path": "a.rs"}),
            result: Some(ToolResult {
                content: vec![text(&"x".repeat(40))],
                is_error: false,
            }),
        });
        add(&mut t, "X", Some("M1"), tool);
        add(&mut t, "S", Some("X"), run());
        add(&mut t, "S1", Some("S"), call("sub"));
        add(&mut t, "M2", Some("T1"), call(""));
        let marker = Node::Marker(Marker {
            kind: "compaction".into(),
            summary: String::new(),
        });
        add(&mut t, "C", Some("T1"), marker);
        add(&mut t, "M3", Some("T1"), call(""));
        t
    }

    fn chars(m: &ContextMeasure, kind: SliceKind, label: &str) -> Option<u64> {
        m.items(kind)
            .iter()
            .find(|i| i.label == label)
            .map(|i| i.chars)
    }

    #[test]
    fn first_call_sees_only_the_prompt() {
        let m = measure_transcript(&sample(), "M1", &rules()).unwrap();
        assert_eq!(m.source, ContextSource::Transcript);
        assert_eq!(chars(&m, SliceKind::Conversation, "your prompts"), Some(5));
        assert_eq!(m.total_chars(), 5);
    }

    #[test]
    fn later_call_sees_replies_inputs_and_results() {
        let m = measure_transcript(&sample(), "M2", &rules()).unwrap();
        assert_eq!(chars(&m, SliceKind::Conversation, "model replies"), Some(2));
        let input_len = json_chars(&json!({"path": "a.rs"}));
        assert_eq!(
            chars(&m, SliceKind::Conversation, "tool inputs"),
            Some(input_len)
        );
        assert_eq!(chars(&m, SliceKind::FilesRead, "a.rs"), Some(40));
    }

    #[test]
    fn compaction_resets_history() {
        let all = measure_transcript_all(&sample(), &rules());
        assert_eq!(all["M3"].total_chars(), 0);
    }

    #[test]
    fn subagent_run_is_separate() {
        let all = measure_transcript_all(&sample(), &rules());
        assert_eq!(all["S1"].total_chars(), 0);
        let m2 = &all["M2"];
        assert_eq!(chars(m2, SliceKind::Conversation, "model replies"), Some(2));
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn not_a_model_call_is_none() {
        assert!(measure_transcript(&sample(), "T1", &rules()).is_none());
        assert!(measure_transcript(&sample(), "nope", &rules()).is_none());
    }
}
