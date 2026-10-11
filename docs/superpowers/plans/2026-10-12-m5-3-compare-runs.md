# M5.3 Compare Two Runs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Compare two runs side by side (setup, work, tokens, problems, outcome), each side a whole session or one prompt.

**Architecture:** A new pure module `insights::compare` builds a `RunSummary` per side from the trace, the 5.1 breakdown of the first main-run call and the 5.2 flags, and turns two summaries into a `Comparison` of ready-made rows. The app's `compare_runs` command loads both sides and returns a `CompareView` (comparison plus prompt choices per side). The UI adds a session picker mode, a Compare view and a plain SVG chart, and only formats what it gets.

**Tech Stack:** Rust (trace-core, insights, trace-view, Tauri 2 app), React + TypeScript + Vite, Vitest.

**Spec:** `docs/superpowers/specs/2026-10-12-m5-3-compare-runs-design.md`

## Global Constraints

- No schema change: `crates/trace-core` is not touched.
- `insights` stays harness-neutral: no Claude Code names in it. It does no I/O.
- No `unwrap()` / `expect()` outside tests (clippy denies them workspace-wide).
- No em dashes anywhere (code, comments, docs, UI text).
- The UI does no maths on trace data: every number, difference, "higher" side and "only in" tag comes from Rust.
- No new dependencies (Rust or npm). The chart is plain SVG.
- Every command and doc instruction works in PowerShell.
- Tauri permissions stay minimal: one new `allow-compare-runs` permission and nothing else.
- Fixtures are invented; nothing personal in the repo.
- Commit in small steps; commit messages for a public reader, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- Checks before a task is done: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; for UI tasks also `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.

## Review Focus

- A session with no model calls (or one whose prompt has no model call yet): every token row is unknown ("?"), nothing panics, and the setup rows are unknown. Test in Task 1.
- The same session on both sides with two different prompts: each side counts only its own prompt. Test in Task 3.
- A scoped prompt id that is not a main-run turn (deleted, a subagent turn, or garbage): the side falls back to the whole session and says so. Tests in Tasks 1 and 3.
- A tool that appears on one side only: shown with 0 on the other side and an "only in" tag, never as unknown. A side whose setup is unknown: its named setup rows are unknown, not "only in" the other side. Test in Task 2.
- Swap and scope changes while an older request is still running: the older response must not overwrite the newer one. Test in Task 5 (a token guard like `captureToken` in `App.tsx`).

---

### Task 1: `insights::compare` run summary

**Files:**
- Create: `crates/insights/src/compare.rs`
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Consumes: `trace_core::{Trace, TraceEvent, Node, ContentBlock}`, `crate::breakdown::ContextBreakdown`, `crate::flags::{Flag, FlagKind}`, `crate::measure::{ContextSource, SliceKind}`.
- Produces (all `pub`, re-exported from `lib.rs`): `Scope`, `Total`, `Named`, `CallPoint`, `KindCount`, `RunSummary`, `run_summary(trace: &Trace, scope: &Scope, first_call: Option<&ContextBreakdown>, flags: &[Flag]) -> RunSummary`, `first_main_call(trace: &Trace, scope: &Scope) -> Option<String>`, `prompt_in_main_run(trace: &Trace, turn_id: &str) -> bool`, `FLAG_KINDS: [FlagKind; 6]`.

Definitions:
- The **main run** is the first root node whose node is `Node::Run`.
- `Scope::Session` covers every root and everything under it. `Scope::Prompt(id)` covers the turn `id` and everything under it, but only if `prompt_in_main_run(trace, id)`; otherwise the summary is empty (all counts 0, all totals unknown).
- A node is **main** when no `Run` with a `parent_id` (a subagent run) lies between it and the scope's start.
- A **subagent** is a `Run` node that has a `parent_id`.

- [ ] **Step 1: Write the module with its types and the failing tests**

Create `crates/insights/src/compare.rs`:

```rust
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
    /// The rest of the first call's context.
    pub other_context_tokens: Option<u64>,
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
```

Then add the tests module at the end of the file (it fails to compile until Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::{Item, Slice};
    use serde_json::json;
    use trace_core::{Marker, ModelCall, Run, ToolCall, ToolResult, Turn, Usage};

    fn add(trace: &mut Trace, id: &str, parent: Option<&str>, node: Node, times: (Option<i64>, Option<i64>)) {
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
        Node::Run(Run { harness: "test".into(), title: None })
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
            result: failed.map(|is_error| ToolResult { content: Vec::new(), is_error }),
        })
    }

    fn compaction() -> Node {
        Node::Marker(Marker { kind: "compaction".into(), summary: String::new(), compaction: None })
    }

    /// R > T1 > M1 (Bash, Task > S > ST > SM1 > Read), M2 "done";
    /// R > T2 > M3 (Bash failed), marker; M4 with no usage.
    fn sample() -> Trace {
        let mut t = Trace::new();
        add(&mut t, "R", None, run(), NO_TIME);
        add(&mut t, "T1", Some("R"), turn(), (Some(1_000), Some(9_000)));
        add(&mut t, "M1", Some("T1"), model(Some("model-a"), usage(10, 5, 100), ""), (Some(1_000), Some(2_000)));
        add(&mut t, "X1", Some("M1"), tool("Bash", Some(false)), (Some(2_000), Some(2_500)));
        add(&mut t, "X2", Some("M1"), tool("Task", Some(false)), (Some(2_000), Some(6_000)));
        add(&mut t, "S", Some("X2"), run(), NO_TIME);
        add(&mut t, "ST", Some("S"), turn(), NO_TIME);
        add(&mut t, "SM1", Some("ST"), model(Some("model-b"), usage(1, 2, 3), "sub text"), (Some(2_100), Some(2_600)));
        add(&mut t, "SX1", Some("SM1"), tool("Read", Some(false)), (Some(2_600), None));
        add(&mut t, "M2", Some("T1"), model(Some("model-a"), usage(20, 7, 200), "done"), (Some(6_000), Some(9_000)));
        add(&mut t, "T2", Some("R"), turn(), (Some(10_000), Some(12_000)));
        add(&mut t, "M3", Some("T2"), model(Some("model-a"), usage(30, 1, 300), "second"), (Some(10_000), Some(11_000)));
        add(&mut t, "X3", Some("M3"), tool("Bash", Some(true)), (Some(11_000), Some(11_500)));
        add(&mut t, "K", Some("T2"), compaction(), (Some(11_600), None));
        add(&mut t, "M4", Some("T2"), model(Some("model-a"), None, ""), (Some(11_700), Some(12_000)));
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
        assert_eq!(names(&s.tools_by_name), vec![("Bash", 2), ("Task", 1), ("Read", 1)]);
    }

    #[test]
    fn tokens_add_known_values_and_mark_partial() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.input_tokens, Total { value: Some(61), partial: true });
        assert_eq!(s.output_tokens, Total { value: Some(15), partial: true });
        assert_eq!(s.cache_read_tokens, Total { value: Some(603), partial: true });
        // Main run only: M1 110, M2 220, M3 330, M4 unknown.
        let points: Vec<(u32, Option<u64>)> = s.context_per_call.iter().map(|p| (p.index, p.tokens)).collect();
        assert_eq!(points, vec![(1, Some(110)), (2, Some(220)), (3, Some(330)), (4, None)]);
        assert_eq!(s.largest_context, Some(330));
    }

    #[test]
    fn times_cover_the_scope() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        assert_eq!(s.wall_ms, Some(11_000));
        assert_eq!(s.model_ms, Total { value: Some(1_000 + 500 + 3_000 + 1_000 + 300), partial: false });
        // SX1 has no end.
        assert_eq!(s.tool_ms, Total { value: Some(500 + 4_000 + 500), partial: true });
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
        assert_eq!(first_main_call(&sample(), &Scope::Prompt("T2".into())).as_deref(), Some("M3"));
        assert_eq!(first_main_call(&sample(), &Scope::Session).as_deref(), Some("M1"));
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
        assert_eq!(s.input_tokens, Total { value: None, partial: false });
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
        assert_eq!(s.output_tokens, Total { value: None, partial: false });
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
        let count = |s: &RunSummary, k: FlagKind| s.flags_by_kind.iter().find(|c| c.kind == k).map(|c| c.count);
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
                slice(SliceKind::SystemPrompt, 100, vec![("system prompt", 100, 1)]),
                slice(SliceKind::ToolDefinitions, 300, vec![("built-in", 200, 12), ("mcp: docs", 100, 3)]),
                slice(SliceKind::Instructions, 50, vec![("CLAUDE.md", 40, 1), ("other reminders", 10, 2)]),
                slice(SliceKind::Conversation, 20, vec![("prompt", 20, 1)]),
            ],
            470,
        );
        let s = run_summary(&sample(), &Scope::Session, Some(&b), &[]);
        assert_eq!(s.first_call_source, Some(ContextSource::Captured));
        assert_eq!(s.first_call_tokens, Some(470));
        assert_eq!(s.system_prompt_tokens, Some(100));
        assert_eq!(s.tool_definition_tokens, Some(300));
        assert_eq!(s.instruction_tokens, Some(50));
        assert_eq!(s.other_context_tokens, Some(20));
        assert_eq!(s.tool_count, Some(15));
        assert_eq!(s.tool_groups.as_deref().map(names), Some(vec![("built-in", 12), ("mcp: docs", 3)]));
        assert_eq!(s.instruction_parts.as_deref().map(names), Some(vec![("CLAUDE.md", 40), ("other reminders", 10)]));
    }

    #[test]
    fn setup_slices_not_seen_stay_unknown() {
        let b = breakdown_of(vec![slice(SliceKind::Conversation, 20, vec![])], 500);
        let s = run_summary(&sample(), &Scope::Session, Some(&b), &[]);
        assert_eq!(s.system_prompt_tokens, None);
        assert_eq!(s.tool_count, None);
        assert_eq!(s.tool_groups, None);
        assert_eq!(s.instruction_parts, None);
        assert_eq!(s.other_context_tokens, Some(500));
    }
}
```

