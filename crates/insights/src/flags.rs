//! Waste and problem flags: patterns in a trace worth a second look, such as
//! the same file read again and again or a tool result that fills the context.
//! Each run (the main agent and each subagent) is checked on its own. Harness
//! rules come in as `FlagRules`.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value};
use trace_core::{Compaction, Node, ToolCall, Trace};

use crate::advice::{AdviceLevel, about_tokens, plural};
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
        run.failed_retries(&mut flags);
        run.loops(&mut flags);
        run.compactions(&mut flags);
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
        turn: Option<&'a str>,
        call: &'a ToolCall,
    },
    /// A compaction marker, with its details if the harness recorded them.
    Compaction {
        id: &'a str,
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
                    // A failed edit still counts as an edit, on purpose.
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

    /// A failed call followed by calls with the same name and an equal input.
    /// A series ends at the first of them that works. Calls still running
    /// are left out.
    fn failed_retries(&self, flags: &mut Vec<Flag>) {
        // `Value` is not `Hash`, so series are searched linearly.
        let mut open: Vec<(&'a ToolCall, Vec<&'a str>)> = Vec::new();
        for step in self.steps {
            let Step::Tool { id, call, .. } = step else {
                continue;
            };
            let Some(result) = &call.result else {
                continue;
            };
            if let Some(i) = open.iter().position(|(c, _)| same_call(c, call)) {
                open[i].1.push(id);
                if !result.is_error {
                    let (first, ids) = open.remove(i);
                    self.close_retry(first, &ids, false, flags);
                }
            } else if result.is_error {
                open.push((call, vec![id]));
            }
        }
        for (first, ids) in open {
            self.close_retry(first, &ids, true, flags);
        }
    }

    fn close_retry(&self, call: &ToolCall, ids: &[&str], last_failed: bool, flags: &mut Vec<Flag>) {
        if ids.len() < 2 {
            return;
        }
        let retries = (ids.len() - 1) as u32;
        let name = &call.name;
        let (level, text) = if !last_failed {
            (
                AdviceLevel::Info,
                format!(
                    "`{name}` failed and was retried {} with the same input; the last try worked.",
                    plural(retries, "time")
                ),
            )
        } else if retries == 1 {
            (
                AdviceLevel::Warn,
                format!(
                    "`{name}` failed and was retried 1 time with the same input; it failed again."
                ),
            )
        } else {
            (
                AdviceLevel::Warn,
                format!(
                    "`{name}` failed and was retried {retries} times with the same input; it failed every time."
                ),
            )
        };
        flags.push(self.flag(FlagKind::FailedRetry, level, text, ids));
    }

    /// The same call made again and again in one turn with no file edit in
    /// between. Reads are left to `repeated_reads`, and a series with a
    /// failed call is left to `failed_retries`.
    fn loops(&self, flags: &mut Vec<Flag>) {
        let mut open: Vec<LoopSeries<'a>> = Vec::new();
        let mut current_turn: Option<&str> = None;
        for step in self.steps {
            match step {
                Step::Tool { id, turn, call } => {
                    if *turn != current_turn {
                        self.close_loops(std::mem::take(&mut open), flags);
                        current_turn = *turn;
                    }
                    // A failed edit still counts as an edit, on purpose.
                    if edit_path(call, self.rules).is_some() {
                        self.close_loops(std::mem::take(&mut open), flags);
                        continue;
                    }
                    if read_path(call, self.rules).is_some() {
                        continue;
                    }
                    if let Some(s) = open.iter_mut().find(|s| same_call(s.call, call)) {
                        s.ids.push(id);
                        s.any_failed |= failed(call);
                    } else {
                        open.push(LoopSeries {
                            call,
                            ids: vec![id],
                            any_failed: failed(call),
                        });
                    }
                }
                Step::Compaction { .. } => {
                    self.close_loops(std::mem::take(&mut open), flags);
                }
            }
        }
        self.close_loops(open, flags);
    }

    fn close_loops(&self, series: Vec<LoopSeries<'a>>, flags: &mut Vec<Flag>) {
        for s in series {
            if s.ids.len() >= LOOP_WARN && !s.any_failed {
                let text = format!(
                    "The same `{}` call ran {} times in this prompt with no file edit in between.",
                    s.call.name,
                    s.ids.len()
                );
                flags.push(self.flag(FlagKind::Loop, AdviceLevel::Warn, text, &s.ids));
            }
        }
    }

    /// One flag per compaction marker, and a flag for the first read of each
    /// file a compaction dropped.
    fn compactions(&self, flags: &mut Vec<Flag>) {
        // Distinct paths read since the last compaction, plus the files it
        // restored.
        let mut in_context: Vec<String> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        let none = Compaction::default();
        for step in self.steps {
            match step {
                Step::Tool { id, call, .. } => {
                    let Some(path) = read_path(call, self.rules) else {
                        continue;
                    };
                    if failed(call) {
                        continue;
                    }
                    if let Some(i) = dropped.iter().position(|p| p == path) {
                        dropped.remove(i);
                        let text = format!("`{path}` was read again after compaction dropped it.");
                        flags.push(self.flag(
                            FlagKind::RereadAfterCompaction,
                            AdviceLevel::Warn,
                            text,
                            &[id],
                        ));
                    }
                    if !in_context.iter().any(|p| p == path) {
                        in_context.push(path.to_string());
                    }
                }
                Step::Compaction { id, details } => {
                    let d = details.unwrap_or(&none);
                    let restored = &d.files_restored;
                    let mentioned =
                        |p: &String| restored.contains(p) || d.files_referenced.contains(p);
                    let not_mentioned = in_context.iter().filter(|p| !mentioned(p)).count();
                    for p in in_context.drain(..) {
                        if !dropped.contains(&p) {
                            dropped.push(p);
                        }
                    }
                    dropped.retain(|p| !restored.contains(p));
                    in_context = restored.clone();
                    let text = compaction_text(d, not_mentioned);
                    flags.push(self.flag(FlagKind::Compaction, AdviceLevel::Info, text, &[id]));
                }
            }
        }
    }
}

