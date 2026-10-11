//! Comparing two runs: one summary per run (setup, work, tokens, problems,
//! outcome), and two summaries side by side as rows the UI shows as they
//! are. Nothing here knows which harness produced the trace.

use std::collections::HashSet;

use serde::Serialize;
use trace_core::{ContentBlock, Node, Trace, TraceEvent};

use crate::breakdown::ContextBreakdown;
use crate::flags::{Flag, FlagKind};
use crate::measure::{ContextSource, SliceKind};

/// Every flag kind, in the order the comparison lists them.
pub const FLAG_KINDS: [FlagKind; 6] = [
    FlagKind::RepeatedRead,
    FlagKind::FailedRetry,
    FlagKind::Loop,
    FlagKind::LargeResult,
    FlagKind::Compaction,
    FlagKind::RereadAfterCompaction,
];

/// What part of a trace a summary covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The whole trace: the main run and every run inside it.
    Session,
    /// One turn of the main run, by trace id, and every run started inside it.
    Prompt(String),
}

/// A sum over calls where some values may be unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Total {
    /// The sum of the known values, `None` if no value was known.
    pub value: Option<u64>,
    /// True when some values were known and some were not.
    pub partial: bool,
}

/// A name with a number: calls per model, tools per group, and so on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Named {
    /// The name.
    pub name: String,
    /// Its number.
    pub value: u64,
}

/// One model call of the main run, for the context chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallPoint {
    /// Position among the main run's calls in scope, from 1.
    pub index: u32,
    /// The model call's trace id.
    pub trace_id: String,
    /// Its reported input context, if known.
    pub tokens: Option<u64>,
}

/// How many flags of one kind a run has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KindCount {
    /// The flag kind.
    pub kind: FlagKind,
    /// Flags of that kind whose first node is in scope.
    pub count: u64,
}

/// One run reduced to the numbers a comparison shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    /// Where the first call's context breakdown came from, if there is one.
    pub first_call_source: Option<ContextSource>,
    /// The first main-run call's input context.
    pub first_call_tokens: Option<u64>,
    /// System prompt tokens of the first call; `None` if not seen.
    pub system_prompt_tokens: Option<u64>,
    /// Tool definition tokens of the first call; `None` if not seen.
    pub tool_definition_tokens: Option<u64>,
    /// Instruction and reminder tokens of the first call; `None` if not seen.
    pub instruction_tokens: Option<u64>,
    /// The rest of the first call's context that was seen: files read, tool
    /// results and conversation. `None` without a first-call breakdown,
    /// `Some(0)` when a breakdown has none of these.
    pub other_context_tokens: Option<u64>,
    /// Context of the first call that could not be seen. `None` without a
    /// first-call breakdown, `Some(0)` when the breakdown has no such slice.
    pub not_captured_tokens: Option<u64>,
    /// Tools defined in the first call; `None` if not seen.
    pub tool_count: Option<u64>,
    /// Tools per group in the first call; `None` if not seen.
    pub tool_groups: Option<Vec<Named>>,
    /// Instruction parts of the first call with their tokens; `None` if not seen.
    pub instruction_parts: Option<Vec<Named>>,
    /// Model calls per model name, in first-seen order.
    pub models: Vec<Named>,
    /// Prompts (turns of the main run).
    pub prompts: u64,
    /// Model calls in every run.
    pub model_calls: u64,
    /// Tool calls in every run.
    pub tool_calls: u64,
    /// Subagent runs.
    pub subagents: u64,
    /// Tool calls per tool name, in first-seen order.
    pub tools_by_name: Vec<Named>,
    /// From the first start to the last end in scope.
    pub wall_ms: Option<u64>,
    /// Summed model call durations.
    pub model_ms: Total,
    /// Summed tool call durations.
    pub tool_ms: Total,
    /// Uncached input tokens over every model call.
    pub input_tokens: Total,
    /// Output tokens over every model call.
    pub output_tokens: Total,
    /// Cache read tokens over every model call.
    pub cache_read_tokens: Total,
    /// Cache write tokens over every model call.
    pub cache_write_tokens: Total,
    /// The largest reported context of one main-run call.
    pub largest_context: Option<u64>,
    /// The main run's calls in order with their context.
    pub context_per_call: Vec<CallPoint>,
    /// Flag counts for every kind in `FLAG_KINDS`, in that order.
    pub flags_by_kind: Vec<KindCount>,
    /// Tool calls whose result is an error.
    pub failed_tool_calls: u64,
    /// Compaction markers.
    pub compactions: u64,
    /// The text of the last main-run model call that wrote text.
    pub final_message: Option<String>,
}

/// The first root node that is a run.
fn main_run(trace: &Trace) -> Option<&str> {
    trace
        .roots()
        .find(|r| matches!(r.node, Node::Run(_)))
        .map(|r| r.id.as_str())
}