Note on `names` in `setup_comes_from_the_first_call`: `tool_groups.as_deref()` gives `Option<&[Named]>`, so `.map(names)` works as written.

- [ ] **Step 2: Register the module and run the tests to see them fail**

In `crates/insights/src/lib.rs` add `pub mod compare;` after `pub mod breakdown;`, and add the re-export line:

```rust
pub use compare::{
    CallPoint, FLAG_KINDS, KindCount, Named, RunSummary, Scope, Total, first_main_call,
    prompt_in_main_run, run_summary,
};
```

Also extend the crate doc comment's last sentence to: "Also flags waste and problems in a trace (`session_flags`), with rules as `FlagRules`, and compares two runs (`compare`)."

Run: `cargo test -p insights compare`
Expected: FAIL to compile (`run_summary`, `first_main_call`, `prompt_in_main_run` not found).

- [ ] **Step 3: Implement the walk**

Add above the tests module in `compare.rs`:

```rust
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
    walk(trace, scope).main_calls.first().map(|p| p.trace_id.clone())
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
        None => list.push(Named { name: name.to_string(), value: by }),
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
                bump(&mut self.models, call.model.as_deref().unwrap_or("unknown model"), 1);
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
                value: if by_count { u64::from(i.count) } else { i.tokens },
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
        let known: u64 = [system, tools, instructions].iter().flatten().sum();
        b.total_tokens.saturating_sub(known)
    });
    let tool_groups = first_call.and_then(|b| slice_items(b, SliceKind::ToolDefinitions, true));
    let flags_by_kind = FLAG_KINDS
        .iter()
        .map(|&kind| KindCount {
            kind,
            count: flags
                .iter()
                .filter(|f| f.kind == kind)
                .filter(|f| f.trace_ids.first().is_some_and(|id| w.ids.contains(id.as_str())))
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
        tool_count: tool_groups.as_ref().map(|g| g.iter().map(|n| n.value).sum()),
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
```

If the borrow checker rejects `w.ids.contains` after moving fields out of `w`, compute `flags_by_kind` and `wall_ms` before building `RunSummary` (as written) so the moves come last.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p insights compare`
Expected: all 10 tests PASS. If an expected number in a test is wrong, recheck the arithmetic against the `sample()` comment before changing the code; the test numbers are the contract.

- [ ] **Step 5: Lint and commit**

Run: `cargo fmt --all; cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

