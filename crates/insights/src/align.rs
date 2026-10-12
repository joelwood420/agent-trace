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
    let turns = trace
        .children(main)
        .filter(|e| matches!(e.node, Node::Turn(_)));
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
            key: if is_notification {
                "\u{0}notification".to_string()
            } else {
                text
            },
            steps: turn_steps(trace, &turn.id, rules),
        });
    }
    RunSteps {
        prompts,
        scoped: matches!(scope, Scope::Prompt(_)),
    }
}

/// One item of a diff: indexes into A and B.
type Pairing = (Option<usize>, Option<usize>);

/// Lines up two key lists. Each item is (index in A, index in B), one of
/// them `None` for a one-sided item. The flag is true when the table would
/// exceed `limit` cells and in-order matching was used instead.
fn diff(a: &[&str], b: &[&str], limit: usize) -> (Vec<Pairing>, bool) {
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
            AlignRow {
                kind,
                a,
                b,
                differs,
            }
        })
        .collect();
    let first_difference = if a.is_none() || b.is_none() {
        Some(0)
    } else {
        rows.iter()
            .position(|r| r.kind != RowKind::Both || r.differs)
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
    Alignment {
        pairs,
        first_difference,
    }
}

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
                TargetRule {
                    tool: "Open".into(),
                    fields: vec!["file".into()],
                },
                TargetRule {
                    tool: "Find".into(),
                    fields: vec!["pattern".into(), "dir".into()],
                },
            ],
            fallback_fields: vec!["cmd".into()],
            notification_prefixes: vec!["<note>".into()],
        }
    }

    /// One tool call of a test trace: (tool, input, failed).
    type Tool = (&'static str, Value, Option<bool>);

    /// One run: prompts is a list of (prompt text, model calls), each model
    /// call a list of (tool, input, failed). Ids: T{p}, M{p}_{m}, X{p}_{m}_{t}.
    fn trace(prefix: &str, prompts: &[(&str, Vec<Vec<Tool>>)]) -> Trace {
        let mut t = Trace::new();
        let root = format!("{prefix}R");
        add(
            &mut t,
            &root,
            None,
            Node::Run(Run {
                harness: "test".into(),
                title: None,
            }),
        );
        for (p, (text, calls)) in prompts.iter().enumerate() {
            let turn = format!("{prefix}T{p}");
            add(
                &mut t,
                &turn,
                Some(&root),
                Node::Turn(Turn {
                    prompt: vec![ContentBlock::Text {
                        text: (*text).into(),
                    }],
                }),
            );
            for (m, tools) in calls.iter().enumerate() {
                let call = format!("{prefix}M{p}_{m}");
                add(
                    &mut t,
                    &call,
                    Some(&turn),
                    Node::ModelCall(ModelCall {
                        model: None,
                        output: Vec::new(),
                        stop_reason: None,
                        usage: None,
                    }),
                );
                for (x, (name, input, failed)) in tools.iter().enumerate() {
                    let id = format!("{prefix}X{p}_{m}_{x}");
                    let result = failed.map(|is_error| ToolResult {
                        content: Vec::new(),
                        is_error,
                    });
                    add(
                        &mut t,
                        &id,
                        Some(&call),
                        Node::ToolCall(ToolCall {
                            name: (*name).into(),
                            input: input.clone(),
                            result,
                        }),
                    );
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
        align_steps(
            &run_steps(a, &Scope::Session, &rules()),
            &run_steps(b, &Scope::Session, &rules()),
        )
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
        let a = trace(
            "a",
            &[(
                "fix it",
                vec![
                    vec![open("a.rs")],
                    vec![run("grep", false)],
                    vec![run("test", false)],
                ],
            )],
        );
        let b = trace(
            "b",
            &[(
                "fix it",
                vec![
                    vec![open("a.rs")],
                    vec![run("test", false)],
                    vec![open("b.rs")],
                ],
            )],
        );
        let al = lineup(&a, &b);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, OnlyA, Both, OnlyB]);
        assert_eq!(al.pairs[0].first_difference, Some(1));
        assert_eq!(al.first_difference, Some(DiffPoint { pair: 0, row: 1 }));
        assert_eq!(
            al.pairs[0].rows[1].a.as_ref().map(|s| s.label.as_str()),
            Some("Shell(grep)")
        );
        assert!(al.pairs[0].rows[1].b.is_none());
    }

    #[test]
    fn swapped_steps_show_as_one_sided() {
        let a = trace("a", &[("go", vec![vec![open("x")], vec![open("y")]])]);
        let b = trace("b", &[("go", vec![vec![open("y")], vec![open("x")]])]);
        let al = lineup(&a, &b);
        assert_eq!(al.pairs[0].rows.len(), 3);
        assert_eq!(
            al.pairs[0].rows.iter().filter(|r| r.kind == Both).count(),
            1
        );
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
        let a = trace(
            "a",
            &[(
                "go",
                vec![vec![
                    ("Open", json!({"file": "a.rs", "offset": 10}), Some(false)),
                    run("cargo   test", false),
                ]],
            )],
        );
        let b = trace(
            "b",
            &[(
                "go",
                vec![vec![
                    ("Open", json!({"file": "a.rs"}), Some(false)),
                    run(" cargo test ", false),
                ]],
            )],
        );
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
        let a = trace(
            "a",
            &[
                ("one", vec![vec![open("a")]]),
                ("extra", vec![]),
                ("two", vec![vec![open("b")]]),
            ],
        );
        let b = trace(
            "b",
            &[
                ("one", vec![vec![open("a")]]),
                ("two", vec![vec![open("b")]]),
            ],
        );
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
        let a = trace(
            "a",
            &[("go", vec![]), ("<note> id 1", vec![vec![open("a")]])],
        );
        let b = trace(
            "b",
            &[("go", vec![]), ("<note> id 2", vec![vec![open("a")]])],
        );
        let al = lineup(&a, &b);
        assert_eq!(al.pairs.len(), 2);
        assert!(al.pairs.iter().all(|p| p.a.is_some() && p.b.is_some()));
        assert_eq!(al.first_difference, None);
    }

    #[test]
    fn a_prompt_reading_notification_is_not_a_notification() {
        let a = trace("a", &[("notification", vec![vec![open("a")]])]);
        let b = trace("b", &[("<note> id 2", vec![vec![open("a")]])]);
        let al = lineup(&a, &b);
        assert!(al.pairs.iter().any(|p| p.a.is_some() && p.b.is_none()));
        assert!(al.pairs.iter().any(|p| p.a.is_none() && p.b.is_some()));
    }

    #[test]
    fn fallback_fields_and_tool_name_only() {
        let s = run_steps(
            &trace(
                "a",
                &[(
                    "go",
                    vec![vec![run("ls", false), ("Think", json!({}), None)]],
                )],
            ),
            &Scope::Session,
            &rules(),
        );
        let labels: Vec<&str> = s.prompts[0]
            .steps
            .iter()
            .map(|(_, st)| st.label.as_str())
            .collect();
        assert_eq!(labels, vec!["Shell(ls)", "Think"]);
        assert!(s.prompts[0].steps[1].1.running);
    }

    #[test]
    fn a_subagent_is_one_step() {
        let mut t = trace(
            "a",
            &[(
                "go",
                vec![vec![(
                    "Find",
                    json!({"pattern": "x", "dir": "src"}),
                    Some(false),
                )]],
            )],
        );
        add(
            &mut t,
            "S",
            Some("aX0_0_0"),
            Node::Run(Run {
                harness: "test".into(),
                title: None,
            }),
        );
        add(
            &mut t,
            "ST",
            Some("S"),
            Node::Turn(Turn { prompt: Vec::new() }),
        );
        add(
            &mut t,
            "SM",
            Some("ST"),
            Node::ModelCall(ModelCall {
                model: None,
                output: Vec::new(),
                stop_reason: None,
                usage: None,
            }),
        );
        add(
            &mut t,
            "SX",
            Some("SM"),
            Node::ToolCall(ToolCall {
                name: "Open".into(),
                input: json!({"file": "q"}),
                result: None,
            }),
        );
        let s = run_steps(&t, &Scope::Session, &rules());
        assert_eq!(s.prompts[0].steps.len(), 1);
        assert_eq!(s.prompts[0].steps[0].1.label, "Find(x / src)");
    }

    #[test]
    fn prompt_scopes_pair_whatever_the_text() {
        let a = trace(
            "a",
            &[
                ("one", vec![vec![open("a")]]),
                ("two", vec![vec![open("b")]]),
            ],
        );
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
        let a = trace(
            "a",
            &[(
                "go",
                vec![vec![open("a")], vec![open("b")], vec![open("c")]],
            )],
        );
        let b = trace(
            "b",
            &[(
                "go",
                vec![vec![open("a")], vec![open("x")], vec![open("c")]],
            )],
        );
        let al = align_with_limit(
            &run_steps(&a, &Scope::Session, &rules()),
            &run_steps(&b, &Scope::Session, &rules()),
            4,
        );
        assert!(al.pairs[0].approximate);
        assert_eq!(kinds(&al.pairs[0]), vec![Both, OnlyA, OnlyB, Both]);
        let exact = lineup(&a, &b);
        assert!(!exact.pairs[0].approximate);
    }
}
