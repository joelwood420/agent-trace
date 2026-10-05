//! Ordering of model calls within a run, used to pair each call with the one
//! before it (for request diffs).

use trace_core::{Node, Trace};

/// Model calls of the run that contains `model_call_id`, in order, not
/// descending into nested runs (subagents); returns the one before it.
pub fn previous_model_call(trace: &Trace, model_call_id: &str) -> Option<String> {
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
    let mut calls = Vec::new();
    walk(trace, &run_id, &mut calls, &mut Vec::new());
    let position = calls.iter().position(|id| id == model_call_id)?;
    position.checked_sub(1).map(|i| calls[i].clone())
}

/// All model call ids of a trace, each run's calls in order, runs in tree order.
/// Returns `(run id, model call ids)` pairs.
pub fn model_calls_by_run(trace: &Trace) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for root in trace.roots() {
        if matches!(root.node, Node::Run(_)) {
            visit_run(trace, &root.id, &mut out);
        }
    }
    out
}

fn visit_run(trace: &Trace, run_id: &str, out: &mut Vec<(String, Vec<String>)>) {
    let index = out.len();
    out.push((run_id.to_string(), Vec::new()));
    let mut calls = Vec::new();
    let mut nested = Vec::new();
    walk(trace, run_id, &mut calls, &mut nested);
    out[index].1 = calls;
    for run in nested {
        visit_run(trace, &run, out);
    }
}

/// Collects model calls under `id` depth first, and the nested runs met on the way.
fn walk(trace: &Trace, id: &str, calls: &mut Vec<String>, nested: &mut Vec<String>) {
    for child in trace.children(id) {
        match child.node {
            Node::Run(_) => nested.push(child.id.clone()),
            Node::ModelCall(_) => {
                calls.push(child.id.clone());
                walk(trace, &child.id, calls, nested);
            }
            _ => walk(trace, &child.id, calls, nested),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trace_core::{ModelCall, Run, ToolCall, TraceEvent, Turn};

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

    fn turn() -> Node {
        Node::Turn(Turn { prompt: Vec::new() })
    }

    fn call() -> Node {
        Node::ModelCall(ModelCall {
            model: None,
            output: Vec::new(),
            stop_reason: None,
            usage: None,
        })
    }

    fn sample() -> Trace {
        let mut t = Trace::new();
        add(&mut t, "R", None, run());
        add(&mut t, "T1", Some("R"), turn());
        add(&mut t, "M1", Some("T1"), call());
        let tool = Node::ToolCall(ToolCall {
            name: "Agent".into(),
            input: json!({}),
            result: None,
        });
        add(&mut t, "X", Some("M1"), tool);
        add(&mut t, "S", Some("X"), run());
        add(&mut t, "S1", Some("S"), call());
        add(&mut t, "S2", Some("S"), call());
        add(&mut t, "M2", Some("T1"), call());
        add(&mut t, "T2", Some("R"), turn());
        add(&mut t, "M3", Some("T2"), call());
        t
    }

    #[test]
    fn previous_model_call_skips_subagent_runs() {
        let trace = sample();
        assert_eq!(previous_model_call(&trace, "M2").as_deref(), Some("M1"));
        assert_eq!(previous_model_call(&trace, "M3").as_deref(), Some("M2"));
        assert_eq!(previous_model_call(&trace, "S2").as_deref(), Some("S1"));
        assert_eq!(previous_model_call(&trace, "M1"), None);
        assert_eq!(previous_model_call(&trace, "S1"), None);
        assert_eq!(previous_model_call(&trace, "nope"), None);
    }

    #[test]
    fn model_calls_are_grouped_by_run() {
        let trace = sample();
        assert_eq!(
            model_calls_by_run(&trace),
            [
                (
                    "R".to_string(),
                    vec!["M1".to_string(), "M2".into(), "M3".into()]
                ),
                ("S".to_string(), vec!["S1".to_string(), "S2".into()])
            ]
        );
    }
}