```powershell
git add crates/insights/src/compare.rs crates/insights/src/lib.rs
git commit -m "Summarise one run for comparison in insights" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `insights::compare` comparison rows

**Files:**
- Modify: `crates/insights/src/compare.rs`
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Consumes: `RunSummary`, `Named`, `Total`, `FLAG_KINDS` from Task 1.
- Produces (`pub`, re-exported): `Unit`, `Higher`, `Presence`, `Row`, `Section`, `Comparison`, `compare(a: RunSummary, b: RunSummary) -> Comparison`.

JSON: `Unit` is `"count" | "tokens" | "ms"`; `Higher` is `"a" | "b" | "equal" | "unknown"`; `Presence` is `"both" | "only_a" | "only_b"`.

Sections, titles and row labels, in this exact order (empty named sections are kept with no rows; the UI hides them):

1. `Setup`: `First call context` (tokens, `first_call_tokens`), `System prompt` (tokens), `Tool definitions` (tokens), `Instructions and reminders` (tokens), `Everything else` (tokens, `other_context_tokens`), `Tools` (count, `tool_count`).
2. `Models`: one row per model (count).
3. `Tool groups`: one row per group (count, from `tool_groups`).
4. `Instructions`: one row per instruction part (tokens, from `instruction_parts`).
5. `Work`: `Prompts`, `Model calls`, `Tool calls`, `Subagents` (count), `Wall-clock time` (ms, `wall_ms`), `Time in model calls` (ms, `model_ms`), `Time in tools` (ms, `tool_ms`).
6. `Tools used`: one row per tool name (count).
7. `Tokens`: `Input (not cached)`, `Output`, `Cache read`, `Cache write` (tokens, `Total`s), `Largest context` (tokens).
8. `Problems`: `Repeated reads`, `Failed retries`, `Loops`, `Large results`, `Re-reads after compaction` (count, from `flags_by_kind`; the `compaction` flag kind is not a row because the next row covers it), `Failed tool calls`, `Compactions` (count).

Named rows: names of side A in A's order, then names only in B in B's order. If both lists are known, a name missing on one side has value `Some(0)` there and presence `only_a` / `only_b`. If either side's list is `None` (unknown setup), that side's values are `None` and presence is `both`.

- [ ] **Step 1: Write the failing tests**

Append inside the tests module of `compare.rs`:

```rust
    fn summary_with(models: Vec<(&str, u64)>, groups: Option<Vec<(&str, u64)>>, output: Total) -> RunSummary {
        let mut s = run_summary(&Trace::new(), &Scope::Session, None, &[]);
        s.models = models.into_iter().map(|(n, v)| Named { name: n.into(), value: v }).collect();
        s.tool_groups = groups.map(|g| g.into_iter().map(|(n, v)| Named { name: n.into(), value: v }).collect());
        s.output_tokens = output;
        s
    }

    fn section<'c>(c: &'c Comparison, title: &str) -> &'c Section {
        c.sections.iter().find(|s| s.title == title).expect("section")
    }

    fn row<'c>(c: &'c Comparison, title: &str, label: &str) -> &'c Row {
        section(c, title).rows.iter().find(|r| r.label == label).expect("row")
    }

    #[test]
    fn sections_come_in_order() {
        let s = run_summary(&sample(), &Scope::Session, None, &[]);
        let c = compare(s.clone(), s);
        let titles: Vec<&str> = c.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Setup", "Models", "Tool groups", "Instructions", "Work", "Tools used", "Tokens", "Problems"]
        );
        let problems: Vec<&str> = section(&c, "Problems").rows.iter().map(|r| r.label.as_str()).collect();
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
        let a = summary_with(vec![], None, Total { value: Some(100), partial: false });
        let b = summary_with(vec![], None, Total { value: Some(40), partial: true });
        let c = compare(a, b);
        let r = row(&c, "Tokens", "Output");
        assert_eq!((r.a, r.b), (Some(100), Some(40)));
        assert_eq!(r.difference, Some(-60));
        assert_eq!(r.higher, Higher::A);
        assert!(!r.a_partial);
        assert!(r.b_partial);
        assert_eq!(r.unit, Unit::Tokens);
    }

    #[test]
    fn unknown_on_one_side_is_unknown() {
        let a = summary_with(vec![], None, Total { value: Some(1), partial: false });
        let b = summary_with(vec![], None, Total::default());
        let c = compare(a, b);
        let r = row(&c, "Tokens", "Output");
        assert_eq!(r.difference, None);
        assert_eq!(r.higher, Higher::Unknown);
    }

    #[test]
    fn names_on_one_side_get_zero_and_a_tag() {
        let a = summary_with(vec![("m1", 3), ("m2", 1)], Some(vec![("built-in", 10)]), Total::default());
        let b = summary_with(vec![("m3", 2), ("m1", 1)], Some(vec![("built-in", 10), ("mcp: docs", 4)]), Total::default());
        let c = compare(a, b);
        let models: Vec<(&str, Option<u64>, Option<u64>, Presence)> = section(&c, "Models")
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
        let docs = row(&c, "Tool groups", "mcp: docs");
        assert_eq!((docs.a, docs.b, docs.presence, docs.higher), (Some(0), Some(4), Presence::OnlyB, Higher::B));
    }

    #[test]
    fn unknown_setup_is_not_only_on_the_other_side() {
        let a = summary_with(vec![], None, Total::default());
        let b = summary_with(vec![], Some(vec![("built-in", 10)]), Total::default());
        let c = compare(a, b);
        let r = row(&c, "Tool groups", "built-in");
        assert_eq!((r.a, r.b, r.presence, r.higher), (None, Some(10), Presence::Both, Higher::Unknown));
    }
```

Run: `cargo test -p insights compare`
Expected: FAIL to compile (`compare`, `Comparison`, `Row`, ... not found).

- [ ] **Step 2: Implement**

Add above the tests module:

```rust
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
    Total { value, partial: false }
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
    let has = |list: Option<&[Named]>, name: &str| list.is_some_and(|l| l.iter().any(|n| n.name == name));
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
    s.flags_by_kind.iter().find(|c| c.kind == kind).map_or(0, |c| c.count)
}

fn section(title: &str, rows: Vec<Row>) -> Section {
    Section { title: title.to_string(), rows }
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
        .map(|&(label, kind)| row(label, Count, count(flag_count(&a, kind)), count(flag_count(&b, kind))))
        .collect();
    problems.push(row("Failed tool calls", Count, count(a.failed_tool_calls), count(b.failed_tool_calls)));
    problems.push(row("Compactions", Count, count(a.compactions), count(b.compactions)));
    let sections = vec![
        section(
            "Setup",
            vec![
                row("First call context", Tokens, known(a.first_call_tokens), known(b.first_call_tokens)),
                row("System prompt", Tokens, known(a.system_prompt_tokens), known(b.system_prompt_tokens)),
                row("Tool definitions", Tokens, known(a.tool_definition_tokens), known(b.tool_definition_tokens)),
                row("Instructions and reminders", Tokens, known(a.instruction_tokens), known(b.instruction_tokens)),
                row("Everything else", Tokens, known(a.other_context_tokens), known(b.other_context_tokens)),
                row("Tools", Count, known(a.tool_count), known(b.tool_count)),
            ],
        ),
        section("Models", named_rows(Count, Some(&a.models), Some(&b.models))),
        section("Tool groups", named_rows(Count, a.tool_groups.as_deref(), b.tool_groups.as_deref())),
        section("Instructions", named_rows(Tokens, a.instruction_parts.as_deref(), b.instruction_parts.as_deref())),
        section(
            "Work",
            vec![
                row("Prompts", Count, count(a.prompts), count(b.prompts)),
                row("Model calls", Count, count(a.model_calls), count(b.model_calls)),
                row("Tool calls", Count, count(a.tool_calls), count(b.tool_calls)),
                row("Subagents", Count, count(a.subagents), count(b.subagents)),
                row("Wall-clock time", Ms, known(a.wall_ms), known(b.wall_ms)),
                row("Time in model calls", Ms, a.model_ms, b.model_ms),
                row("Time in tools", Ms, a.tool_ms, b.tool_ms),
            ],
        ),
        section("Tools used", named_rows(Count, Some(&a.tools_by_name), Some(&b.tools_by_name))),
        section(
            "Tokens",
            vec![
                row("Input (not cached)", Tokens, a.input_tokens, b.input_tokens),
                row("Output", Tokens, a.output_tokens, b.output_tokens),
                row("Cache read", Tokens, a.cache_read_tokens, b.cache_read_tokens),
                row("Cache write", Tokens, a.cache_write_tokens, b.cache_write_tokens),
                row("Largest context", Tokens, known(a.largest_context), known(b.largest_context)),
            ],
        ),
        section("Problems", problems),
    ];
    Comparison { a, b, sections }
}
```

The tests module defines its own `section` and `row` helpers with different signatures; rename the test helpers to `find_section` and `find_row` (and update their uses) if the names clash.

In `lib.rs` extend the re-export to include `Comparison, Higher, Presence, Row, Section, Unit, compare`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p insights compare`
Expected: all 16 compare tests PASS.