/// True when `turn_id` is a turn whose parent is the main run.
pub fn prompt_in_main_run(trace: &Trace, turn_id: &str) -> bool {
    let Some(event) = trace.get(turn_id) else {
        return false;
    };
    matches!(event.node, Node::Turn(_)) && event.parent_id.as_deref() == main_run(trace)
}

/// The first main-run model call in scope, for the setup breakdown.
pub fn first_main_call(trace: &Trace, scope: &Scope) -> Option<String> {
    walk(trace, scope)
        .main_calls
        .first()
        .map(|p| p.trace_id.clone())
}

/// Sums over values that may be unknown.
#[derive(Default)]
struct Sum {
    sum: u64,
    known: bool,
    unknown: bool,
}

impl Sum {
    fn add(&mut self, value: Option<u64>) {
        match value {
            Some(v) => {
                self.sum = self.sum.saturating_add(v);
                self.known = true;
            }
            None => self.unknown = true,
        }
    }

    fn total(&self) -> Total {
        Total {
            value: self.known.then_some(self.sum),
            partial: self.known && self.unknown,
        }
    }
}

/// Everything collected in one pass over the scope.
#[derive(Default)]
struct Walk<'a> {
    ids: HashSet<&'a str>,
    first_start: Option<i64>,
    last_end: Option<i64>,
    models: Vec<Named>,
    prompts: u64,
    model_calls: u64,
    tool_calls: u64,
    subagents: u64,
    tools_by_name: Vec<Named>,
    model_ms: Sum,
    tool_ms: Sum,
    input: Sum,
    output: Sum,
    cache_read: Sum,
    cache_write: Sum,
    main_calls: Vec<CallPoint>,
    failed: u64,
    compactions: u64,
    final_message: Option<String>,
}

/// Adds `by` to the entry `name`, keeping first-seen order.
fn bump(list: &mut Vec<Named>, name: &str, by: u64) {
    match list.iter_mut().find(|n| n.name == name) {
        Some(n) => n.value += by,
        None => list.push(Named {
            name: name.to_string(),
            value: by,
        }),
    }
}

/// The node's duration when both ends are known and in order.
fn duration(event: &TraceEvent) -> Option<u64> {
    let (start, end) = (event.started_at_ms?, event.ended_at_ms?);
    u64::try_from(end.checked_sub(start)?).ok()
}

/// The text blocks of a model call's output, joined, or `None` if empty.
fn output_text(blocks: &[ContentBlock]) -> Option<String> {
    let text = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// Walks the scope once.
fn walk<'a>(trace: &'a Trace, scope: &Scope) -> Walk<'a> {
    let mut w = Walk::default();
    match scope {
        Scope::Session => {
            let main = main_run(trace);
            for root in trace.roots() {
                w.visit(trace, root, Some(root.id.as_str()) == main);
            }
        }
        Scope::Prompt(id) => {
            if prompt_in_main_run(trace, id) {
                if let Some(turn) = trace.get(id) {
                    w.visit(trace, turn, true);
                }
            }
        }
    }
    w
}

impl<'a> Walk<'a> {
    /// Records `event` and everything under it. `main` is true while no
    /// subagent run lies between the scope's start and `event`.
    fn visit(&mut self, trace: &'a Trace, event: &'a TraceEvent, main: bool) {
        self.ids.insert(event.id.as_str());
        if let Some(start) = event.started_at_ms {
            self.first_start = Some(self.first_start.map_or(start, |s| s.min(start)));
        }
        if let Some(end) = event.ended_at_ms.or(event.started_at_ms) {
            self.last_end = Some(self.last_end.map_or(end, |e| e.max(end)));
        }
        let mut child_main = main;
        match &event.node {
            Node::Run(_) if event.parent_id.is_some() => {
                self.subagents += 1;
                child_main = false;
            }
            Node::Turn(_) if main => self.prompts += 1,
            Node::ModelCall(call) => {
                self.model_calls += 1;
                bump(
                    &mut self.models,
                    call.model.as_deref().unwrap_or("unknown model"),
                    1,
                );
                let usage = call.usage.unwrap_or_default();
                self.input.add(usage.input_tokens);
                self.output.add(usage.output_tokens);
                self.cache_read.add(usage.cache_read_tokens);
                self.cache_write.add(usage.cache_write_tokens);
                self.model_ms.add(duration(event));
                if main {
                    let index = u32::try_from(self.main_calls.len() + 1).unwrap_or(u32::MAX);
                    self.main_calls.push(CallPoint {
                        index,
                        trace_id: event.id.clone(),
                        tokens: usage.context_tokens(),
                    });
                    if let Some(text) = output_text(&call.output) {
                        self.final_message = Some(text);
                    }
                }
            }
            Node::ToolCall(call) => {
                self.tool_calls += 1;
                bump(&mut self.tools_by_name, &call.name, 1);
                self.tool_ms.add(duration(event));
                if call.result.as_ref().is_some_and(|r| r.is_error) {
                    self.failed += 1;
                }
            }
            Node::Marker(marker) if marker.kind == "compaction" => self.compactions += 1,
            _ => {}
        }
        for child in trace.children(&event.id) {
            self.visit(trace, child, child_main);
        }
    }
}

