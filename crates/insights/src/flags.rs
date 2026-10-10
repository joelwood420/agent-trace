//! Waste and problem flags: patterns in a trace worth a second look, such as
//! the same file read again and again or a tool result that fills the context.
//! Each run (the main agent and each subagent) is checked on its own. Harness
//! rules come in as `FlagRules`.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};
use trace_core::{Compaction, Node, ToolCall, Trace};

use crate::advice::{AdviceLevel, about_tokens};
use crate::breakdown::CHARS_PER_TOKEN;
use crate::rules::FlagRules;
use crate::transcript::block_chars;

/// Reads of one file, with no edit in between, that make a `RepeatedRead`.
pub const REPEAT_READ_WARN: usize = 3;
/// Identical tool calls in one prompt, with no edit in between, that make a `Loop`.
pub const LOOP_WARN: usize = 3;
/// Tokens in one tool result that make a `LargeResult`.
pub const LARGE_RESULT_TOKENS: u64 = 10_000;

/// The pattern a flag is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    /// The same file and range read again and again with no edit in between.
    RepeatedRead,
    /// A failed tool call retried with the same input.
    FailedRetry,
    /// The same tool call made again and again in one prompt.
    Loop,
    /// One tool result that is very large.
    LargeResult,
    /// The conversation was compacted.
    Compaction,
    /// A file read again after a compaction dropped it.
    RereadAfterCompaction,
}

/// One flag: a pattern found in one run, the nodes involved and plain words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flag {
    /// The pattern found.
    pub kind: FlagKind,
    /// How much attention it deserves.
    pub level: AdviceLevel,
    /// Plain words describing it.
    pub text: String,
    /// Every node involved, in session order.
    pub trace_ids: Vec<String>,
    /// The run the flag is about.
    pub run_id: String,
}

/// Every flag in the trace, in session order of each flag's first node.
/// Each run is checked on its own, so a subagent's calls never count
/// towards its parent's patterns.
pub fn session_flags(trace: &Trace, rules: &FlagRules) -> Vec<Flag> {
    let order = node_order(trace);
    let mut flags = Vec::new();
    let mut pending: Vec<&str> = trace
        .roots()
        .filter(|r| matches!(r.node, Node::Run(_)))
        .map(|r| r.id.as_str())
        .collect();
    pending.reverse();
    while let Some(run_id) = pending.pop() {
        let mut steps = Vec::new();
        let mut nested = Vec::new();
        collect_steps(trace, run_id, None, &mut steps, &mut nested);
        pending.extend(nested.into_iter().rev());
        let run = RunCheck {
            run_id,
            steps: &steps,
            rules,
            order: &order,
        };
        run.repeated_reads(&mut flags);
        run.large_results(&mut flags);
    }
    let first = |f: &Flag| {
        f.trace_ids
            .first()
            .and_then(|id| order.get(id.as_str()))
            .copied()
            .unwrap_or(usize::MAX)
    };
    flags.sort_by(|a, b| {
        first(a)
            .cmp(&first(b))
            .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            .then_with(|| a.text.cmp(&b.text))
    });
    flags
}

/// One event of a run that a rule looks at, in session order.
enum Step<'a> {
    /// A tool call, with the turn it belongs to.
    Tool {
        id: &'a str,
        // Read by the per-turn rules.
        #[allow(dead_code)]
        turn: Option<&'a str>,
        call: &'a ToolCall,
    },
    /// A compaction marker, with its details if the harness recorded them.
    Compaction {
        // Read by the compaction rules.
        #[allow(dead_code)]
        id: &'a str,
        #[allow(dead_code)]
        details: Option<&'a Compaction>,
    },
}

/// The position of every node in a pre-order walk of the whole trace.
fn node_order(trace: &Trace) -> HashMap<&str, usize> {
    let mut order = HashMap::new();
    let mut stack: Vec<&str> = trace.roots().map(|r| r.id.as_str()).collect();
    stack.reverse();
    while let Some(id) = stack.pop() {
        let next = order.len();
        order.entry(id).or_insert(next);
        let children: Vec<&str> = trace.children(id).map(|c| c.id.as_str()).collect();
        stack.extend(children.into_iter().rev());
    }
    order
}