- [ ] **Step 4: Lint and commit**

Run: `cargo fmt --all; cargo clippy --workspace --all-targets -- -D warnings`

```powershell
git add crates/insights/src/compare.rs crates/insights/src/lib.rs
git commit -m "Compare two run summaries as sections of rows" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `compare_runs` command

**Files:**
- Create: `src-tauri/src/compare.rs`
- Modify: `src-tauri/src/main.rs` (module, command, handler list)
- Modify: `src-tauri/src/capture_sink.rs` (`compare_side`)
- Modify: `src-tauri/src/sessions.rs` (`SessionError::InRun`, and the mock snapshot test for `compare-data.json`)
- Modify: `src-tauri/build.rs` (command list)
- Modify: `src-tauri/capabilities/default.json` (permission and description)
- Create (generated by the build): `src-tauri/permissions/autogenerated/compare_runs.toml` if Tauri generates it under `permissions/autogenerated` as it did for `session_flags.toml`; commit it if it appears.
- Create: `ui/src/mock/compare-data.json` (written by the snapshot test)

**Interfaces:**
- Consumes: `insights::{Scope, run_summary, first_main_call, prompt_in_main_run, compare, Comparison, RunSummary, session_flags}`, `adapter_claude_code::flag_rules`, `crate::context::{ContextFacts, call_context}`, `crate::captures::SessionRecords` (the type `measured_records` returns; check its module path in `capture_sink.rs` imports), `trace_view::build_session`.
- Produces: Tauri command `compare_runs(a: RunRef, b: RunRef) -> CompareView`, JSON:

```
RunRef      { project: string, session_id: string, prompt_trace_id?: string | null }
CompareView { comparison: Comparison, a: CompareSide, b: CompareSide }
CompareSide { prompts: PromptChoice[], scope_missing: bool }
PromptChoice { turn_id: string, index: number, preview: string }   // index from 0, as in PromptDiagram
```

- [ ] **Step 1: Add the error variant**

In `src-tauri/src/sessions.rs`, add to `SessionError`:

```rust
    /// Loading one side of a comparison failed.
    #[error("Run {side}: {source}")]
    InRun {
        /// "A" or "B".
        side: &'static str,
        /// What went wrong.
        source: Box<SessionError>,
    },
```

- [ ] **Step 2: Write `src-tauri/src/compare.rs` with failing tests**

```rust
//! The compare view: two runs side by side. Each side is a whole session or
//! one prompt of it; the numbers come from the harness-neutral `insights`
//! crate, with the Claude Code flag rules from the adapter.

use adapter_claude_code::flag_rules;
use serde::{Deserialize, Serialize};
use trace_core::Trace;

use crate::captures::SessionRecords;
use crate::context::{self, ContextFacts};

/// One side of a comparison, as the UI asks for it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunRef {
    /// The encoded project folder name.
    pub project: String,
    /// The session id.
    pub session_id: String,
    /// A main-run turn to limit the side to, or none for the whole session.
    #[serde(default)]
    pub prompt_trace_id: Option<String>,
}

/// A prompt the UI can limit a side to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptChoice {
    /// The turn's trace id.
    pub turn_id: String,
    /// Its position, from 0, as in the diagram.
    pub index: usize,
    /// A one-line preview of the prompt.
    pub preview: String,
}

/// What the UI needs about one side besides its numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompareSide {
    /// Every prompt of the session, for the scope drop-down.
    pub prompts: Vec<PromptChoice>,
    /// True when the asked-for prompt was not found and the whole session
    /// is shown instead.
    pub scope_missing: bool,
}

/// The response of `compare_runs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompareView {
    /// The two summaries and the rows.
    pub comparison: insights::Comparison,
    /// Run A's prompts and scope note.
    pub a: CompareSide,
    /// Run B's prompts and scope note.
    pub b: CompareSide,
}

/// One side summarised, ready to compare.
#[derive(Debug, Clone, PartialEq)]
pub struct SideSummary {
    /// The numbers.
    pub summary: insights::RunSummary,
    /// The prompts and scope note.
    pub side: CompareSide,
}

/// Summarises one side. `captures` must be summarised, as for the context
/// views. A prompt that is not a main-run turn falls back to the whole
/// session.
pub fn side_summary(trace: &Trace, prompt: Option<&str>, captures: &SessionRecords) -> SideSummary {
    let (scope, scope_missing) = match prompt {
        Some(id) if insights::prompt_in_main_run(trace, id) => (insights::Scope::Prompt(id.to_string()), false),
        Some(_) => (insights::Scope::Session, true),
        None => (insights::Scope::Session, false),
    };
    let first = insights::first_main_call(trace, &scope).and_then(|id| {
        let facts = ContextFacts::of_call(trace, &id);
        context::call_context(&facts, captures, &id)
    });
    let flags = insights::session_flags(trace, &flag_rules());
    let summary = insights::run_summary(trace, &scope, first.as_ref(), &flags);
    let prompts = trace_view::build_session(trace)
        .prompts
        .into_iter()
        .map(|p| PromptChoice {
            turn_id: p.turn_id,
            index: p.index,
            preview: p.prompt_preview,
        })
        .collect();
    SideSummary {
        summary,
        side: CompareSide { prompts, scope_missing },
    }
}