/// Tokens of one slice of the breakdown, `None` if the slice is absent.
fn slice_tokens(b: &ContextBreakdown, kind: SliceKind) -> Option<u64> {
    b.slices.iter().find(|s| s.kind == kind).map(|s| s.tokens)
}

/// The named items of one slice, `None` if the slice is absent.
fn slice_items(b: &ContextBreakdown, kind: SliceKind, by_count: bool) -> Option<Vec<Named>> {
    let slice = b.slices.iter().find(|s| s.kind == kind)?;
    Some(
        slice
            .items
            .iter()
            .map(|i| Named {
                name: i.label.clone(),
                value: if by_count {
                    u64::from(i.count)
                } else {
                    i.tokens
                },
            })
            .collect(),
    )
}

/// One run reduced to the numbers a comparison shows. `first_call` is the
/// breakdown of `first_main_call(trace, scope)`, if the caller has one;
/// `flags` are the trace's flags, counted when their first node is in scope.
pub fn run_summary(
    trace: &Trace,
    scope: &Scope,
    first_call: Option<&ContextBreakdown>,
    flags: &[Flag],
) -> RunSummary {
    let w = walk(trace, scope);
    let system = first_call.and_then(|b| slice_tokens(b, SliceKind::SystemPrompt));
    let tools = first_call.and_then(|b| slice_tokens(b, SliceKind::ToolDefinitions));
    let instructions = first_call.and_then(|b| slice_tokens(b, SliceKind::Instructions));
    let other = first_call.map(|b| {
        [
            SliceKind::FilesRead,
            SliceKind::ToolResults,
            SliceKind::Conversation,
        ]
        .iter()
        .filter_map(|&kind| slice_tokens(b, kind))
        .fold(0u64, u64::saturating_add)
    });
    let not_captured =
        first_call.map(|b| slice_tokens(b, SliceKind::NotCaptured).unwrap_or(0));
    let tool_groups = first_call.and_then(|b| slice_items(b, SliceKind::ToolDefinitions, true));
    let flags_by_kind = FLAG_KINDS
        .iter()
        .map(|&kind| KindCount {
            kind,
            count: flags
                .iter()
                .filter(|f| f.kind == kind)
                .filter(|f| {
                    f.trace_ids
                        .first()
                        .is_some_and(|id| w.ids.contains(id.as_str()))
                })
                .count() as u64,
        })
        .collect();
    let wall_ms = match (w.first_start, w.last_end) {
        (Some(start), Some(end)) => u64::try_from(end.saturating_sub(start)).ok(),
        _ => None,
    };
    RunSummary {
        first_call_source: first_call.map(|b| b.source),
        first_call_tokens: first_call.map(|b| b.total_tokens),
        system_prompt_tokens: system,
        tool_definition_tokens: tools,
        instruction_tokens: instructions,
        other_context_tokens: other,
        not_captured_tokens: not_captured,
        tool_count: tool_groups
            .as_ref()
            .map(|g| g.iter().map(|n| n.value).sum()),
        tool_groups,
        instruction_parts: first_call.and_then(|b| slice_items(b, SliceKind::Instructions, false)),
        models: w.models,
        prompts: w.prompts,
        model_calls: w.model_calls,
        tool_calls: w.tool_calls,
        subagents: w.subagents,
        tools_by_name: w.tools_by_name,
        wall_ms,
        model_ms: w.model_ms.total(),
        tool_ms: w.tool_ms.total(),
        input_tokens: w.input.total(),
        output_tokens: w.output.total(),
        cache_read_tokens: w.cache_read.total(),
        cache_write_tokens: w.cache_write.total(),
        largest_context: w.main_calls.iter().filter_map(|p| p.tokens).max(),
        context_per_call: w.main_calls,
        flags_by_kind,
        failed_tool_calls: w.failed,
        compactions: w.compactions,
        final_message: w.final_message,
    }
}

/// What a row's numbers measure, for formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A plain count.
    Count,
    /// Tokens.
    Tokens,
    /// Milliseconds.
    Ms,
}

/// Which side of a row has the larger number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Higher {
    /// Run A.
    A,
    /// Run B.
    B,
    /// Both the same.
    Equal,
    /// At least one side is unknown.
    Unknown,
}

/// Whether a named row's name appears in both runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// In both runs, or not known for one of them.
    Both,
    /// Only in run A.
    OnlyA,
    /// Only in run B.
    OnlyB,
}

/// One line of a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    /// What the row is.
    pub label: String,
    /// What the numbers measure.
    pub unit: Unit,
    /// Run A's value, `None` if unknown.
    pub a: Option<u64>,
    /// Run B's value, `None` if unknown.
    pub b: Option<u64>,
    /// True when run A's value counts only some calls.
    pub a_partial: bool,
    /// True when run B's value counts only some calls.
    pub b_partial: bool,
    /// `b - a`, when both are known.
    pub difference: Option<i64>,
    /// Which side is larger.
    pub higher: Higher,
    /// For named rows, which runs have the name.
    pub presence: Presence,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    /// The section title.
    pub title: String,
    /// Its rows, in display order.
    pub rows: Vec<Row>,
}