/// Walks the children of `id` depth first, collecting the run's steps.
/// Nested runs are collected, not entered.
fn collect_steps<'a>(
    trace: &'a Trace,
    id: &str,
    turn: Option<&'a str>,
    steps: &mut Vec<Step<'a>>,
    nested: &mut Vec<&'a str>,
) {
    for child in trace.children(id) {
        let mut child_turn = turn;
        match &child.node {
            Node::Run(_) => {
                nested.push(child.id.as_str());
                continue;
            }
            Node::Turn(_) => child_turn = Some(child.id.as_str()),
            Node::ToolCall(call) => steps.push(Step::Tool {
                id: child.id.as_str(),
                turn,
                call,
            }),
            Node::Marker(marker) if marker.kind == "compaction" => steps.push(Step::Compaction {
                id: child.id.as_str(),
                details: marker.compaction.as_ref(),
            }),
            _ => {}
        }
        collect_steps(trace, &child.id, child_turn, steps, nested);
    }
}

/// The steps of one run and what the rules need to check them.
struct RunCheck<'a, 'r> {
    run_id: &'a str,
    steps: &'r [Step<'a>],
    rules: &'r FlagRules,
    order: &'r HashMap<&'a str, usize>,
}

impl<'a> RunCheck<'a, '_> {
    fn flag(&self, kind: FlagKind, level: AdviceLevel, text: String, ids: &[&str]) -> Flag {
        Flag {
            kind,
            level,
            text,
            trace_ids: ids.iter().map(|id| id.to_string()).collect(),
            run_id: self.run_id.to_string(),
        }
    }

    fn position(&self, id: &str) -> usize {
        self.order.get(id).copied().unwrap_or(usize::MAX)
    }

    /// Reads of one path and range, with no edit of that path and no
    /// compaction in between.
    fn repeated_reads(&self, flags: &mut Vec<Flag>) {
        let mut open: HashMap<(String, String), Vec<&'a str>> = HashMap::new();
        for step in self.steps {
            match step {
                Step::Tool { id, call, .. } => {
                    if let Some(path) = edit_path(call, self.rules) {
                        let keys: Vec<(String, String)> =
                            open.keys().filter(|k| k.0 == path).cloned().collect();
                        let closed = keys
                            .into_iter()
                            .filter_map(|k| open.remove_entry(&k))
                            .collect();
                        self.close_reads(closed, flags);
                    } else if let Some(path) = read_path(call, self.rules) {
                        if !failed(call) {
                            let key = (path.to_string(), range_key(call, self.rules));
                            open.entry(key).or_default().push(id);
                        }
                    }
                }
                Step::Compaction { .. } => {
                    self.close_reads(open.drain().collect(), flags);
                }
            }
        }
        self.close_reads(open.drain().collect(), flags);
    }

    /// Emits a `RepeatedRead` for every closed series long enough, in the
    /// order of each series' first read.
    fn close_reads(
        &self,
        mut series: Vec<((String, String), Vec<&'a str>)>,
        flags: &mut Vec<Flag>,
    ) {
        series.sort_by_key(|(_, ids)| ids.first().map(|id| self.position(id)));
        for ((path, _), ids) in series {
            if ids.len() >= REPEAT_READ_WARN {
                let text = format!("`{path}` read {} times with no edit in between.", ids.len());
                flags.push(self.flag(FlagKind::RepeatedRead, AdviceLevel::Warn, text, &ids));
            }
        }
    }

    /// Tool results of `LARGE_RESULT_TOKENS` or more.
    fn large_results(&self, flags: &mut Vec<Flag>) {
        for step in self.steps {
            let Step::Tool { id, call, .. } = step else {
                continue;
            };
            let Some(tokens) = result_tokens(call) else {
                continue;
            };
            if tokens >= LARGE_RESULT_TOKENS {
                let text = format!("`{}` returned {} tokens.", call.name, about_tokens(tokens));
                flags.push(self.flag(FlagKind::LargeResult, AdviceLevel::Warn, text, &[id]));
            }
        }
    }
}

/// The path a file read reads, if `call` is one.
fn read_path<'a>(call: &'a ToolCall, rules: &FlagRules) -> Option<&'a str> {
    rules
        .file_reads
        .iter()
        .find(|r| r.tool == call.name)
        .and_then(|r| call.input.get(&r.path_field))
        .and_then(Value::as_str)
}