/// Puts two sides together.
pub fn compare_view(a: SideSummary, b: SideSummary) -> CompareView {
    CompareView {
        comparison: insights::compare(a.summary, b.summary),
        a: a.side,
        b: b.side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::load_trace;
    use insights::Higher;
    use std::path::{Path, PathBuf};

    const BASIC: &str = "00000000-0000-4000-8000-000000000002";
    const COMPACTION: &str = "00000000-0000-4000-8000-000000000003";

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("fixtures").join("claude-code")
    }

    fn side(project: &str, session: &str, prompt: Option<&str>) -> SideSummary {
        let loaded = load_trace(&fixture_root(), project, session).expect("trace");
        side_summary(&loaded.trace, prompt, &SessionRecords::default())
    }

    #[test]
    fn a_session_against_itself_is_equal() {
        let view = compare_view(side("basic", BASIC, None), side("basic", BASIC, None));
        for sec in &view.comparison.sections {
            for row in &sec.rows {
                assert!(matches!(row.higher, Higher::Equal | Higher::Unknown), "{}", row.label);
            }
        }
        assert!(!view.a.scope_missing);
        assert!(!view.a.prompts.is_empty());
    }

    #[test]
    fn prompts_match_the_diagram() {
        let loaded = load_trace(&fixture_root(), "compaction", COMPACTION).expect("trace");
        let diagram = trace_view::build_session(&loaded.trace);
        let s = side("compaction", COMPACTION, None);
        let ids: Vec<&str> = s.side.prompts.iter().map(|p| p.turn_id.as_str()).collect();
        let expected: Vec<&str> = diagram.prompts.iter().map(|p| p.turn_id.as_str()).collect();
        assert_eq!(ids, expected);
        assert_eq!(s.summary.prompts as usize, expected.len());
    }

    #[test]
    fn two_prompts_of_one_session_count_separately() {
        let whole = side("compaction", COMPACTION, None);
        let prompts = &whole.side.prompts;
        assert!(prompts.len() >= 2, "the compaction fixture has several prompts");
        let first = side("compaction", COMPACTION, Some(&prompts[0].turn_id));
        let second = side("compaction", COMPACTION, Some(&prompts[1].turn_id));
        assert_eq!(first.summary.prompts, 1);
        assert_eq!(second.summary.prompts, 1);
        assert!(first.summary.model_calls + second.summary.model_calls <= whole.summary.model_calls);
        assert!(!first.side.scope_missing);
    }

    #[test]
    fn a_missing_prompt_falls_back_to_the_session() {
        let whole = side("basic", BASIC, None);
        let missing = side("basic", BASIC, Some("no-such-turn"));
        assert!(missing.side.scope_missing);
        assert_eq!(missing.summary, whole.summary);
    }
}
```

In `main.rs` add `mod compare;` next to the other `mod` lines. Check the real path of `SessionRecords` (search `pub struct SessionRecords`) and fix the `use` if it lives elsewhere; if `SessionRecords` has no `Default`, build an empty one the way `measured_records` does (`SessionRecords::default()` is used there today).

Run: `cargo test -p snitchcraft compare`
Expected: the 4 tests PASS once it compiles (this step's code is complete; if `call_context` or `ContextFacts::of_call` are private, make them `pub(crate)`).

- [ ] **Step 3: Add `compare_side` to `capture_sink.rs`**

```rust
/// One side of a comparison, from the open session when it is that one.
/// Errors are not labelled with the side; the command does that.
pub fn compare_side(
    root: &Path,
    store: Option<&CaptureStore>,
    cache: &RecordCache,
    shared: &Shared,
    run: &RunRef,
) -> Result<SideSummary, SessionError> {
    session_path(root, &run.project, &run.session_id)?;
    if let Some(id) = &run.prompt_trace_id {
        check_trace_id(id)?;
    }
    let records = measured_records(store, cache, &run.session_id)?;
    copy_from_trace(root, shared, &run.project, &run.session_id, |trace| {
        compare::side_summary(trace, run.prompt_trace_id.as_deref(), &records)
    })
}
```

with `use crate::compare::{self, RunRef, SideSummary};` added to the imports.

Add a test in `capture_sink.rs`'s test module if it has one that builds a `Shared` and `RecordCache` for the fixture root (follow the existing `session_flags_view` or `session_context_view` tests there); otherwise add it in `compare.rs` tests only if constructing `Shared` is simple. The test: `compare_side` for `("basic", "nope")` returns `SessionError::NotFound`.

- [ ] **Step 4: Add the command**

In `main.rs`, after `session_flags`:

```rust
/// Two runs side by side: each a whole session or one of its prompts, with
/// setup, work, tokens, problems and the final message. Works when capture
/// is off.
#[tauri::command(rename_all = "snake_case")]
async fn compare_runs(
    paths: tauri::State<'_, AppPaths>,
    shared: tauri::State<'_, Arc<Shared>>,
    captures: tauri::State<'_, Captures>,
    a: compare::RunRef,
    b: compare::RunRef,
) -> Result<compare::CompareView, String> {
    let root = paths.root().map_err(|e| e.to_string())?;
    let store = captures.store.clone();
    let cache = Arc::clone(&captures.cache);
    let shared = Arc::clone(&shared);
    let started = std::time::Instant::now();
    let view = run_blocking(move || {
        let side = |run: &compare::RunRef, name: &'static str| {
            capture_sink::compare_side(&root, store.as_deref(), &cache, &shared, run).map_err(|e| {
                SessionError::InRun {
                    side: name,
                    source: Box::new(e),
                }
            })
        };
        let a = side(&a, "A")?;
        let b = side(&b, "B")?;
        Ok(compare::compare_view(a, b))
    })
    .await?;
    tracing::info!(ms = started.elapsed().as_millis() as u64, "compared two runs");
    Ok(view)
}
```

Add `compare_runs` to `tauri::generate_handler![...]` after `session_flags`, and `"compare_runs"` to the list in `src-tauri/build.rs` after `"session_flags"`. In `capabilities/default.json` add `"allow-compare-runs"` after `"allow-session-flags"` and extend the description's sentence to "...the context breakdown and waste flags of the session and its model calls, compare two sessions or prompts, see the capture proxy's status...".

Run: `cargo build -p snitchcraft`
Expected: builds. If Tauri writes `src-tauri/permissions/autogenerated/compare_runs.toml`, keep it.

- [ ] **Step 5: Mock snapshot `compare-data.json`**

In `src-tauri/src/sessions.rs`'s test module, after `ui_flags_data_matches_the_backend`, add:

```rust
    /// `ui/src/mock/compare-data.json` holds the whole-session comparison of
    /// the two fixture sessions in both orders, keyed by
    /// `<project>/<session id>|<project>/<session id>`, for the dev mock mode.
    #[test]
    fn ui_compare_data_matches_the_backend() {
        let root = fixture_root();
        let side = |project: &str, session_id: &str| {
            let loaded = load_trace(&root, project, session_id).expect("trace");
            crate::compare::side_summary(&loaded.trace, None, &crate::captures::SessionRecords::default())
        };
        let [first, second] = FIXTURE_SESSIONS;
        let mut all = serde_json::Map::new();
        for (a, b) in [(first, second), (second, first)] {
            let view = crate::compare::compare_view(side(a.0, a.1), side(b.0, b.1));
            all.insert(
                format!("{}/{}|{}/{}", a.0, a.1, b.0, b.1),
                serde_json::to_value(view).expect("serialise"),
            );
        }
        check_snapshot(&mock_file("compare-data.json"), &serde_json::Value::Object(all));
    }
```

Run in PowerShell:

```powershell
$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS
cargo test -p snitchcraft
git diff --stat ui/src/mock
```

Expected: `compare-data.json` is new; the other mock files are unchanged (if any changed, stop and find out why). Open `compare-data.json` and check by eye: the basic session's `prompts` count and model call count match its diagram, and the compaction session shows `compactions: 1` and non-zero flag counts.

- [ ] **Step 6: Full checks and commit**

Run: `cargo fmt --all; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace`

```powershell
git add src-tauri ui/src/mock/compare-data.json
git commit -m "Add the compare_runs command" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: UI types, API, mock and pure helpers