/// Two runs side by side.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// Run A.
    pub a: RunSummary,
    /// Run B.
    pub b: RunSummary,
    /// The rows to show, in sections.
    pub sections: Vec<Section>,
}

fn difference(a: u64, b: u64) -> i64 {
    if b >= a {
        i64::try_from(b - a).unwrap_or(i64::MAX)
    } else {
        i64::try_from(a - b).map(|d| -d).unwrap_or(i64::MIN)
    }
}

fn row(label: &str, unit: Unit, a: Total, b: Total) -> Row {
    let (difference, higher) = match (a.value, b.value) {
        (Some(x), Some(y)) => (
            Some(difference(x, y)),
            match x.cmp(&y) {
                std::cmp::Ordering::Greater => Higher::A,
                std::cmp::Ordering::Less => Higher::B,
                std::cmp::Ordering::Equal => Higher::Equal,
            },
        ),
        _ => (None, Higher::Unknown),
    };
    Row {
        label: label.to_string(),
        unit,
        a: a.value,
        b: b.value,
        a_partial: a.partial,
        b_partial: b.partial,
        difference,
        higher,
        presence: Presence::Both,
    }
}

fn known(value: Option<u64>) -> Total {
    Total {
        value,
        partial: false,
    }
}

fn count(value: u64) -> Total {
    known(Some(value))
}

fn named_rows(unit: Unit, a: Option<&[Named]>, b: Option<&[Named]>) -> Vec<Row> {
    let mut names: Vec<&str> = Vec::new();
    for n in a.into_iter().flatten().chain(b.into_iter().flatten()) {
        if !names.contains(&n.name.as_str()) {
            names.push(n.name.as_str());
        }
    }
    let value = |list: Option<&[Named]>, name: &str| {
        list.map(|l| l.iter().find(|n| n.name == name).map_or(0, |n| n.value))
    };
    let has =
        |list: Option<&[Named]>, name: &str| list.is_some_and(|l| l.iter().any(|n| n.name == name));
    names
        .into_iter()
        .map(|name| {
            let mut r = row(name, unit, known(value(a, name)), known(value(b, name)));
            if a.is_some() && b.is_some() {
                r.presence = match (has(a, name), has(b, name)) {
                    (true, false) => Presence::OnlyA,
                    (false, true) => Presence::OnlyB,
                    _ => Presence::Both,
                };
            }
            r
        })
        .collect()
}

fn flag_count(s: &RunSummary, kind: FlagKind) -> u64 {
    s.flags_by_kind
        .iter()
        .find(|c| c.kind == kind)
        .map_or(0, |c| c.count)
}

fn section(title: &str, rows: Vec<Row>) -> Section {
    Section {
        title: title.to_string(),
        rows,
    }
}

