# M5.3b Step Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Line two runs up step by step in the Compare view, mark the first divergence, and jump from a step to its box in either run's diagram.

**Architecture:** A new pure module `insights::align` turns each side's main-run tool calls into keyed steps (tool + target, target fields from the adapter's `AlignRules`), pairs prompts and lines up steps with a longest-common-subsequence diff. The app copies each side's steps out under the session lock and aligns after both sides are read, returning `alignment` in `CompareView`. The UI renders a Steps section and handles jumps.

**Tech Stack:** Rust (insights, adapter-claude-code, Tauri 2 app), React + TypeScript + Vite, node:test.

**Spec:** `docs/superpowers/specs/2026-10-12-m5-3b-step-alignment-design.md`

## Global Constraints

- No schema change: `crates/trace-core` is not touched.
- `insights` stays harness-neutral and pure: no Claude Code names (tool names and the notification prefix come from the adapter's `AlignRules`), no I/O, no dependency on `trace-view`.
- No `unwrap()` / `expect()` outside tests. No em dashes anywhere. Doc comments on public items. No new dependencies.
- The UI does no maths on trace data; it renders what the backend sends.
- UI tests use `node:test` + `node:assert/strict` (see `ui/src/compare.test.ts`), not vitest.
- Fixtures are invented; nothing personal in the repo.
- Commit messages for a public reader, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- Checks: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; UI: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.

## Review Focus

- Parallel tool calls in a different order must not count as a divergence (Task 1 test).
- A prompt only one run has must not shift the lineup of later prompts (Task 1 test).
- A one-sided prompt pair with no steps still counts as a difference and the UI scrolls to its heading (Task 1 test, Task 5).
- Very long prompt pairs must not allocate a huge table: above `MAX_CELLS` the in-order fallback runs (Task 1 test with a low limit).
- Jumping to a side-B step must open session B and select the box once its diagram has loaded, and must not jump if the user opened something else in between (Task 5).

---

### Task 1: `insights::align`

**Files:**
- Create: `crates/insights/src/align.rs`
- Modify: `crates/insights/src/rules.rs` (add `AlignRules`, `TargetRule`)
- Modify: `crates/insights/src/compare.rs` (make `main_run` and `duration` `pub(crate)`)
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Produces (pub, re-exported from lib.rs): `AlignRules`, `TargetRule`, `MAX_CELLS`, `Step`, `PromptSide`, `PromptSteps`, `RunSteps`, `RowKind`, `AlignRow`, `PromptPair`, `DiffPoint`, `Alignment`, `run_steps(trace: &Trace, scope: &Scope, rules: &AlignRules) -> RunSteps`, `align_steps(a: &RunSteps, b: &RunSteps) -> Alignment`.

- [ ] **Step 1: Rules types** in `rules.rs`, following the style of `FlagRules`:

```rust
/// How to line up tool calls of two runs: which input fields name a
/// tool's target, and which prompts are notifications.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlignRules {
    /// Target fields per tool.
    pub targets: Vec<TargetRule>,
    /// Fields tried in order for a tool with no rule; the first present wins.
    pub fallback_fields: Vec<String>,
    /// A prompt whose trimmed text starts with one of these is a
    /// notification; notifications pair with each other by order.
    pub notification_prefixes: Vec<String>,
}

/// The input fields that name one tool's target.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetRule {
    /// Tool name.
    pub tool: String,
    /// Fields joined, in this order, when present.
    pub fields: Vec<String>,
}
```

- [ ] **Step 2: Write `align.rs` with types and failing tests.**

```rust
//! Lining up two runs step by step: each main-run tool call is a step
//! keyed by tool and target, prompts are paired first, then the steps of
//! each pair are lined up with a longest-common-subsequence diff. Harness
//! rules come in as `AlignRules`.

use serde::Serialize;
use serde_json::Value;
use trace_core::{ContentBlock, Node, Trace, TraceEvent};

use crate::compare::{Scope, duration, main_run};
use crate::rules::AlignRules;

/// Largest diff table (steps of A times steps of B) computed exactly.
/// Larger pairs are matched in order instead.
pub const MAX_CELLS: usize = 4_000_000;

/// Longest prompt preview, in characters.
const PREVIEW_CHARS: usize = 80;
/// Longest target shown in a step label, in characters.
const LABEL_TARGET_CHARS: usize = 40;

/// One step of a run as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    /// The tool call's trace id.
    pub trace_id: String,
    /// `Tool(target)` or `Tool`.
    pub label: String,
    /// The result is an error.
    pub failed: bool,
    /// No result yet.
    pub running: bool,
    /// How long it took, if known.
    pub duration_ms: Option<u64>,
}

/// One prompt of a side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptSide {
    /// The turn's trace id.
    pub turn_id: String,
    /// Its position among the main run's prompts, from 0.
    pub index: usize,
    /// One line of its text.
    pub preview: String,
}

/// One prompt with its keyed steps, ready to align.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSteps {
    /// The prompt.
    pub side: PromptSide,
    /// What it pairs on.
    pub key: String,
    /// Its steps with their keys, in order.
    pub steps: Vec<(String, Step)>,
}

/// Everything one side needs for aligning, copied out of the trace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunSteps {
    /// The prompts in scope, in order.
    pub prompts: Vec<PromptSteps>,
    /// True when the side is limited to one prompt.
    pub scoped: bool,
}

/// How a row lines up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// The same step in both runs.
    Both,
    /// A step only run A took.
    OnlyA,
    /// A step only run B took.
    OnlyB,
}

/// One row of a lineup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlignRow {
    /// How it lines up.
    pub kind: RowKind,
    /// Run A's step, if any.
    pub a: Option<Step>,
    /// Run B's step, if any.
    pub b: Option<Step>,
    /// Both took the step but exactly one failed.
    pub differs: bool,
}

/// A prompt of each run (or of one) with its steps lined up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptPair {
    /// Run A's prompt, if any.
    pub a: Option<PromptSide>,
    /// Run B's prompt, if any.
    pub b: Option<PromptSide>,
    /// The lineup.
    pub rows: Vec<AlignRow>,
    /// The first row that is not the same clean step; `Some(0)` for a
    /// prompt only one run has, even with no rows.
    pub first_difference: Option<usize>,
    /// Too long to diff exactly; matched in order.
    pub approximate: bool,
}

/// Where the runs first differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DiffPoint {
    /// Index into `pairs`.
    pub pair: usize,
    /// Index into that pair's rows (0 for a one-sided pair).
    pub row: usize,
}

/// Two runs lined up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Alignment {
    /// Prompt pairs in order.
    pub pairs: Vec<PromptPair>,
    /// The first difference over all pairs.
    pub first_difference: Option<DiffPoint>,
}
```

Tests (append at the end of the file). They build traces the same way `compare.rs` tests do:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::TargetRule;
    use serde_json::json;
    use trace_core::{ModelCall, Run, ToolCall, ToolResult, Turn};

    fn add(t: &mut Trace, id: &str, parent: Option<&str>, node: Node) {
        let event = TraceEvent {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            node,
            started_at_ms: None,
            ended_at_ms: None,
            raw: Vec::new(),
            metadata: Default::default(),
        };
        assert!(t.apply(event).is_ok());
    }

    fn rules() -> AlignRules {
        AlignRules {
            targets: vec![
                TargetRule { tool: "Open".into(), fields: vec!["file".into()] },
                TargetRule { tool: "Find".into(), fields: vec!["pattern".into(), "dir".into()] },
            ],
            fallback_fields: vec!["cmd".into()],
            notification_prefixes: vec!["<note>".into()],
        }
    }

    /// One run: prompts is a list of (prompt text, model calls), each model
    /// call a list of (tool, input, failed). Ids: T{p}, M{p}_{m}, X{p}_{m}_{t}.
    fn trace(prefix: &str, prompts: &[(&str, Vec<Vec<(&str, Value, Option<bool>)>>)]) -> Trace {
        let mut t = Trace::new();
        let root = format!("{prefix}R");
        add(&mut t, &root, None, Node::Run(Run { harness: "test".into(), title: None }));
        for (p, (text, calls)) in prompts.iter().enumerate() {
            let turn = format!("{prefix}T{p}");
            add(&mut t, &turn, Some(&root), Node::Turn(Turn { prompt: vec![ContentBlock::Text { text: (*text).into() }] }));
            for (m, tools) in calls.iter().enumerate() {
                let call = format!("{prefix}M{p}_{m}");
                add(&mut t, &call, Some(&turn), Node::ModelCall(ModelCall { model: None, output: Vec::new(), stop_reason: None, usage: None }));
                for (x, (name, input, failed)) in tools.iter().enumerate() {
                    let id = format!("{prefix}X{p}_{m}_{x}");
                    let result = failed.map(|is_error| ToolResult { content: Vec::new(), is_error });
                    add(&mut t, &id, Some(&call), Node::ToolCall(ToolCall { name: (*name).into(), input: input.clone(), result }));
                }
            }
        }
        t
    }

    fn open(file: &str) -> (&'static str, Value, Option<bool>) {
        ("Open", json!({ "file": file }), Some(false))
    }

    fn run(cmd: &str, failed: bool) -> (&'static str, Value, Option<bool>) {
        ("Shell", json!({ "cmd": cmd }), Some(failed))
    }

    fn lineup(a: &Trace, b: &Trace) -> Alignment {
        align_steps(&run_steps(a, &Scope::Session, &rules()), &run_steps(b, &Scope::Session, &rules()))
    }

    fn kinds(pair: &PromptPair) -> Vec<RowKind> {
        pair.rows.iter().map(|r| r.kind).collect()
    }

    use RowKind::{Both, OnlyA, OnlyB};

    #[test]
    fn identical_runs_have_no_difference() {
        let p = vec![("fix it", vec![vec![open("a.rs"), run("test", false)]])];
        let al = lineup(&trace("a", &p), &trace("b", &p));
        assert_eq!(al.pairs.len(), 1);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, Both]);
        assert_eq!(al.pairs[0].first_difference, None);
        assert_eq!(al.first_difference, None);
    }

    #[test]
    fn extra_steps_are_one_sided() {
        let a = trace("a", &[("fix it", vec![vec![open("a.rs")], vec![run("grep", false)], vec![run("test", false)]])]);
        let b = trace("b", &[("fix it", vec![vec![open("a.rs")], vec![run("test", false)], vec![open("b.rs")]])]);
        let al = lineup(&a, &b);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, OnlyA, Both, OnlyB]);
        assert_eq!(al.pairs[0].first_difference, Some(1));
        assert_eq!(al.first_difference, Some(DiffPoint { pair: 0, row: 1 }));
        assert_eq!(al.pairs[0].rows[1].a.as_ref().map(|s| s.label.as_str()), Some("Shell(grep)"));
        assert!(al.pairs[0].rows[1].b.is_none());
    }

    #[test]
    fn swapped_steps_show_as_one_sided() {
        let a = trace("a", &[("go", vec![vec![open("x")], vec![open("y")]])]);
        let b = trace("b", &[("go", vec![vec![open("y")], vec![open("x")]])]);
        let al = lineup(&a, &b);
        assert_eq!(al.pairs[0].rows.len(), 3);
        assert_eq!(al.pairs[0].rows.iter().filter(|r| r.kind == Both).count(), 1);
    }

    #[test]
    fn parallel_calls_in_another_order_match() {
        let a = trace("a", &[("go", vec![vec![open("x"), open("y")]])]);
        let b = trace("b", &[("go", vec![vec![open("y"), open("x")]])]);
        let al = lineup(&a, &b);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, Both]);
        assert_eq!(al.first_difference, None);
    }

    #[test]
    fn range_and_whitespace_do_not_matter() {
        let a = trace("a", &[("go", vec![vec![("Open", json!({"file": "a.rs", "offset": 10}), Some(false)), run("cargo   test", false)]])]);
        let b = trace("b", &[("go", vec![vec![("Open", json!({"file": "a.rs"}), Some(false)), run(" cargo test ", false)]])]);
        assert_eq!(lineup(&a, &b).first_difference, None);
    }

    #[test]
    fn a_failure_on_one_side_differs() {
        let a = trace("a", &[("go", vec![vec![run("test", true)]])]);
        let b = trace("b", &[("go", vec![vec![run("test", false)]])]);
        let al = lineup(&a, &b);
        assert_eq!(kinds(&al.pairs[0]), vec![Both]);
        assert!(al.pairs[0].rows[0].differs);
        assert_eq!(al.first_difference, Some(DiffPoint { pair: 0, row: 0 }));
    }

    #[test]
    fn an_extra_prompt_does_not_shift_later_ones() {
        let a = trace("a", &[("one", vec![vec![open("a")]]), ("extra", vec![]), ("two", vec![vec![open("b")]])]);
        let b = trace("b", &[("one", vec![vec![open("a")]]), ("two", vec![vec![open("b")]])]);
        let al = lineup(&a, &b);
        assert_eq!(al.pairs.len(), 3);
        assert!(al.pairs[1].a.is_some() && al.pairs[1].b.is_none());
        assert!(al.pairs[1].rows.is_empty());
        assert_eq!(al.pairs[1].first_difference, Some(0));
        assert_eq!(al.pairs[2].first_difference, None);
        assert_eq!(al.first_difference, Some(DiffPoint { pair: 1, row: 0 }));
    }

    #[test]
    fn notifications_pair_by_order() {
        let a = trace("a", &[("go", vec![]), ("<note> id 1", vec![vec![open("a")]])]);
        let b = trace("b", &[("go", vec![]), ("<note> id 2", vec![vec![open("a")]])]);
        let al = lineup(&a, &b);
        assert_eq!(al.pairs.len(), 2);
        assert!(al.pairs.iter().all(|p| p.a.is_some() && p.b.is_some()));
        assert_eq!(al.first_difference, None);
    }

    #[test]
    fn fallback_fields_and_tool_name_only() {
        let s = run_steps(&trace("a", &[("go", vec![vec![run("ls", false), ("Think", json!({}), None)]])]), &Scope::Session, &rules());
        let labels: Vec<&str> = s.prompts[0].steps.iter().map(|(_, st)| st.label.as_str()).collect();
        assert_eq!(labels, vec!["Shell(ls)", "Think"]);
        assert!(s.prompts[0].steps[1].1.running);
    }

    #[test]
    fn a_subagent_is_one_step() {
        let mut t = trace("a", &[("go", vec![vec![("Find", json!({"pattern": "x", "dir": "src"}), Some(false))]])]);
        add(&mut t, "S", Some("aX0_0_0"), Node::Run(Run { harness: "test".into(), title: None }));
        add(&mut t, "ST", Some("S"), Node::Turn(Turn { prompt: Vec::new() }));
        add(&mut t, "SM", Some("ST"), Node::ModelCall(ModelCall { model: None, output: Vec::new(), stop_reason: None, usage: None }));
        add(&mut t, "SX", Some("SM"), Node::ToolCall(ToolCall { name: "Open".into(), input: json!({"file": "q"}), result: None }));
        let s = run_steps(&t, &Scope::Session, &rules());
        assert_eq!(s.prompts[0].steps.len(), 1);
        assert_eq!(s.prompts[0].steps[0].1.label, "Find(x / src)");
    }

    #[test]
    fn prompt_scopes_pair_whatever_the_text() {
        let a = trace("a", &[("one", vec![vec![open("a")]]), ("two", vec![vec![open("b")]])]);
        let b = trace("b", &[("other", vec![vec![open("b")]])]);
        let sa = run_steps(&a, &Scope::Prompt("aT1".into()), &rules());
        let sb = run_steps(&b, &Scope::Prompt("bT0".into()), &rules());
        assert!(sa.scoped && sb.scoped);
        let al = align_steps(&sa, &sb);
        assert_eq!(al.pairs.len(), 1);
        assert_eq!(al.pairs[0].a.as_ref().map(|p| p.index), Some(1));
        assert_eq!(kinds(&al.pairs[0]), vec![Both]);
    }

    #[test]
    fn huge_pairs_fall_back_to_in_order_matching() {
        let a = trace("a", &[("go", vec![vec![open("a")], vec![open("b")], vec![open("c")]])]);
        let b = trace("b", &[("go", vec![vec![open("a")], vec![open("x")], vec![open("c")]])]);
        let al = align_with_limit(&run_steps(&a, &Scope::Session, &rules()), &run_steps(&b, &Scope::Session, &rules()), 4);
        assert!(al.pairs[0].approximate);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, OnlyA, OnlyB, Both]);
        let exact = lineup(&a, &b);
        assert!(!exact.pairs[0].approximate);
    }
}
```

Run: `cargo test -p insights align` - Expected: FAIL to compile.

- [ ] **Step 3: Implement** (above the tests):

```rust
/// Runs of whitespace collapsed to one space, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// At most `max` characters, ending in "..." if cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{}...", cut.trim_end())
}