**Files:**
- Modify: `ui/src/types.ts`
- Modify: `ui/src/api.ts`
- Modify: `ui/src/mock/mockApi.ts`
- Create: `ui/src/compare.ts`
- Create: `ui/src/compare.test.ts`

**Interfaces:**
- Consumes: the JSON from Task 3 and `formatTokens`, `formatDuration` from `ui/src/format.ts`.
- Produces:
  - Types `RunRef`, `CompareView`, `CompareSide`, `PromptChoice`, `Comparison`, `RunSummary`, `CompareSection`, `CompareRow`, `CallPoint`, `Total`, `Named`, `KindCount`, `CompareUnit`, `Higher`, `Presence` in `types.ts`, matching the Rust JSON field for field (snake_case).
  - `Api.compareRuns(a: RunRef, b: RunRef): Promise<CompareView>`; Tauri call `invoke<CompareView>('compare_runs', { a, b })` where each `RunRef` is `{ project, session_id, prompt_trace_id }`.
  - In `compare.ts`:
    - `scopeOptions(prompts: PromptChoice[]): { value: string; label: string }[]`: first `{ value: '', label: 'Whole session' }`, then `{ value: turn_id, label: 'Prompt <index+1>: <preview>' }`.
    - `formatValue(value: number | null, unit: CompareUnit): string`: `'?'` for null; tokens via `formatTokens`; ms via `formatDuration`; counts as plain integers with no grouping (`String(value)`). If `formatTokens` or `formatDuration` returns null for a number, fall back to `String(value)`.
    - `formatDifference(row: CompareRow): string`: `''` when `difference` is null; `'same'` when 0; otherwise a sign (`+` or `-`, a plain hyphen-minus) and the absolute value formatted with `formatValue`.
    - `presenceTag(row: CompareRow): string | null`: `'only in A'`, `'only in B'`, or null.
    - `visibleSections(sections: CompareSection[]): CompareSection[]`: drops sections with no rows.
    - `chartLines(a: CallPoint[], b: CallPoint[], width: number, height: number): { a: string; b: string; maxTokens: number }`: SVG `points` strings. x for call index i (1-based) of n = max(a.length, b.length) is `n <= 1 ? 0 : ((i - 1) / (n - 1)) * width`; y is `height - (tokens / maxTokens) * height`; points with null tokens are skipped; numbers rounded to 1 decimal; `maxTokens` is the largest known token count over both (0 if none, and then both strings are empty).

- [ ] **Step 1: Types and API**

Add to `ui/src/types.ts` (after `SessionFlags`):

```ts
/** One side of a comparison, as sent to `compare_runs`. */
export interface RunRef {
  project: string
  session_id: string
  /** A prompt's turn id, or null for the whole session. */
  prompt_trace_id: string | null
}

export interface Total {
  value: number | null
  /** True when only some calls reported a value. */
  partial: boolean
}

export interface Named {
  name: string
  value: number
}

export interface CallPoint {
  index: number
  trace_id: string
  tokens: number | null
}

export interface KindCount {
  kind: FlagKind
  count: number
}

export interface RunSummary {
  first_call_source: ContextSource | null
  first_call_tokens: number | null
  system_prompt_tokens: number | null
  tool_definition_tokens: number | null
  instruction_tokens: number | null
  other_context_tokens: number | null
  tool_count: number | null
  tool_groups: Named[] | null
  instruction_parts: Named[] | null
  models: Named[]
  prompts: number
  model_calls: number
  tool_calls: number
  subagents: number
  tools_by_name: Named[]
  wall_ms: number | null
  model_ms: Total
  tool_ms: Total
  input_tokens: Total
  output_tokens: Total
  cache_read_tokens: Total
  cache_write_tokens: Total
  largest_context: number | null
  context_per_call: CallPoint[]
  flags_by_kind: KindCount[]
  failed_tool_calls: number
  compactions: number
  final_message: string | null
}

export type CompareUnit = 'count' | 'tokens' | 'ms'
export type Higher = 'a' | 'b' | 'equal' | 'unknown'
export type Presence = 'both' | 'only_a' | 'only_b'

export interface CompareRow {
  label: string
  unit: CompareUnit
  a: number | null
  b: number | null
  a_partial: boolean
  b_partial: boolean
  /** b - a, when both are known. */
  difference: number | null
  higher: Higher
  presence: Presence
}

export interface CompareSection {
  title: string
  rows: CompareRow[]
}

export interface Comparison {
  a: RunSummary
  b: RunSummary
  sections: CompareSection[]
}

export interface PromptChoice {
  turn_id: string
  /** From 0, as in the diagram. */
  index: number
  preview: string
}

export interface CompareSide {
  prompts: PromptChoice[]
  /** The asked-for prompt was not found; the whole session is shown. */
  scope_missing: boolean
}

/** The response of `compare_runs`. */
export interface CompareView {
  comparison: Comparison
  a: CompareSide
  b: CompareSide
}
```

In `api.ts`: import `CompareView` and `RunRef`, add to `Api`:

```ts
  /** Two runs side by side; each a whole session or one prompt. */
  compareRuns(a: RunRef, b: RunRef): Promise<CompareView>
```

and to `tauriApi`:

```ts
  compareRuns: (a, b) => invoke<CompareView>('compare_runs', { a, b }),
```

- [ ] **Step 2: Mock**

In `mockApi.ts`: import `compareData from './compare-data.json'`, add `const comparisons = compareData as unknown as Record<string, CompareView>`, mention `compare-data.json` in the header comment and `compare` in the `fail=` list, and add:

```ts
    // The mock only has whole-session comparisons of the two sample sessions,
    // so a prompt scope is ignored here.
    async compareRuns(a, b) {
      await wait()
      if (fail === 'compare') throw 'Run B: could not load session: mock failure'
      const found = comparisons[`${a.project}/${a.session_id}|${b.project}/${b.session_id}`]
      if (!found) throw 'Run B: the mock has no comparison for these sessions'
      return found
    },
```

- [ ] **Step 3: Write failing tests for the helpers**