/// Two runs side by side, as sections of rows.
pub fn compare(a: RunSummary, b: RunSummary) -> Comparison {
    use Unit::{Count, Ms, Tokens};
    let flag_rows = [
        ("Repeated reads", FlagKind::RepeatedRead),
        ("Failed retries", FlagKind::FailedRetry),
        ("Loops", FlagKind::Loop),
        ("Large results", FlagKind::LargeResult),
        ("Re-reads after compaction", FlagKind::RereadAfterCompaction),
    ];
    let mut problems: Vec<Row> = flag_rows
        .iter()
        .map(|&(label, kind)| {
            row(
                label,
                Count,
                count(flag_count(&a, kind)),
                count(flag_count(&b, kind)),
            )
        })
        .collect();
    problems.push(row(
        "Failed tool calls",
        Count,
        count(a.failed_tool_calls),
        count(b.failed_tool_calls),
    ));
    problems.push(row(
        "Compactions",
        Count,
        count(a.compactions),
        count(b.compactions),
    ));
    let sections = vec![
        section(
            "Setup",
            vec![
                row(
                    "First call context",
                    Tokens,
                    known(a.first_call_tokens),
                    known(b.first_call_tokens),
                ),
                row(
                    "System prompt",
                    Tokens,
                    known(a.system_prompt_tokens),
                    known(b.system_prompt_tokens),
                ),
                row(
                    "Tool definitions",
                    Tokens,
                    known(a.tool_definition_tokens),
                    known(b.tool_definition_tokens),
                ),
                row(
                    "Instructions and reminders",
                    Tokens,
                    known(a.instruction_tokens),
                    known(b.instruction_tokens),
                ),
                row(
                    "Everything else",
                    Tokens,
                    known(a.other_context_tokens),
                    known(b.other_context_tokens),
                ),
                row(
                    "Not captured",
                    Tokens,
                    known(a.not_captured_tokens),
                    known(b.not_captured_tokens),
                ),
                row("Tools", Count, known(a.tool_count), known(b.tool_count)),
            ],
        ),
        section(
            "Models",
            named_rows(Count, Some(&a.models), Some(&b.models)),
        ),
        section(
            "Tool groups",
            named_rows(Count, a.tool_groups.as_deref(), b.tool_groups.as_deref()),
        ),
        section(
            "Instructions",
            named_rows(
                Tokens,
                a.instruction_parts.as_deref(),
                b.instruction_parts.as_deref(),
            ),
        ),
        section(
            "Work",
            vec![
                row("Prompts", Count, count(a.prompts), count(b.prompts)),
                row(
                    "Model calls",
                    Count,
                    count(a.model_calls),
                    count(b.model_calls),
                ),
                row(
                    "Tool calls",
                    Count,
                    count(a.tool_calls),
                    count(b.tool_calls),
                ),
                row("Subagents", Count, count(a.subagents), count(b.subagents)),
                row("Wall-clock time", Ms, known(a.wall_ms), known(b.wall_ms)),
                row("Time in model calls", Ms, a.model_ms, b.model_ms),
                row("Time in tools", Ms, a.tool_ms, b.tool_ms),
            ],
        ),
        section(
            "Tools used",
            named_rows(Count, Some(&a.tools_by_name), Some(&b.tools_by_name)),
        ),
        section(
            "Tokens",
            vec![
                row("Input (not cached)", Tokens, a.input_tokens, b.input_tokens),
                row("Output", Tokens, a.output_tokens, b.output_tokens),
                row(
                    "Cache read",
                    Tokens,
                    a.cache_read_tokens,
                    b.cache_read_tokens,
                ),
                row(
                    "Cache write",
                    Tokens,
                    a.cache_write_tokens,
                    b.cache_write_tokens,
                ),
                row(
                    "Largest context",
                    Tokens,
                    known(a.largest_context),
                    known(b.largest_context),
                ),
            ],
        ),
        section("Problems", problems),
    ];
    Comparison { a, b, sections }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::{Item, Slice};
    use serde_json::json;
    use trace_core::{Marker, ModelCall, Run, ToolCall, ToolResult, Turn, Usage};

    fn add(
        trace: &mut Trace,
        id: &str,
        parent: Option<&str>,
        node: Node,
        times: (Option<i64>, Option<i64>),
    ) {
        let event = TraceEvent {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            node,
            started_at_ms: times.0,
            ended_at_ms: times.1,
            raw: Vec::new(),
            metadata: Default::default(),
        };
        assert!(trace.apply(event).is_ok());
    }

    const NO_TIME: (Option<i64>, Option<i64>) = (None, None);

    fn run() -> Node {
        Node::Run(Run {
            harness: "test".into(),
            title: None,
        })
    }

    fn turn() -> Node {
        Node::Turn(Turn { prompt: Vec::new() })
    }

    fn model(name: Option<&str>, usage: Option<Usage>, text: &str) -> Node {
        let output = if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text { text: text.into() }]
        };
        Node::ModelCall(ModelCall {
            model: name.map(str::to_string),
            output,
            stop_reason: None,
            usage,
        })
    }

    fn usage(input: u64, output: u64, read: u64) -> Option<Usage> {
        Some(Usage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(read),
            cache_write_tokens: Some(0),
        })
    }

    fn tool(name: &str, failed: Option<bool>) -> Node {
        Node::ToolCall(ToolCall {
            name: name.into(),
            input: json!({}),
            result: failed.map(|is_error| ToolResult {
                content: Vec::new(),
                is_error,
            }),
        })
    }

    fn compaction() -> Node {
        Node::Marker(Marker {
            kind: "compaction".into(),
            summary: String::new(),
            compaction: None,
        })
    }

    /// R > T1 > M1 (Bash, Task > S > ST > SM1 > Read), M2 "done";
    /// R > T2 > M3 (Bash failed), marker; M4 with no usage.
    fn sample() -> Trace {
        let mut t = Trace::new();
        add(&mut t, "R", None, run(), NO_TIME);
        add(&mut t, "T1", Some("R"), turn(), (Some(1_000), Some(9_000)));
        add(
            &mut t,
            "M1",
            Some("T1"),
            model(Some("model-a"), usage(10, 5, 100), ""),
            (Some(1_000), Some(2_000)),
        );
        add(
            &mut t,
            "X1",
            Some("M1"),
            tool("Bash", Some(false)),
            (Some(2_000), Some(2_500)),
        );
        add(
            &mut t,
            "X2",
            Some("M1"),
            tool("Task", Some(false)),
            (Some(2_000), Some(6_000)),
        );
        add(&mut t, "S", Some("X2"), run(), NO_TIME);
        add(&mut t, "ST", Some("S"), turn(), NO_TIME);
        add(
            &mut t,
            "SM1",
            Some("ST"),
            model(Some("model-b"), usage(1, 2, 3), "sub text"),
            (Some(2_100), Some(2_600)),
        );
        add(
            &mut t,
            "SX1",
            Some("SM1"),
            tool("Read", Some(false)),
            (Some(2_600), None),
        );
        add(
            &mut t,
            "M2",
            Some("T1"),
            model(Some("model-a"), usage(20, 7, 200), "done"),
            (Some(6_000), Some(9_000)),
        );
        add(
            &mut t,
            "T2",
            Some("R"),
            turn(),
            (Some(10_000), Some(12_000)),
        );
        add(
            &mut t,
            "M3",
            Some("T2"),
            model(Some("model-a"), usage(30, 1, 300), "second"),
            (Some(10_000), Some(11_000)),
        );
        add(
            &mut t,
            "X3",
            Some("M3"),
            tool("Bash", Some(true)),
            (Some(11_000), Some(11_500)),
        );
        add(&mut t, "K", Some("T2"), compaction(), (Some(11_600), None));
        add(
            &mut t,
            "M4",
            Some("T2"),
            model(Some("model-a"), None, ""),
            (Some(11_700), Some(12_000)),
        );
        t
    }

    fn names(list: &[Named]) -> Vec<(&str, u64)> {
        list.iter().map(|n| (n.name.as_str(), n.value)).collect()
    }

    #[test]
    fn counts_every_run_and_keeps_order() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.prompts, 2);
        assert_eq!(s.model_calls, 5);
        assert_eq!(s.tool_calls, 4);
        assert_eq!(s.subagents, 1);
        assert_eq!(s.failed_tool_calls, 1);
        assert_eq!(s.compactions, 1);
        assert_eq!(names(&s.models), vec![("model-a", 4), ("model-b", 1)]);
        assert_eq!(
            names(&s.tools_by_name),
            vec![("Bash", 2), ("Task", 1), ("Read", 1)]
        );
    }

    #[test]
    fn tokens_add_known_values_and_mark_partial() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(
            s.input_tokens,
            Total {
                value: Some(61),
                partial: true
            }
        );
        assert_eq!(
            s.output_tokens,
            Total {
                value: Some(15),
                partial: true
            }
        );
        assert_eq!(
            s.cache_read_tokens,
            Total {
                value: Some(603),
                partial: true
            }
        );
        // Main run only: M1 110, M2 220, M3 330, M4 unknown.
        let points: Vec<(u32, Option<u64>)> = s
            .context_per_call
            .iter()
            .map(|p| (p.index, p.tokens))
            .collect();
        assert_eq!(
            points,
            vec![(1, Some(110)), (2, Some(220)), (3, Some(330)), (4, None)]
        );
        assert_eq!(s.largest_context, Some(330));
    }

    #[test]
    fn times_cover_the_scope() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.wall_ms, Some(11_000));
        assert_eq!(
            s.model_ms,
            Total {
                value: Some(1_000 + 500 + 3_000 + 1_000 + 300),
                partial: false
            }
        );
        // SX1 has no end.
        assert_eq!(
            s.tool_ms,
            Total {
                value: Some(500 + 4_000 + 500),
                partial: true
            }
        );
    }

    #[test]
    fn final_message_is_the_main_runs_last_text() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.final_message.as_deref(), Some("second"));
        let p = run_summary(&sample(), &Scope::Prompt("T1".into()), None, &[]);
        assert_eq!(p.final_message.as_deref(), Some("done"));
    }

    #[test]
    fn prompt_scope_counts_only_that_turn() {
        let s = run_summary(&sample(), &Scope::Prompt("T2".into()), None, &[]);
        assert_eq!(s.prompts, 1);
        assert_eq!(s.model_calls, 2);
        assert_eq!(s.tool_calls, 1);
        assert_eq!(s.subagents, 0);
        assert_eq!(s.wall_ms, Some(2_000));
        assert_eq!(
            first_main_call(&sample(), &Scope::Prompt("T2".into())).as_deref(),
            Some("M3")
        );
        assert_eq!(
            first_main_call(&sample(), &Scope::Session).as_deref(),
            Some("M1")
        );
    }

    #[test]
    fn prompt_scope_needs_a_main_run_turn() {
        let trace = sample();
        assert!(prompt_in_main_run(&trace, "T1"));
        assert!(!prompt_in_main_run(&trace, "ST"));
        assert!(!prompt_in_main_run(&trace, "M1"));
        assert!(!prompt_in_main_run(&trace, "nope"));
        let s = run_summary(&trace, &Scope::Prompt("ST".into()), None, &[]);
        assert_eq!(s.model_calls, 0);
        assert_eq!(
            s.input_tokens,
            Total {
                value: None,
                partial: false
            }
        );
        assert_eq!(s.wall_ms, None);
        assert_eq!(first_main_call(&trace, &Scope::Prompt("ST".into())), None);
    }

    #[test]
    fn no_model_calls_leaves_tokens_unknown() {
        let mut t = Trace::new();
        add(&mut t, "R", None, run(), NO_TIME);
        add(&mut t, "T1", Some("R"), turn(), NO_TIME);
        let s = run_summary(&t, &Scope::Session, None, &[]);
        assert_eq!(s.prompts, 1);
        assert_eq!(
            s.output_tokens,
            Total {
                value: None,
                partial: false
            }
        );
        assert_eq!(s.largest_context, None);
        assert!(s.context_per_call.is_empty());
        assert_eq!(s.final_message, None);
        assert_eq!(s.first_call_tokens, None);
        assert_eq!(s.tool_groups, None);
    }

    fn flag(kind: FlagKind, first: &str) -> Flag {
        Flag {
            kind,
            level: crate::advice::AdviceLevel::Warn,
            text: String::new(),
            trace_ids: vec![first.to_string(), "M1".to_string()],
            run_id: "R".into(),
        }
    }

    #[test]
    fn flags_count_by_first_node_in_scope() {
        let flags = vec![
            flag(FlagKind::Loop, "X1"),
            flag(FlagKind::Loop, "X3"),
            flag(FlagKind::LargeResult, "SX1"),
        ];
        let count = |s: &RunSummary, k: FlagKind| {
            s.flags_by_kind
                .iter()
                .find(|c| c.kind == k)
                .map(|c| c.count)
        };
        let all = run_summary(&sample(), &Scope::Session, None, &flags);
        assert_eq!(all.flags_by_kind.len(), FLAG_KINDS.len());
        assert_eq!(count(&all, FlagKind::Loop), Some(2));
        assert_eq!(count(&all, FlagKind::LargeResult), Some(1));
        assert_eq!(count(&all, FlagKind::Compaction), Some(0));
        let second = run_summary(&sample(), &Scope::Prompt("T2".into()), None, &flags);
        assert_eq!(count(&second, FlagKind::Loop), Some(1));
        assert_eq!(count(&second, FlagKind::LargeResult), Some(0));
    }

    fn slice(kind: SliceKind, tokens: u64, items: Vec<(&str, u64, u32)>) -> Slice {
        Slice {
            kind,
            label: kind.label().to_string(),
            tokens,
            share: 0.0,
            items: items
                .into_iter()
                .map(|(label, tokens, count)| Item {
                    label: label.into(),
                    tokens,
                    count,
                    largest_tokens: tokens,
                    from_transcript: false,
                })
                .collect(),
        }
    }

    fn breakdown_of(slices: Vec<Slice>, total: u64) -> ContextBreakdown {
        ContextBreakdown {
            source: ContextSource::Captured,
            total_tokens: total,
            total_is_reported: true,
            slices,
            advice: Vec::new(),
        }
    }

    #[test]
    fn setup_comes_from_the_first_call() {
        let b = breakdown_of(
            vec![
                slice(
                    SliceKind::SystemPrompt,
                    100,
                    vec![("system prompt", 100, 1)],
                ),
                slice(
                    SliceKind::ToolDefinitions,
                    300,
                    vec![("built-in", 200, 12), ("mcp: docs", 100, 3)],
                ),
                slice(
                    SliceKind::Instructions,
                    50,
                    vec![("CLAUDE.md", 40, 1), ("other reminders", 10, 2)],
                ),
                slice(SliceKind::Conversation, 20, vec![("prompt", 20, 1)]),
                slice(SliceKind::FilesRead, 5, vec![("a.rs", 5, 1)]),
            ],
            475,
        );
        let s = run_summary(&sample(), &Scope::Session, Some(&b), &[]);
        assert_eq!(s.first_call_source, Some(ContextSource::Captured));
        assert_eq!(s.first_call_tokens, Some(475));
        assert_eq!(s.system_prompt_tokens, Some(100));
        assert_eq!(s.tool_definition_tokens, Some(300));
        assert_eq!(s.instruction_tokens, Some(50));
        assert_eq!(s.other_context_tokens, Some(25));
        assert_eq!(s.not_captured_tokens, Some(0));
        assert_eq!(s.tool_count, Some(15));
        assert_eq!(
            s.tool_groups.as_deref().map(names),
            Some(vec![("built-in", 12), ("mcp: docs", 3)])
        );
        assert_eq!(
            s.instruction_parts.as_deref().map(names),
            Some(vec![("CLAUDE.md", 40), ("other reminders", 10)])
        );
    }

    #[test]
    fn setup_slices_not_seen_stay_unknown() {
        let b = breakdown_of(
            vec![
                slice(SliceKind::Conversation, 20, vec![]),
                slice(SliceKind::NotCaptured, 480, vec![]),
            ],
            500,
        );
        let s = run_summary(&sample(), &Scope::Session, Some(&b), &[]);
        assert_eq!(s.system_prompt_tokens, None);
        assert_eq!(s.tool_count, None);
        assert_eq!(s.tool_groups, None);
        assert_eq!(s.instruction_parts, None);
        assert_eq!(s.other_context_tokens, Some(20));
        assert_eq!(s.not_captured_tokens, Some(480));
    }

    #[test]
    fn setup_without_a_breakdown_is_unknown() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.other_context_tokens, None);
        assert_eq!(s.not_captured_tokens, None);
    }

    fn summary_with(
        models: Vec<(&str, u64)>,
        groups: Option<Vec<(&str, u64)>>,
        output: Total,
    ) -> RunSummary {
        let mut s = run_summary(&Trace::new(), &Scope::Session, None, &[]);
        s.models = models
            .into_iter()
            .map(|(n, v)| Named {
                name: n.into(),
                value: v,
            })
            .collect();
        s.tool_groups = groups.map(|g| {
            g.into_iter()
                .map(|(n, v)| Named {
                    name: n.into(),
                    value: v,
                })
                .collect()
        });
        s.output_tokens = output;
        s
    }

    fn find_section<'c>(c: &'c Comparison, title: &str) -> &'c Section {
        c.sections
            .iter()
            .find(|s| s.title == title)
            .expect("section")
    }

    fn find_row<'c>(c: &'c Comparison, title: &str, label: &str) -> &'c Row {
        find_section(c, title)
            .rows
            .iter()
            .find(|r| r.label == label)
            .expect("row")
    }

    #[test]
    fn sections_come_in_order() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        let c = compare(s.clone(), s);
        let titles: Vec<&str> = c.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Setup",
                "Models",
                "Tool groups",
                "Instructions",
                "Work",
                "Tools used",
                "Tokens",
                "Problems"
            ]
        );
        let problems: Vec<&str> = find_section(&c, "Problems")
            .rows
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(
            problems,
            vec![
                "Repeated reads",
                "Failed retries",
                "Loops",
                "Large results",
                "Re-reads after compaction",
                "Failed tool calls",
                "Compactions"
            ]
        );
        let setup: Vec<&str> = find_section(&c, "Setup")
            .rows
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(
            setup,
            vec![
                "First call context",
                "System prompt",
                "Tool definitions",
                "Instructions and reminders",
                "Everything else",
                "Not captured",
                "Tools"
            ]
        );
    }

    #[test]
    fn a_run_against_itself_is_equal_everywhere_known() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        let c = compare(s.clone(), s);
        for sec in &c.sections {
            for r in &sec.rows {
                if r.a.is_some() {
                    assert_eq!(r.higher, Higher::Equal, "{}", r.label);
                    assert_eq!(r.difference, Some(0), "{}", r.label);
                } else {
                    assert_eq!(r.higher, Higher::Unknown, "{}", r.label);
                }
            }
        }
    }

    #[test]
    fn difference_is_b_minus_a() {
        let a = summary_with(
            vec![],
            None,
            Total {
                value: Some(100),
                partial: false,
            },
        );
        let b = summary_with(
            vec![],
            None,
            Total {
                value: Some(40),
                partial: true,
            },
        );
        let c = compare(a, b);
        let r = find_row(&c, "Tokens", "Output");
        assert_eq!((r.a, r.b), (Some(100), Some(40)));
        assert_eq!(r.difference, Some(-60));
        assert_eq!(r.higher, Higher::A);
        assert!(!r.a_partial);
        assert!(r.b_partial);
        assert_eq!(r.unit, Unit::Tokens);
    }

    #[test]
    fn unknown_on_one_side_is_unknown() {
        let a = summary_with(
            vec![],
            None,
            Total {
                value: Some(1),
                partial: false,
            },
        );
        let b = summary_with(vec![], None, Total::default());
        let c = compare(a, b);
        let r = find_row(&c, "Tokens", "Output");
        assert_eq!(r.difference, None);
        assert_eq!(r.higher, Higher::Unknown);
    }

    #[test]
    fn names_on_one_side_get_zero_and_a_tag() {
        let a = summary_with(
            vec![("m1", 3), ("m2", 1)],
            Some(vec![("built-in", 10)]),
            Total::default(),
        );
        let b = summary_with(
            vec![("m3", 2), ("m1", 1)],
            Some(vec![("built-in", 10), ("mcp: docs", 4)]),
            Total::default(),
        );
        let c = compare(a, b);
        let models: Vec<(&str, Option<u64>, Option<u64>, Presence)> = find_section(&c, "Models")
            .rows
            .iter()
            .map(|r| (r.label.as_str(), r.a, r.b, r.presence))
            .collect();
        assert_eq!(
            models,
            vec![
                ("m1", Some(3), Some(1), Presence::Both),
                ("m2", Some(1), Some(0), Presence::OnlyA),
                ("m3", Some(0), Some(2), Presence::OnlyB),
            ]
        );
        let docs = find_row(&c, "Tool groups", "mcp: docs");
        assert_eq!(
            (docs.a, docs.b, docs.presence, docs.higher),
            (Some(0), Some(4), Presence::OnlyB, Higher::B)
        );
    }

    #[test]
    fn unknown_setup_is_not_only_on_the_other_side() {
        let a = summary_with(vec![], None, Total::default());
        let b = summary_with(vec![], Some(vec![("built-in", 10)]), Total::default());
        let c = compare(a, b);
        let r = find_row(&c, "Tool groups", "built-in");
        assert_eq!(
            (r.a, r.b, r.presence, r.higher),
            (None, Some(10), Presence::Both, Higher::Unknown)
        );
    }
}