/// A field's value as text: strings as they are, anything else as JSON.
fn field_text(input: &Value, field: &str) -> Option<String> {
    match input.get(field)? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// The tool's target from its input, collapsed; empty if none.
fn target(name: &str, input: &Value, rules: &AlignRules) -> String {
    let text = match rules.targets.iter().find(|r| r.tool == name) {
        Some(rule) => rule
            .fields
            .iter()
            .filter_map(|f| field_text(input, f))
            .collect::<Vec<_>>()
            .join(" / "),
        None => rules
            .fallback_fields
            .iter()
            .find_map(|f| field_text(input, f))
            .unwrap_or_default(),
    };
    collapse(&text)
}

/// The prompt's text, collapsed.
fn prompt_text(blocks: &[ContentBlock]) -> String {
    let text = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    collapse(&text)
}

/// The keyed steps of one turn: each model call's tool calls, sorted by
/// key within the call. Nothing inside a tool call (such as a subagent run)
/// is a step.
fn turn_steps(trace: &Trace, turn_id: &str, rules: &AlignRules) -> Vec<(String, Step)> {
    let mut steps = Vec::new();
    for call in trace.children(turn_id) {
        if !matches!(call.node, Node::ModelCall(_)) {
            continue;
        }
        let mut group: Vec<(String, Step)> = trace
            .children(&call.id)
            .filter_map(|event| tool_step(event, rules))
            .collect();
        group.sort_by(|x, y| x.0.cmp(&y.0));
        steps.extend(group);
    }
    steps
}

fn tool_step(event: &TraceEvent, rules: &AlignRules) -> Option<(String, Step)> {
    let Node::ToolCall(call) = &event.node else {
        return None;
    };
    let target = target(&call.name, &call.input, rules);
    let (key, label) = if target.is_empty() {
        (call.name.clone(), call.name.clone())
    } else {
        (
            format!("{} {}", call.name, target),
            format!("{}({})", call.name, shorten(&target, LABEL_TARGET_CHARS)),
        )
    };
    Some((
        key,
        Step {
            trace_id: event.id.clone(),
            label,
            failed: call.result.as_ref().is_some_and(|r| r.is_error),
            running: call.result.is_none(),
            duration_ms: duration(event),
        },
    ))
}

/// One side's prompts and steps in scope, copied out of the trace.
pub fn run_steps(trace: &Trace, scope: &Scope, rules: &AlignRules) -> RunSteps {
    let Some(main) = main_run(trace) else {
        return RunSteps::default();
    };
    let mut prompts = Vec::new();
    let turns = trace.children(main).filter(|e| matches!(e.node, Node::Turn(_)));
    for (index, turn) in turns.enumerate() {
        if let Scope::Prompt(id) = scope {
            if &turn.id != id {
                continue;
            }
        }
        let Node::Turn(t) = &turn.node else {
            continue;
        };
        let text = prompt_text(&t.prompt);
        let is_notification = rules
            .notification_prefixes
            .iter()
            .any(|p| text.starts_with(p.as_str()));
        prompts.push(PromptSteps {
            side: PromptSide {
                turn_id: turn.id.clone(),
                index,
                preview: shorten(&text, PREVIEW_CHARS),
            },
            key: if is_notification { "notification".to_string() } else { text },
            steps: turn_steps(trace, &turn.id, rules),
        });
    }
    RunSteps {
        prompts,
        scoped: matches!(scope, Scope::Prompt(_)),
    }
}

/// Lines up two key lists. Each item is (index in A, index in B), one of
/// them `None` for a one-sided item. The flag is true when the table would
/// exceed `limit` cells and in-order matching was used instead.
fn diff(a: &[&str], b: &[&str], limit: usize) -> (Vec<(Option<usize>, Option<usize>)>, bool) {
    let (n, m) = (a.len(), b.len());
    let cells = (n + 1).checked_mul(m + 1);
    let mut out = Vec::new();
    match cells {
        Some(cells) if cells <= limit => {
            // dp[i][j] = longest common subsequence of a[i..] and b[j..].
            let w = m + 1;
            let mut dp = vec![0u32; cells];
            for i in (0..n).rev() {
                for j in (0..m).rev() {
                    dp[i * w + j] = if a[i] == b[j] {
                        dp[(i + 1) * w + j + 1] + 1
                    } else {
                        dp[(i + 1) * w + j].max(dp[i * w + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < n && j < m {
                if a[i] == b[j] {
                    out.push((Some(i), Some(j)));
                    i += 1;
                    j += 1;
                } else if dp[(i + 1) * w + j] >= dp[i * w + j + 1] {
                    out.push((Some(i), None));
                    i += 1;
                } else {
                    out.push((None, Some(j)));
                    j += 1;
                }
            }
            out.extend((i..n).map(|i| (Some(i), None)));
            out.extend((j..m).map(|j| (None, Some(j))));
            (out, false)
        }
        _ => {
            let shared = n.min(m);
            for k in 0..shared {
                if a[k] == b[k] {
                    out.push((Some(k), Some(k)));
                } else {
                    out.push((Some(k), None));
                    out.push((None, Some(k)));
                }
            }
            out.extend((shared..n).map(|i| (Some(i), None)));
            out.extend((shared..m).map(|j| (None, Some(j))));
            (out, true)
        }
    }
}

fn line_up(a: Option<&PromptSteps>, b: Option<&PromptSteps>, limit: usize) -> PromptPair {
    let empty = Vec::new();
    let sa = a.map_or(&empty, |p| &p.steps);
    let sb = b.map_or(&empty, |p| &p.steps);
    let ka: Vec<&str> = sa.iter().map(|(k, _)| k.as_str()).collect();
    let kb: Vec<&str> = sb.iter().map(|(k, _)| k.as_str()).collect();
    let (ops, approximate) = diff(&ka, &kb, limit);
    let rows: Vec<AlignRow> = ops
        .into_iter()
        .map(|(i, j)| {
            let a = i.and_then(|i| sa.get(i)).map(|(_, s)| s.clone());
            let b = j.and_then(|j| sb.get(j)).map(|(_, s)| s.clone());
            let kind = match (&a, &b) {
                (Some(_), Some(_)) => RowKind::Both,
                (Some(_), None) => RowKind::OnlyA,
                _ => RowKind::OnlyB,
            };
            let differs = match (&a, &b) {
                (Some(x), Some(y)) => x.failed != y.failed,
                _ => false,
            };
            AlignRow { kind, a, b, differs }
        })
        .collect();
    let first_difference = if a.is_none() || b.is_none() {
        Some(0)
    } else {
        rows.iter().position(|r| r.kind != RowKind::Both || r.differs)
    };
    PromptPair {
        a: a.map(|p| p.side.clone()),
        b: b.map(|p| p.side.clone()),
        rows,
        first_difference,
        approximate,
    }
}

/// Two sides lined up: prompts paired, then each pair's steps.
pub fn align_steps(a: &RunSteps, b: &RunSteps) -> Alignment {
    align_with_limit(a, b, MAX_CELLS)
}

fn align_with_limit(a: &RunSteps, b: &RunSteps, limit: usize) -> Alignment {
    let pairs_idx = if a.scoped && b.scoped && a.prompts.len() == 1 && b.prompts.len() == 1 {
        vec![(Some(0), Some(0))]
    } else {
        let ka: Vec<&str> = a.prompts.iter().map(|p| p.key.as_str()).collect();
        let kb: Vec<&str> = b.prompts.iter().map(|p| p.key.as_str()).collect();
        diff(&ka, &kb, limit).0
    };
    let pairs: Vec<PromptPair> = pairs_idx
        .into_iter()
        .map(|(i, j)| {
            line_up(
                i.and_then(|i| a.prompts.get(i)),
                j.and_then(|j| b.prompts.get(j)),
                limit,
            )
        })
        .collect();
    let first_difference = pairs
        .iter()
        .enumerate()
        .find_map(|(pair, p)| p.first_difference.map(|row| DiffPoint { pair, row }));
    Alignment { pairs, first_difference }
}
```

Note: the in-order fallback for prompt pairing is approximate too, but `approximate` is a per-pair flag about steps; prompt pairing above the limit (over about 2,000 prompts a side) is not flagged. That is acceptable.

In `compare.rs` change `fn main_run` and `fn duration` to `pub(crate) fn`. In `lib.rs` add `pub mod align;` and re-export:

```rust
pub use align::{
    AlignRow, Alignment, DiffPoint, MAX_CELLS, PromptPair, PromptSide, PromptSteps, RowKind,
    RunSteps, Step, align_steps, run_steps,
};
```
and add `AlignRules, TargetRule` to the `rules` re-export. Extend the crate doc's last sentence with "and lines two runs up step by step (`align`)".

- [ ] **Step 4:** `cargo test -p insights align` - all 12 PASS. Then fmt, clippy, `cargo test --workspace`.
- [ ] **Step 5: Commit** "Line up two runs step by step in insights".

---

### Task 2: Claude Code align rules

**Files:** Modify `crates/adapter-claude-code/src/context.rs`, `crates/adapter-claude-code/src/lib.rs`.

**Interfaces:** Produces `pub fn align_rules() -> insights::AlignRules`, re-exported from the adapter's `lib.rs` next to `flag_rules`.

- [ ] **Step 1: Failing test** in `context.rs` tests:

```rust
    #[test]
    fn align_rules_name_each_tools_target() {
        let rules = align_rules();
        let fields = |tool: &str| {
            rules
                .targets
                .iter()
                .find(|r| r.tool == tool)
                .map(|r| r.fields.clone())
                .unwrap_or_default()
        };
        for tool in ["Read", "Edit", "MultiEdit", "Write"] {
            assert_eq!(fields(tool), vec!["file_path".to_string()], "{tool}");
        }
        assert_eq!(fields("NotebookEdit"), vec!["notebook_path".to_string()]);
        for tool in ["Bash", "PowerShell"] {
            assert_eq!(fields(tool), vec!["command".to_string()], "{tool}");
        }
        assert_eq!(fields("Grep"), vec!["pattern".to_string(), "path".into(), "glob".into()]);
        assert_eq!(fields("Glob"), vec!["pattern".to_string(), "path".into()]);
        assert_eq!(fields("WebFetch"), vec!["url".to_string()]);
        assert_eq!(fields("WebSearch"), vec!["query".to_string()]);
        for tool in ["Agent", "Task"] {
            assert_eq!(fields(tool), vec!["subagent_type".to_string(), "description".into()], "{tool}");
        }
        assert_eq!(rules.fallback_fields, vec!["command", "file_path", "pattern", "query", "url", "path"]);
        assert_eq!(rules.notification_prefixes, vec!["<task-notification>"]);
    }
```

- [ ] **Step 2: Implement** `align_rules()` after `flag_rules()` with a doc comment ("Which input fields name each Claude Code tool's target, for lining up two runs, and the prefix of background-agent notification prompts."), building the `targets` list exactly as the test says.
- [ ] **Step 3:** test, fmt, clippy; commit "Add the Claude Code rules for lining up runs".

---

### Task 3: Variant fixture session

**Files:**
- Create: `fixtures/claude-code/variant/00000000-0000-4000-8000-000000000004.jsonl` (and a `00000000-0000-4000-8000-000000000004/subagents/` folder if the copy needs it)
- Modify: tests and mock snapshots that list the fixture sessions (`src-tauri/src/sessions.rs` `FIXTURE_SESSIONS` and any test counting sessions), `ui/src/mock/*.json` (regenerated).

Requirements:
- Copy `fixtures/claude-code/basic/00000000-0000-4000-8000-000000000002.jsonl` (and its `00000000-0000-4000-8000-000000000002/subagents/` folder, renamed to the new id) and replace every occurrence of the session id `00000000-0000-4000-8000-000000000002` with `00000000-0000-4000-8000-000000000004`. Keep every other id (uuids, message ids, tool ids) unchanged unless a test needs them unique across sessions; if a test does (for example the mock's node details keyed by trace id across both sessions), change the variant's ids in a consistent, invented way and say so in the report.
- Change exactly two steps so the runs diverge: (1) in prompt 2 change one tool call's target (for example a different file path or pattern) and its result text to match; (2) in prompt 4 change one command's input. Both `tool_use` input and the matching `tool_result` content must stay consistent. Keep all text invented, in the style of the existing fixture (no real paths or names).
- Set the variant's first prompt or title so it is distinguishable in the session list (if the title comes from a summary/title line, change its text to "Example text 1690, variant." or similar).
- Add the variant to `FIXTURE_SESSIONS` (as `("variant", "00000000-0000-4000-8000-000000000004")`) and fix every test that counts or lists fixture sessions. Regenerate the mock snapshots (`$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS`) and review the diff: only additions for the new session (and compare-data.json if its test iterates `FIXTURE_SESSIONS`; if `ui_compare_data_matches_the_backend` destructures `[first, second]`, keep it comparing basic and compaction for now; Task 4 adds basic vs variant).
- `cargo run -p adapter-claude-code --example print_tree -- fixtures/claude-code/variant/00000000-0000-4000-8000-000000000004.jsonl` prints a tree with the same shape as the basic one apart from the two changed steps.
- Check: no personal data, no em dashes, all fmt/clippy/tests pass, UI checks pass (mock data changed).
- Commit "Add an invented variant of the basic sample session for lining up runs".

---

### Task 4: `alignment` in `compare_runs`

**Files:** Modify `src-tauri/src/compare.rs`, `src-tauri/src/sessions.rs` (snapshot test), `ui/src/mock/compare-data.json` (regenerated).

**Interfaces:** Consumes `insights::{run_steps, align_steps, RunSteps, Alignment}`, `adapter_claude_code::align_rules`. Produces `CompareView.alignment: insights::Alignment`.

- [ ] **Step 1:** In `SideSummary` add `pub steps: insights::RunSteps` (doc: "The side's steps, for lining up."). In `side_summary`, after the scope is resolved: `let steps = insights::run_steps(trace, &scope, &align_rules());`. In `CompareView` add `pub alignment: insights::Alignment` (doc: "The two runs lined up step by step.") and fill it in `compare_view` with `insights::align_steps(&a.steps, &b.steps)` before the summaries are moved.
- [ ] **Step 2: Tests** in `compare.rs`:
  - basic vs basic: `alignment.first_difference` is `None` and every row is `Both`.
  - basic vs variant: `first_difference` is `Some` with `pair == 1` (prompt 2, from 0), and the alignment has exactly two `OnlyA` and two `OnlyB` rows (one of each per changed step), unless the variant's changes in Task 3 were made differently; derive the expectation from the Task 3 report and state it in the test name.
  - a prompt scope on each side of basic (prompt 1 vs prompt 1) gives one pair.
- [ ] **Step 3: Snapshot:** extend `ui_compare_data_matches_the_backend` to also store basic vs variant in both orders (keys `basic/...0002|variant/...0004` and the reverse). The basic side keeps its fixture captures. Regenerate and review: only compare-data.json changes.
- [ ] **Step 4:** fmt, clippy, `cargo test --workspace`; commit "Return the step alignment from compare_runs".

---

### Task 5: Steps section in the Compare view

**Files:** Modify `ui/src/types.ts`, `ui/src/compare.ts`, `ui/src/compare.test.ts`, `ui/src/components/CompareView.tsx`, `ui/src/App.tsx`, CSS (`ui/src/app.css`; tokens in `ui/src/index.css`). Create `ui/src/components/StepsSection.tsx`.

**Interfaces:** Consumes `CompareView.alignment` (types mirror Task 1 field for field, snake_case: `Alignment`, `PromptPair`, `AlignRow`, `AlignStep` (the Rust `Step`), `PromptSide`, `DiffPoint`, `RowKind = 'both' | 'only_a' | 'only_b'`).

Behaviour:
1. `StepsSection` renders after Problems and before Outcome, titled "Steps".
2. Header: when `first_difference` is set, a link-button "First difference: prompt N, step M" (N = the A side's prompt index + 1, or the B side's if A is missing; M = row + 1) that scrolls the target row (or the pair heading for a pair with no rows) into view inside `.compare-body` and briefly highlights it; otherwise "The runs took the same steps." With no pairs at all: "No steps to line up."
3. Per pair: an `h4` with "A: Prompt N: preview" and "B: Prompt N: preview", or "Only in A" / "Only in B" for the missing side. A note "Too long to line up exactly; matched in order." when `approximate`.
4. A two-column table (A, B). Cell: the label as a link-button (title "Open this step in run A's diagram" / "B"), a red error mark if `failed`, "running" if `running`, and the duration (`formatDuration`). Empty slot on one-sided rows tinted, with the tag "only in A" / "only in B" in the row. `differs` rows tag "outcome differs". The pair's `first_difference` row gets the class `steps-first-difference` (left border marker).
5. Long pairs: first 50 rows, then "Show all N steps".
6. Clicking a step calls `onJump(side, traceId)`. App: if the step's session is the open session, close the comparison and run the existing `selectTrace(traceId)`. Otherwise open that session (`openSession`) and keep a pending jump `{ project, sessionId, traceId }`; when that session's view becomes ready (effect on `view`), run `selectTrace` once and clear the pending jump. Opening any other session or closing clears the pending jump. Which session a side is depends on `swapped`; put that mapping in a pure helper.
7. Below 900px each row stacks A above B.

Pure helpers in `compare.ts` with node:test tests: `visibleRows(rows, showAll, limit = 50)`, `stepTags(row)` (returns the tags shown for a row), `diffLabel(alignment)` (the header text), `jumpSession(side, swapped, left, right)` (which `SessionSummary` a side's step belongs to).

- [ ] Steps: helpers with tests first (RED, GREEN), then the component, App wiring, CSS.
- [ ] Checks: lint, typecheck, test, build.
- [ ] Browser check with the mock (`cd ui; npm run dev`, `http://localhost:5173/?mock`): open the basic sample, Compare with the variant sample, check the Steps section, the first-difference link, a jump on side A and on side B, "Show all". Stop the dev server (and its node process) afterwards.
- [ ] Commit "Show the two runs lined up step by step in the Compare view".

---

### Task 6: Docs

- `docs/VIEW-MODEL.md`: the `alignment` field and its shapes, `first_difference` rules (one-sided pair is `Some(0)`), `approximate`, the step key (tool + target, whitespace collapsed, parallel calls sorted).
- `docs/DECISIONS.md`: M5.3b entry: LCS diff, `MAX_CELLS` and the in-order fallback, parallel calls sorted by key, subagent as one step, target fields as adapter data (`AlignRules`), notifications pair by order.
- `CLAUDE.md`: 5.3b line marked complete (one sentence naming `insights::align`, `align_rules`, the Steps section, and the spec path); 5.4 stays planned. Mention the variant sample in the mock line.
- `README.md`: milestone status.
- Check no em dashes with `git grep -nP "\x{2014}" -- docs README.md CLAUDE.md`. Commit "Document lining up two runs".