/// Identical calls in one turn with no file edit in between.
struct LoopSeries<'a> {
    call: &'a ToolCall,
    ids: Vec<&'a str>,
    any_failed: bool,
}

/// True when two calls have the same name and an equal input.
fn same_call(a: &ToolCall, b: &ToolCall) -> bool {
    a.name == b.name && a.input == b.input
}

/// Plain words for one compaction, leaving out what is not known.
fn compaction_text(d: &Compaction, not_mentioned: usize) -> String {
    let mut sentences = Vec::new();
    sentences.push(match (d.tokens_before, d.tokens_after) {
        (Some(before), Some(after)) => format!(
            "Compaction dropped {} tokens (kept {}).",
            about_tokens(before.saturating_sub(after)),
            about_tokens(after)
        ),
        (Some(before), None) => format!("Compaction ran at {} tokens.", about_tokens(before)),
        _ => "Context was compacted.".to_string(),
    });
    let restored = d.files_restored.len() as u32;
    let referenced = d.files_referenced.len() as u32;
    match (restored, referenced) {
        (0, 0) => {}
        (r, 0) => sentences.push(format!("Re-attached {} in full.", plural(r, "file"))),
        (0, f) => sentences.push(format!("Re-attached {} by name only.", plural(f, "file"))),
        (r, f) => sentences.push(format!(
            "Re-attached {} in full and {f} by name only.",
            plural(r, "file")
        )),
    }
    if not_mentioned > 0 {
        let verb = if not_mentioned == 1 { "was" } else { "were" };
        sentences.push(format!(
            "{} read earlier {verb} not re-attached.",
            plural(not_mentioned as u32, "file")
        ));
    }
    sentences.join(" ")
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
        Compaction, ContentBlock, Marker, ModelCall, Node, Run, ToolCall, ToolResult, Trace,
        TraceEvent, Turn,
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
        // The failed read and its working retry are a `FailedRetry`, but
        // not a `RepeatedRead`.
        assert_eq!(
            kinds(&session_flags(&t, &rules())),
            vec![FlagKind::FailedRetry]
        );
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

    #[test]
    fn edit_path_falls_back_to_a_later_field() {
        let mut r = rules();
        r.file_edits = vec![FileEditRule {
            tool: "EditFile".into(),
            path_fields: vec!["a".into(), "b".into()],
        }];
        let edit = tool("EditFile", json!({"b": "x.rs"}), Some(("ok", false)));
        let t = session(vec![read("x.rs"), read("x.rs"), edit, read("x.rs")]);
        assert!(session_flags(&t, &r).is_empty());
    }

    fn bash(command: &str, result: Option<(&str, bool)>) -> Node {
        tool("Bash", json!({ "command": command }), result)
    }

    fn of_kind(flags: &[Flag], kind: FlagKind) -> Vec<&Flag> {
        flags.iter().filter(|f| f.kind == kind).collect()
    }

    #[test]
    fn failed_then_same_failure_is_a_warn() {
        let t = session(vec![
            bash("make", Some(("err", true))),
            bash("make", Some(("err", true))),
        ]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::FailedRetry]);
        assert_eq!(flags[0].level, AdviceLevel::Warn);
        assert_eq!(flags[0].trace_ids, vec!["X1", "X2"]);
        assert!(
            flags[0].text.ends_with("it failed again."),
            "{}",
            flags[0].text
        );
    }

    #[test]
    fn failed_three_times_says_every_time() {
        let t = session(vec![
            bash("make", Some(("err", true))),
            bash("make", Some(("err", true))),
            bash("make", Some(("err", true))),
        ]);
        let flags = session_flags(&t, &rules());
        let retries = of_kind(&flags, FlagKind::FailedRetry);
        assert_eq!(retries.len(), 1);
        assert_eq!(
            retries[0].text,
            "`Bash` failed and was retried 2 times with the same input; it failed every time."
        );
        assert_eq!(retries[0].trace_ids, vec!["X1", "X2", "X3"]);
    }

    #[test]
    fn retry_that_works_is_info() {
        let t = session(vec![
            bash("make", Some(("err", true))),
            bash("make", Some(("ok", false))),
            bash("make", Some(("ok", false))),
        ]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::FailedRetry]);
        assert_eq!(flags[0].level, AdviceLevel::Info);
        assert_eq!(
            flags[0].text,
            "`Bash` failed and was retried 1 time with the same input; the last try worked."
        );
        assert_eq!(flags[0].trace_ids, vec!["X1", "X2"]);
    }

    #[test]
    fn different_input_is_not_a_retry() {
        let t = session(vec![
            bash("make", Some(("err", true))),
            bash("make test", Some(("ok", false))),
        ]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn inputs_compare_as_json_values() {
        let second: Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let t = session(vec![
            tool("Bash", json!({"a": 1, "b": 2}), Some(("err", true))),
            tool("Bash", second, Some(("err", true))),
        ]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::FailedRetry]);
    }

    #[test]
    fn pending_calls_are_ignored_by_retries() {
        let t = session(vec![bash("make", Some(("err", true))), bash("make", None)]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn same_call_three_times_is_a_loop() {
        let ok = || bash("ls", Some(("ok", false)));
        let t = session(vec![ok(), ok(), ok()]);
        let flags = session_flags(&t, &rules());
        assert_eq!(
            flags,
            vec![Flag {
                kind: FlagKind::Loop,
                level: AdviceLevel::Warn,
                text:
                    "The same `Bash` call ran 3 times in this prompt with no file edit in between."
                        .into(),
                trace_ids: vec!["X1".into(), "X2".into(), "X3".into()],
                run_id: "R".into(),
            }]
        );
    }

    #[test]
    fn loop_needs_one_turn() {
        let mut t = session(vec![
            bash("ls", Some(("ok", false))),
            bash("ls", Some(("ok", false))),
        ]);
        add(&mut t, "T2", Some("R"), turn());
        add(&mut t, "M3", Some("T2"), call());
        add(&mut t, "X3", Some("M3"), bash("ls", Some(("ok", false))));
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn an_edit_breaks_a_loop() {
        let ok = || bash("ls", Some(("ok", false)));
        let edit = tool("EditFile", json!({"path": "x"}), Some(("ok", false)));
        let t = session(vec![ok(), ok(), edit, ok()]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn a_failed_edit_still_breaks_a_loop() {
        let ok = || bash("ls", Some(("ok", false)));
        let edit = tool("EditFile", json!({"path": "x"}), Some(("err", true)));
        let t = session(vec![ok(), ok(), edit, ok()]);
        assert!(session_flags(&t, &rules()).is_empty());
    }

    #[test]
    fn a_loop_with_a_failure_is_not_a_loop() {
        let t = session(vec![
            bash("ls", Some(("ok", false))),
            bash("ls", Some(("err", true))),
            bash("ls", Some(("ok", false))),
        ]);
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::FailedRetry]);
        assert_eq!(flags[0].level, AdviceLevel::Info);
        assert_eq!(flags[0].trace_ids, vec!["X2", "X3"]);
    }

    #[test]
    fn reads_are_not_loops() {
        let t = session(vec![read("a.rs"), read("a.rs"), read("a.rs")]);
        assert_eq!(
            kinds(&session_flags(&t, &rules())),
            vec![FlagKind::RepeatedRead]
        );
    }

    fn compaction_with(details: Compaction) -> Node {
        Node::Marker(Marker {
            kind: "compaction".into(),
            summary: String::new(),
            compaction: Some(details),
        })
    }

    /// Reads of a.rs, b.rs, c.rs, then compaction `C` that restores a.rs
    /// in full and names b.rs.
    fn compacted_session() -> Trace {
        let mut t = session(vec![read("a.rs"), read("b.rs"), read("c.rs")]);
        let details = Compaction {
            tokens_before: Some(380_389),
            tokens_after: Some(19_037),
            files_restored: vec!["a.rs".into()],
            files_referenced: vec!["b.rs".into()],
        };
        add(&mut t, "C", Some("T1"), compaction_with(details));
        t
    }

    #[test]
    fn compaction_flag_text() {
        let flags = session_flags(&compacted_session(), &rules());
        assert_eq!(
            flags,
            vec![Flag {
                kind: FlagKind::Compaction,
                level: AdviceLevel::Info,
                text: "Compaction dropped about 361k tokens (kept about 19k). Re-attached 1 file in full and 1 by name only. 1 file read earlier was not re-attached."
                    .into(),
                trace_ids: vec!["C".into()],
                run_id: "R".into(),
            }]
        );
    }

    #[test]
    fn compaction_without_details() {
        let mut t = Trace::new();
        add(&mut t, "R", None, run());
        add(&mut t, "T1", Some("R"), turn());
        add(&mut t, "C", Some("T1"), compaction());
        let flags = session_flags(&t, &rules());
        assert_eq!(kinds(&flags), vec![FlagKind::Compaction]);
        assert_eq!(flags[0].text, "Context was compacted.");
        assert_eq!(flags[0].trace_ids, vec!["C"]);
    }

    #[test]
    fn compaction_sentences_without_some_details() {
        let mut t = Trace::new();
        add(&mut t, "R", None, run());
        add(&mut t, "T1", Some("R"), turn());
        let only_before = Compaction {
            tokens_before: Some(5_000),
            files_referenced: vec!["x".into(), "y".into()],
            ..Compaction::default()
        };
        add(&mut t, "C", Some("T1"), compaction_with(only_before));
        let flags = session_flags(&t, &rules());
        assert_eq!(
            flags[0].text,
            "Compaction ran at about 5k tokens. Re-attached 2 files by name only."
        );
    }

    #[test]
    fn reread_after_compaction_is_flagged_once() {
        let mut t = compacted_session();
        for (i, path) in ["b.rs", "c.rs", "a.rs", "b.rs"].into_iter().enumerate() {
            let m = format!("M{}", i + 4);
            add(&mut t, &m, Some("T1"), call());
            add(&mut t, &format!("X{}", i + 4), Some(&m), read(path));
        }
        let flags = session_flags(&t, &rules());
        let rereads = of_kind(&flags, FlagKind::RereadAfterCompaction);
        assert_eq!(rereads.len(), 2, "{flags:#?}");
        assert_eq!(rereads[0].trace_ids, vec!["X4"]);
        assert_eq!(
            rereads[0].text,
            "`b.rs` was read again after compaction dropped it."
        );
        assert_eq!(rereads[0].level, AdviceLevel::Warn);
        assert_eq!(rereads[1].trace_ids, vec!["X5"]);
    }

    #[test]
    fn dropped_files_carry_over_a_second_compaction() {
        let mut t = session(vec![read("a.rs")]);
        add(&mut t, "C1", Some("T1"), compaction());
        add(&mut t, "C2", Some("T1"), compaction());
        add(&mut t, "M2", Some("T1"), call());
        add(&mut t, "X2", Some("M2"), read("a.rs"));
        let flags = session_flags(&t, &rules());
        let rereads = of_kind(&flags, FlagKind::RereadAfterCompaction);
        assert_eq!(rereads.len(), 1, "{flags:#?}");
        assert_eq!(rereads[0].trace_ids, vec!["X2"]);
    }
}