/// The path a file edit changes, if `call` is one. The rule's first path
/// field present as a string wins.
fn edit_path<'a>(call: &'a ToolCall, rules: &FlagRules) -> Option<&'a str> {
    let rule = rules.file_edits.iter().find(|r| r.tool == call.name)?;
    rule.path_fields
        .iter()
        .find_map(|f| call.input.get(f).and_then(Value::as_str))
}

/// The range fields present in the call's input as compact JSON, or an empty
/// string if there are none.
fn range_key(call: &ToolCall, rules: &FlagRules) -> String {
    let mut range = Map::new();
    for field in &rules.range_fields {
        if let Some(value) = call.input.get(field) {
            range.insert(field.clone(), value.clone());
        }
    }
    if range.is_empty() {
        String::new()
    } else {
        Value::Object(range).to_string()
    }
}

/// True when the call has a result and it is an error. A call still running
/// has not failed.
fn failed(call: &ToolCall) -> bool {
    call.result.as_ref().is_some_and(|r| r.is_error)
}

/// Tokens in the call's result, or `None` while it is still running.
fn result_tokens(call: &ToolCall) -> Option<u64> {
    let result = call.result.as_ref()?;
    let chars: u64 = result.content.iter().map(block_chars).sum();
    Some((chars as f64 / CHARS_PER_TOKEN).floor() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advice::AdviceLevel;
    use crate::rules::{FileEditRule, FileReadRule, FlagRules};
    use serde_json::{Value, json};
    use trace_core::{
        ContentBlock, Marker, ModelCall, Node, Run, ToolCall, ToolResult, Trace, TraceEvent, Turn,
    };

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

    fn call() -> Node {
        Node::ModelCall(ModelCall {
            model: None,
            output: Vec::new(),
            stop_reason: None,
            usage: None,
        })
    }

    fn turn() -> Node {
        Node::Turn(Turn { prompt: Vec::new() })
    }

    fn compaction() -> Node {
        Node::Marker(Marker {
            kind: "compaction".into(),
            summary: String::new(),
            compaction: None,
        })
    }

    fn rules() -> FlagRules {
        FlagRules {
            file_reads: vec![FileReadRule {
                tool: "ReadFile".into(),
                path_field: "path".into(),
            }],
            range_fields: vec!["from".into()],
            file_edits: vec![FileEditRule {
                tool: "EditFile".into(),
                path_fields: vec!["path".into()],
            }],
        }
    }

    fn tool(name: &str, input: Value, result: Option<(&str, bool)>) -> Node {
        Node::ToolCall(ToolCall {
            name: name.into(),
            input,
            result: result.map(|(t, e)| ToolResult {
                content: vec![ContentBlock::Text { text: t.into() }],
                is_error: e,
            }),
        })
    }

    fn read(path: &str) -> Node {
        tool("ReadFile", json!({ "path": path }), Some(("text", false)))
    }

    /// R > T1 > M1..Mn, with tool call Xi under model call Mi.
    fn session(tools: Vec<Node>) -> Trace {
        let mut t = Trace::new();
        add(&mut t, "R", None, run());
        add(&mut t, "T1", Some("R"), turn());
        for (i, node) in tools.into_iter().enumerate() {
            let m = format!("M{}", i + 1);
            add(&mut t, &m, Some("T1"), call());
            add(&mut t, &format!("X{}", i + 1), Some(&m), node);
        }
        t
    }

    fn kinds(flags: &[Flag]) -> Vec<FlagKind> {
        flags.iter().map(|f| f.kind).collect()
    }

    #[test]
    fn three_reads_of_one_file_are_flagged() {
        let t = session(vec![read("a.rs"), read("a.rs"), read("a.rs")]);
        let flags = session_flags(&t, &rules());
        assert_eq!(
            flags,
            vec![Flag {
                kind: FlagKind::RepeatedRead,
                level: AdviceLevel::Warn,
                text: "`a.rs` read 3 times with no edit in between.".into(),
                trace_ids: vec!["X1".into(), "X2".into(), "X3".into()],
                run_id: "R".into(),
            }]
        );
    }

    #[test]
    fn two_reads_are_not_flagged() {
        let t = session(vec![read("a.rs"), read("a.rs")]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn an_edit_between_reads_restarts_the_count() {
        let edit = tool("EditFile", json!({"path": "a.rs"}), Some(("ok", false)));
        let t = session(vec![
            read("a.rs"),
            read("a.rs"),
            edit,
            read("a.rs"),
            read("a.rs"),
        ]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn an_edit_of_another_file_does_not() {
        let edit = tool("EditFile", json!({"path": "b.rs"}), Some(("ok", false)));
        let t = session(vec![read("a.rs"), read("a.rs"), edit, read("a.rs")]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::RepeatedRead]);
        assert_eq!(flags[0].trace_ids, vec!["X1", "X2", "X4"]);
    }

    #[test]
    fn different_ranges_are_different_reads() {
        let ranged = |from: u64| {
            tool(
                "ReadFile",
                json!({"path": "a.rs", "from": from}),
                Some(("text", false)),
            )
        };
        let t = session(vec![ranged(1), ranged(1), ranged(50), ranged(50)]);
        assert!(session_flags(&t, &rules()).is_empty());
        let t = session(vec![ranged(1), ranged(1), ranged(1)]);
        assert_eq!(
            kinds(&session_flags(&t, &rules())),
            vec![FlagKind::RepeatedRead]
        );
    }

    #[test]
    fn failed_reads_do_not_count() {
        let failed = tool("ReadFile", json!({"path": "a.rs"}), Some(("nope", true)));
        let t = session(vec![read("a.rs"), failed, read("a.rs")]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn running_reads_count() {
        let running = tool("ReadFile", json!({"path": "a.rs"}), None);
        let t = session(vec![read("a.rs"), read("a.rs"), running]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::RepeatedRead]);
        assert_eq!(flags[0].trace_ids, vec!["X1", "X2", "X3"]);
    }

    #[test]
    fn a_compaction_restarts_the_count() {
        let mut t = session(vec![read("a.rs"), read("a.rs")]);
        add(&mut t, "C", Some("T1"), compaction());
        add(&mut t, "M3", Some("T1"), call());
        add(&mut t, "X3", Some("M3"), read("a.rs"));
        let flags = session_flags(&t, &rules());
        assert!(
            !kinds(&flags).contains(&FlagKind::RepeatedRead),
            "{flags:#?}"
        );
    }

    #[test]
    fn runs_are_checked_separately() {
        let mut t = session(vec![read("a.rs"), read("a.rs")]);
        add(&mut t, "S", Some("X1"), run());
        add(&mut t, "S1", Some("S"), call());
        add(&mut t, "SX1", Some("S1"), read("a.rs"));
        add(&mut t, "SX2", Some("S1"), read("a.rs"));
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn large_result_is_flagged() {
        let big = "x".repeat(40_000);
        let t = session(vec![tool(
            "ReadFile",
            json!({"path": "a.rs"}),
            Some((&big, false)),
        )]);
        let flags = session_flags(&t, &rules());
        assert_eq!(
            flags,
            vec![Flag {
                kind: FlagKind::LargeResult,
                level: AdviceLevel::Warn,
                text: "`ReadFile` returned about 10k tokens.".into(),
                trace_ids: vec!["X1".into()],
                run_id: "R".into(),
            }]
        );
        let under = "x".repeat(39_996);
        let t = session(vec![tool(
            "ReadFile",
            json!({"path": "a.rs"}),
            Some((&under, false)),
        )]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn pending_result_has_no_size() {
        let t = session(vec![tool("ReadFile", json!({"path": "a.rs"}), None)]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn flags_are_in_session_order() {
        let big = "x".repeat(40_000);
        let t = session(vec![
            tool("Search", json!({"q": "x"}), Some((&big, false))),
            read("a.rs"),
            read("a.rs"),
            read("a.rs"),
        ]);
        let flags = session_flags(&t, &rules());
        assert_eq!(
            kinds(&flags),
            vec![FlagKind::LargeResult, FlagKind::RepeatedRead]
        );
        assert_eq!(flags[1].trace_ids, vec!["X2", "X3", "X4"]);
    }
}