Create `ui/src/compare.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { chartLines, formatDifference, formatValue, presenceTag, scopeOptions, visibleSections } from './compare.ts'
import type { CompareRow } from './types.ts'

const row = (over: Partial<CompareRow>): CompareRow => ({
  label: 'Output',
  unit: 'count',
  a: 1,
  b: 1,
  a_partial: false,
  b_partial: false,
  difference: 0,
  higher: 'equal',
  presence: 'both',
  ...over,
})

describe('scopeOptions', () => {
  it('starts with the whole session and numbers prompts from 1', () => {
    expect(scopeOptions([{ turn_id: 't1', index: 0, preview: 'Fix the parser' }])).toEqual([
      { value: '', label: 'Whole session' },
      { value: 't1', label: 'Prompt 1: Fix the parser' },
    ])
  })
})

describe('formatValue', () => {
  it('shows unknown as a question mark', () => {
    expect(formatValue(null, 'tokens')).toBe('?')
  })
  it('shows counts as plain numbers', () => {
    expect(formatValue(12, 'count')).toBe('12')
  })
  it('formats tokens and durations like the rest of the app', () => {
    expect(formatValue(1500, 'tokens')).not.toBe('1500')
    expect(formatValue(65_000, 'ms')).not.toBe('65000')
  })
})

describe('formatDifference', () => {
  it('is empty when unknown and "same" when equal', () => {
    expect(formatDifference(row({ difference: null, higher: 'unknown' }))).toBe('')
    expect(formatDifference(row({}))).toBe('same')
  })
  it('signs the difference', () => {
    expect(formatDifference(row({ a: 5, b: 2, difference: -3, higher: 'a' }))).toBe('-3')
    expect(formatDifference(row({ a: 2, b: 5, difference: 3, higher: 'b' }))).toBe('+3')
  })
})

describe('presenceTag', () => {
  it('names the only side', () => {
    expect(presenceTag(row({ presence: 'only_a' }))).toBe('only in A')
    expect(presenceTag(row({ presence: 'only_b' }))).toBe('only in B')
    expect(presenceTag(row({}))).toBeNull()
  })
})

describe('visibleSections', () => {
  it('drops empty sections', () => {
    expect(visibleSections([{ title: 'Models', rows: [] }, { title: 'Work', rows: [row({})] }]).map((s) => s.title)).toEqual(['Work'])
  })
})

describe('chartLines', () => {
  it('spreads calls over the width and scales to the largest value', () => {
    const lines = chartLines(
      [
        { index: 1, trace_id: 'a1', tokens: 100 },
        { index: 2, trace_id: 'a2', tokens: 200 },
      ],
      [
        { index: 1, trace_id: 'b1', tokens: 50 },
        { index: 2, trace_id: 'b2', tokens: null },
        { index: 3, trace_id: 'b3', tokens: 100 },
      ],
      200,
      100,
    )
    expect(lines.maxTokens).toBe(200)
    expect(lines.a).toBe('0,50 100,0')
    expect(lines.b).toBe('0,75 200,50')
  })
  it('is empty with no known values', () => {
    expect(chartLines([{ index: 1, trace_id: 'a', tokens: null }], [], 10, 10)).toEqual({ a: '', b: '', maxTokens: 0 })
  })
  it('puts a single call at the left edge', () => {
    expect(chartLines([{ index: 1, trace_id: 'a', tokens: 10 }], [], 10, 10).a).toBe('0,0')
  })
})
```

Run: `cd ui; npm test -- compare`
Expected: FAIL (module `./compare.ts` not found).

- [ ] **Step 4: Implement `ui/src/compare.ts`**

```ts
// Formatting for the Compare view. Every number, difference and "only in"
// tag comes from the backend; these helpers only turn them into text and
// chart coordinates.

import { formatDuration, formatTokens } from './format.ts'
import type { CallPoint, CompareRow, CompareSection, CompareUnit, PromptChoice } from './types.ts'

export function scopeOptions(prompts: PromptChoice[]): { value: string; label: string }[] {
  return [
    { value: '', label: 'Whole session' },
    ...prompts.map((p) => ({ value: p.turn_id, label: `Prompt ${p.index + 1}: ${p.preview}` })),
  ]
}

export function formatValue(value: number | null, unit: CompareUnit): string {
  if (value === null) return '?'
  if (unit === 'tokens') return formatTokens(value) ?? String(value)
  if (unit === 'ms') return formatDuration(value) ?? String(value)
  return String(value)
}

export function formatDifference(row: CompareRow): string {
  if (row.difference === null) return ''
  if (row.difference === 0) return 'same'
  const sign = row.difference > 0 ? '+' : '-'
  return `${sign}${formatValue(Math.abs(row.difference), row.unit)}`
}

export function presenceTag(row: CompareRow): string | null {
  if (row.presence === 'only_a') return 'only in A'
  if (row.presence === 'only_b') return 'only in B'
  return null
}

export function visibleSections(sections: CompareSection[]): CompareSection[] {
  return sections.filter((s) => s.rows.length > 0)
}

const round = (n: number) => Math.round(n * 10) / 10

export function chartLines(
  a: CallPoint[],
  b: CallPoint[],
  width: number,
  height: number,
): { a: string; b: string; maxTokens: number } {
  const known = [...a, ...b].flatMap((p) => (p.tokens === null ? [] : [p.tokens]))
  const maxTokens = known.length === 0 ? 0 : Math.max(...known)
  if (maxTokens === 0) return { a: '', b: '', maxTokens: 0 }
  const n = Math.max(a.length, b.length)
  const line = (points: CallPoint[]) =>
    points
      .flatMap((p) => {
        if (p.tokens === null) return []
        const x = n <= 1 ? 0 : ((p.index - 1) / (n - 1)) * width
        const y = height - (p.tokens / maxTokens) * height
        return [`${round(x)},${round(y)}`]
      })
      .join(' ')
  return { a: line(a), b: line(b), maxTokens }
}
```

- [ ] **Step 5: Run checks and commit**

Run: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`
Expected: all pass. The `formatTokens` / `formatDuration` checks in the test only assert the value is formatted; if one of them returns the bare number for these inputs, read `format.ts` and change the test input, not the helper.

```powershell
git add ui/src/types.ts ui/src/api.ts ui/src/mock/mockApi.ts ui/src/compare.ts ui/src/compare.test.ts
git commit -m "Add the compare API, mock and formatting helpers to the UI" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Compare view, picker and flow

**Files:**
- Create: `ui/src/components/CompareView.tsx`
- Modify: `ui/src/components/Sidebar.tsx` (pick mode for `SessionList`, "Compare with..." button in `SessionPanel`)
- Modify: `ui/src/App.tsx` (state and wiring)
- Modify: `ui/src/app.css` (styles)
- Modify: `ui/src/compare.ts` and `ui/src/compare.test.ts` (one more helper, below)

**Interfaces:**
- Consumes: `Api.compareRuns`, the helpers from Task 4, `throttleDelay` from `captureView.ts`, `SessionSummary`.
- Produces: the user-visible flow from the spec.

Behaviour:

1. `SessionPanel` gets a prop `onCompare: () => void` and shows a `Compare with...` button (class `link-button`, `title="Compare this session with another one"`) in the `sidebar-heading` row next to Reload.
2. `SessionList` gets an optional prop `pick?: { disabledKey: string; onCancel: () => void }`. In pick mode: the heading `h2` reads `Pick a session to compare with`, a `Cancel` link-button replaces Refresh, the capture start box is hidden, and the session whose `${project}/${session_id}` equals `disabledKey` is rendered with `disabled` and the class `item-disabled`.
3. `App.tsx` state: `picking: boolean`, `compareWith: SessionSummary | null`. The sidebar shows `SessionList` in pick mode when `session !== null && picking`; picking a session sets `compareWith` and `picking = false`. Opening another session, closing the session, or `Back to diagram` clears both. While `compareWith` is set, the main area shows `CompareView` instead of the diagram and no details panel is shown (clear `selected`, `selectedCapture`, `selectedHidden` when entering compare).
4. `CompareView` props: `api`, `left: SessionSummary`, `right: SessionSummary`, `refreshKey: string`, `onBack: () => void`. State: `swapped: boolean`, `scopes: { left: string; right: string }` (turn ids, `''` for whole session), `result: Loadable<CompareView>`, a `token` ref. It calls `api.compareRuns(refOf(a, scopeA), refOf(b, scopeB))` where A is `left` unless `swapped`. On every new request it bumps the token and ignores responses whose token is not the latest. When a side's `scope_missing` is true, it resets that side's scope to `''` and shows `That prompt is no longer in the session; showing the whole session.` above the table.
5. Refresh: `App.tsx` passes `refreshKey` = `${view version of the open session}|${modified_ms of compareWith from the latest session list}`. When `refreshKey` changes, `CompareView` refetches at most once every 2000 ms (use `throttleDelay`; keep a pending timer, clear it on unmount). The first load and scope/swap changes fetch at once and show loading only on the first load (later refreshes keep the old table until the new one arrives).
6. Layout of `CompareView`:
   - A toolbar: `Back to diagram` button, `Swap` button, title `Compare`.
   - A header with two columns, A and B: tag `A` / `B`, the session title (or `Session <shortId>`), project label, and a `<select>` with `scopeOptions(side.prompts)`, `aria-label="Scope of run A"` / `"...B"`. Below each, a small muted note with `first_call_source`: `Setup from a captured API call` or `Setup estimated from the transcript`, or `No model call to read the setup from` when null.
   - Errors: if the request fails, show the error in a `notice notice-error` with a `Try again` button; the header still shows both sessions and their scope selects (using the last good prompts if any).
   - For each `visibleSections(...)` section: an `h3` and a `table` with columns: label, A, B, Difference. Each value cell shows `formatValue`; when `*_partial` is true append ` *` and give the cell `title="Some calls did not report this; the total counts the ones that did"`. The cell of the higher side (row `higher` is `a` or `b`) gets the class `compare-higher` (a light neutral background, not red or green). A `presenceTag` shows as a small tag after the label.
   - After the Tokens section table: the context chart, an `svg` 600 by 120 (`viewBox="0 0 600 120"`, `width="100%"`), with two `polyline`s from `chartLines(a.context_per_call, b.context_per_call, 600, 120)` (classes `chart-a` and `chart-b`, `fill="none"`), a legend `A` / `B`, and the caption `Context per model call of the main agent (largest: <formatValue(maxTokens,'tokens')>)`. If `maxTokens` is 0, show `No context sizes reported.` instead.
   - Outcome: two `details` elements side by side (each `summary` is `Final message of A` / `B`), the text in a `pre` with `white-space: pre-wrap` and a max height with scrolling; `No final message.` when null.
   - Narrow screens (max-width 900px): the two header columns and the outcome stack vertically; the table scrolls horizontally inside its own container, not the page.
7. CSS uses the existing colour tokens in `app.css` (read the `:root` block first and reuse its variables; define `--compare-a` and `--compare-b` line colours there, with dark mode values beside the existing dark-mode tokens).

Add to `compare.ts` (with tests in `compare.test.ts`):

```ts
/** The request for one side: the whole session when `scope` is empty. */
export function runRef(session: { project: string; session_id: string }, scope: string): RunRef {
  return { project: session.project, session_id: session.session_id, prompt_trace_id: scope === '' ? null : scope }
}
```

Tests: `runRef(s, '')` gives `prompt_trace_id: null`; `runRef(s, 't1')` gives `'t1'`.

- [ ] **Step 1: Add `runRef` with its test, run `npm test -- compare`, see it fail, implement, see it pass.**

- [ ] **Step 2: Sidebar pick mode and the Compare with... button** as in Behaviour 1 and 2.

- [ ] **Step 3: `CompareView.tsx`** as in Behaviour 4 to 6.

- [ ] **Step 4: Wire `App.tsx`** as in Behaviour 3 and 5, and the styles (Behaviour 7).

- [ ] **Step 5: Checks**

Run: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`
Expected: all pass.

- [ ] **Step 6: Try it in the mock**

Run `cd ui; npm run dev` (in the background), open `http://localhost:5173/?mock` in a browser, open the basic sample session, click `Compare with...`, check the open session is greyed out, pick the compaction sample session, and check: sections and rows render, unknown shows `?`, the higher side is highlighted, the chart draws two lines, the final messages open, `Swap` swaps the columns (the mock has both orders), `Back to diagram` returns to the basic session as it was, and `?mock&fail=compare` shows the error with `Try again` while both session names stay in the header. Check a 390px wide window: no page-wide horizontal scroll. Stop the dev server afterwards.

- [ ] **Step 7: Commit**

```powershell
git add ui/src
git commit -m "Add the Compare view and the session picker" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Docs

**Files:**
- Modify: `docs/VIEW-MODEL.md`
- Modify: `docs/DECISIONS.md`
- Modify: `CLAUDE.md`
- Modify: `README.md`

- [ ] **Step 1: `docs/VIEW-MODEL.md`**: add a `compare_runs` section in the style of the existing `session_flags` section: the request (`a`, `b` as `RunRef`), the `CompareView` / `CompareSide` / `PromptChoice` / `Comparison` / `Section` / `Row` / `RunSummary` shapes with one line per field, what "main run", "subagent", "partial" and "unknown" mean, that named rows show 0 and an "only in" tag when a name is on one side only, and that a missing prompt falls back to the whole session with `scope_missing`.
- [ ] **Step 2: `docs/DECISIONS.md`**: add an M5.3 entry: comparison logic in `insights` (harness-neutral, pure, so the owner's harness traces compare too), rows built in Rust so the UI does no maths, the chart is plain SVG (no chart dependency), the higher side is highlighted neutrally because more is not always worse, setup slices not seen in the first call are unknown rather than 0, and one command fails as a whole with the side named in the error.
- [ ] **Step 3: `CLAUDE.md`**: under Current milestone, change the 5.3 line to `5.3 Compare two runs: complete. ...` (one sentence like the 5.2 line, naming `insights::compare`, `compare_runs`, the picker and the Compare view, and the spec path), and keep `5.3b step-by-step alignment, 5.4 shareable report: planned; plan each with me before starting.` In the Commands section, add `compare-data.json` to the mock file list in the snapshot refresh line, and add to the mock line: "open a sample session and use Compare with... to compare the two sample sessions".
- [ ] **Step 4: `README.md`**: update the milestone status for 5.3 in the same style as 5.2.
- [ ] **Step 5: Check and commit**

Run: `git grep -nP "x{2014}" -- docs README.md CLAUDE.md` (expect no new matches) and re-read the changed sections.

```powershell
git add docs CLAUDE.md README.md
git commit -m "Document comparing two runs" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
