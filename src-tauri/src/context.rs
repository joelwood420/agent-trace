//! The context views: what fills each model call's context, as bars for the
//! diagram and in full for the details panel and the session overview. A
//! call with a captured request uses the capture's measure; otherwise the
//! measure comes from the transcript. Only measures are cached; scaling to
//! the reported total happens here each time, so a bar updates when the
//! call's usage arrives later.

use std::collections::{BTreeMap, HashMap};

use adapter_claude_code::context_rules;
use insights::{ContextBar, ContextBreakdown, ContextMeasure, breakdown};
use serde::Serialize;
use trace_core::{ContextPartKind, ContextUpdate, Node, Trace};
use trace_view::model_calls_by_run;

use crate::captures::{SessionRecords, mapped_id};

/// What the context views show for a session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionContext {
    /// Bar per model call trace id, for every model call with a known total
    /// or a capture.
    pub bars: BTreeMap<String, ContextBar>,
    /// The latest model call of the main run, in full.
    pub latest: Option<ContextBreakdown>,
    /// Its trace id.
    pub latest_trace_id: Option<String>,
    /// The hidden context in force at the end of the main run, in the order
    /// the parts were first set, with tool definitions collapsed into one row
    /// per group. Lengths only, never the text.
    pub hidden_context: Vec<HiddenPart>,
}

/// One row of the main agent's hidden context, without its text: a single
/// part, or every tool definition of one group (the built-ins or one MCP
/// server).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HiddenPart {
    /// The `context_update` node that last set this part, or for a group the
    /// one that most recently set any of its tools.
    pub trace_id: String,
    /// Stable identifier of the row: the part key, or `tools:<group>`.
    pub key: String,
    /// What sort of part this is.
    pub kind: ContextPartKind,
    /// Short name for people: the part label, or the group name.
    pub label: String,
    /// Length of the text in characters, summed over a group.
    pub chars: u64,
    /// Number of parts in the row: 1, or the number of tools in a group.
    pub count: u32,
    /// The keys of the parts in the row, in first-set order.
    pub part_keys: Vec<String>,
}

/// What the context views need from a trace, copied under the session lock.
#[derive(Debug, Clone, Default)]
pub struct ContextFacts {
    /// Transcript measures by model call id.
    transcript: HashMap<String, ContextMeasure>,
    /// The reported input context of each model call covered, by id. Every
    /// model call these facts cover has an entry, `None` when not reported.
    totals: HashMap<String, Option<u64>>,
    /// The last model call of the main run, if there is one.
    latest: Option<String>,
    /// The main run's hidden context at its end.
    hidden: Vec<HiddenPart>,
}

impl ContextFacts {
    /// Facts for every model call of `trace`, measured in one pass per run.
    pub fn of(trace: &Trace) -> Self {
        let runs = model_calls_by_run(trace);
        let hidden = hidden_context(trace, runs.first().map(|(id, _)| id.as_str()));
        let latest = runs.first().and_then(|(_, calls)| calls.last().cloned());
        let totals = runs
            .into_iter()
            .flat_map(|(_, calls)| calls)
            .map(|id| {
                let total = reported_total(trace, &id);
                (id, total)
            })
            .collect();
        Self {
            transcript: insights::measure_transcript_all(trace, &context_rules()),
            totals,
            latest,
            hidden,
        }
    }

    /// Facts for one model call only. Empty if `trace_id` is not a model
    /// call.
    pub fn of_call(trace: &Trace, trace_id: &str) -> Self {
        let mut facts = Self::default();
        if let Some(measure) = insights::measure_transcript(trace, trace_id, &context_rules()) {
            facts.transcript.insert(trace_id.to_string(), measure);
            facts
                .totals
                .insert(trace_id.to_string(), reported_total(trace, trace_id));
        }
        facts
    }
}

/// The input context the API reported for model call `id`, if any.
fn reported_total(trace: &Trace, id: &str) -> Option<u64> {
    match &trace.get(id)?.node {
        Node::ModelCall(call) => call.usage.as_ref().and_then(|u| u.context_tokens()),
        _ => None,
    }
}

/// Bars for every model call with data, and the main run's latest call in
/// full. `captures` must be summarised (`summarise_missing`) first.
pub fn session_context(facts: &ContextFacts, captures: &SessionRecords) -> SessionContext {
    let captured = captured_measures(facts, captures);
    let bars = facts
        .totals
        .keys()
        .filter_map(|id| {
            let view = breakdown_of(facts, &captured, id)?;
            Some((id.clone(), view.bar()))
        })
        .collect();
    let latest = facts
        .latest
        .as_deref()
        .and_then(|id| breakdown_of(facts, &captured, id));
    SessionContext {
        bars,
        latest,
        latest_trace_id: facts.latest.clone(),
        hidden_context: facts.hidden.clone(),
    }
}

/// The hidden context in force at the end of the main run, which is the
/// first run of `model_calls_by_run` (the first root run, even one without
/// model calls). Nested runs are not entered. Tool definitions are grouped.
fn hidden_context(trace: &Trace, run_id: Option<&str>) -> Vec<HiddenPart> {
    let mut parts = Vec::new();
    if let Some(run_id) = run_id {
        let mut seq = 0;
        collect_hidden(trace, run_id, &mut parts, &mut seq);
    }
    group_tools(parts)
}

/// A part in force, with the position of the update that last set it.
struct SetPart {
    row: HiddenPart,
    set_at: usize,
}

fn collect_hidden(trace: &Trace, id: &str, parts: &mut Vec<SetPart>, seq: &mut usize) {
    for child in trace.children(id) {
        if matches!(child.node, Node::Run(_)) {
            continue;
        }
        if let Node::ContextUpdate(update) = &child.node {
            *seq += 1;
            apply_hidden(parts, &child.id, update, *seq);
        }
        collect_hidden(trace, &child.id, parts, seq);
    }
}

fn apply_hidden(parts: &mut Vec<SetPart>, trace_id: &str, update: &ContextUpdate, set_at: usize) {
    for part in &update.parts {
        let row = HiddenPart {
            trace_id: trace_id.to_string(),
            key: part.key.clone(),
            kind: part.kind.clone(),
            label: part.label.clone(),
            chars: part.text.chars().count() as u64,
            count: 1,
            part_keys: vec![part.key.clone()],
        };
        let value = SetPart { row, set_at };
        match parts.iter_mut().find(|p| p.row.key == part.key) {
            Some(existing) => *existing = value,
            None => parts.push(value),
        }
    }
    parts.retain(|p| !update.remove.contains(&p.row.key));
}

/// Collapses tool definition parts into one row per group, grouped the way
/// the context breakdown groups them. A group sits where its first member
/// was; other parts stay one row each.
fn group_tools(parts: Vec<SetPart>) -> Vec<HiddenPart> {
    let mut rows: Vec<SetPart> = Vec::new();
    for part in parts {
        if part.row.kind != ContextPartKind::ToolDefinitions {
            rows.push(part);
            continue;
        }
        let group = insights::tool_group(&part.row.label);
        let key = format!("tools:{group}");
        match rows
            .iter_mut()
            .find(|r| r.row.kind == ContextPartKind::ToolDefinitions && r.row.key == key)
        {
            Some(existing) => {
                existing.row.chars += part.row.chars;
                existing.row.count += 1;
                existing.row.part_keys.push(part.row.key);
                if part.set_at > existing.set_at {
                    existing.set_at = part.set_at;
                    existing.row.trace_id = part.row.trace_id;
                }
            }
            None => rows.push(SetPart {
                row: HiddenPart {
                    key,
                    label: group,
                    ..part.row
                },
                set_at: part.set_at,
            }),
        }
    }
    rows.into_iter().map(|r| r.row).collect()
}

/// The full breakdown of one model call, or `None` if it is not a model
/// call covered by `facts` or has neither a reported total nor a capture.
pub fn call_context(
    facts: &ContextFacts,
    captures: &SessionRecords,
    trace_id: &str,
) -> Option<ContextBreakdown> {
    let captured = captured_measures(facts, captures);
    breakdown_of(facts, &captured, trace_id)
}

/// The capture measure of each model call that has a measured capture. If
/// a call has several captures, the first one stored is used. Records not
/// measured yet are skipped.
fn captured_measures<'a>(
    facts: &ContextFacts,
    captures: &'a SessionRecords,
) -> HashMap<String, &'a ContextMeasure> {
    let mut out = HashMap::new();
    for record in captures.records.iter() {
        let Some(id) = mapped_id(record, |id| facts.totals.contains_key(id)) else {
            continue;
        };
        if let Some(Some(measure)) = captures.measures.get(&record.id) {
            out.entry(id).or_insert(measure);
        }
    }
    out
}

/// The breakdown of model call `id`: the capture's measure when it has one,
/// otherwise the transcript's, scaled to the reported total. `None` for an
/// unknown call or one with neither a capture nor a reported total.
fn breakdown_of(
    facts: &ContextFacts,
    captured: &HashMap<String, &ContextMeasure>,
    id: &str,
) -> Option<ContextBreakdown> {
    let total = *facts.totals.get(id)?;
    if let Some(measure) = captured.get(id) {
        return Some(breakdown(measure, total));
    }
    let total = total?;
    let measure = facts.transcript.get(id)?;
    Some(breakdown(measure, Some(total)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::captures::{fixture_records, measure_calls};
    use insights::{ContextSource, SliceKind};
    use std::path::Path;
    use trace_core::TraceEvent;

    const SESSION: &str = "00000000-0000-4000-8000-000000000002";

    fn fixture_events() -> Vec<TraceEvent> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
            .join("basic")
            .join(format!("{SESSION}.jsonl"));
        adapter_claude_code::load_session(&path)
            .expect("load fixture")
            .events
    }

    fn trace_of(events: Vec<TraceEvent>) -> Trace {
        let mut trace = Trace::new();
        for event in events {
            trace.apply(event).expect("fixture events form a tree");
        }
        trace
    }

    fn fixture_trace() -> Trace {
        trace_of(fixture_events())
    }

    /// The fixture captures, summarised and measured as the app does.
    fn fixture_captures() -> SessionRecords {
        let mut captures = SessionRecords::new(fixture_records(), Vec::new());
        captures.summarise_missing();
        captures
    }

    fn main_calls(trace: &Trace) -> Vec<String> {
        model_calls_by_run(trace)
            .into_iter()
            .next()
            .expect("main run")
            .1
    }

    fn reported_total(trace: &Trace, id: &str) -> Option<u64> {
        match &trace.get(id).expect("call").node {
            Node::ModelCall(call) => call.usage.as_ref().and_then(|u| u.context_tokens()),
            _ => None,
        }
    }

    #[test]
    fn captured_calls_use_the_capture() {
        let trace = fixture_trace();
        let view = session_context(&ContextFacts::of(&trace), &fixture_captures());
        let first = &main_calls(&trace)[0];
        let bar = view.bars.get(first).expect("a bar");
        assert_eq!(bar.source, ContextSource::Captured);
        assert!(
            bar.slices
                .iter()
                .any(|s| s.kind == SliceKind::ToolDefinitions)
        );
    }

    #[test]
    fn uncaptured_calls_use_the_transcript() {
        let trace = fixture_trace();
        let view = session_context(&ContextFacts::of(&trace), &SessionRecords::default());
        let with_usage: Vec<String> = model_calls_by_run(&trace)
            .into_iter()
            .flat_map(|(_, calls)| calls)
            .filter(|id| reported_total(&trace, id).is_some())
            .collect();
        assert!(!with_usage.is_empty());
        for id in &with_usage {
            let bar = view.bars.get(id).expect("a bar");
            assert_eq!(bar.source, ContextSource::Transcript, "{id}");
            assert!(bar.total_is_reported);
        }
        assert_eq!(view.bars.len(), with_usage.len(), "no bar without data");
    }

    #[test]
    fn slices_add_up_to_the_reported_total() {
        let trace = fixture_trace();
        let facts = ContextFacts::of(&trace);
        for captures in [fixture_captures(), SessionRecords::default()] {
            let view = session_context(&facts, &captures);
            assert!(!view.bars.is_empty());
            for (id, bar) in view.bars.iter().filter(|(_, b)| b.total_is_reported) {
                let sum: u64 = bar.slices.iter().map(|s| s.tokens).sum();
                assert_eq!(sum, bar.total_tokens, "{id}");
                assert_eq!(Some(bar.total_tokens), reported_total(&trace, id), "{id}");
            }
        }
    }

    #[test]
    fn latest_is_the_main_runs_last_call() {
        let trace = fixture_trace();
        let captures = fixture_captures();
        let facts = ContextFacts::of(&trace);
        let view = session_context(&facts, &captures);
        let last = main_calls(&trace).last().cloned();
        assert!(last.is_some());
        assert_eq!(view.latest_trace_id, last);
        let id = last.expect("last");
        assert_eq!(view.latest, call_context(&facts, &captures, &id));
        assert!(view.latest.is_some());

        let empty = session_context(&ContextFacts::of(&Trace::new()), &captures);
        assert!(empty.bars.is_empty());
        assert_eq!(empty.latest, None);
        assert_eq!(empty.latest_trace_id, None);
    }

    #[test]
    fn usage_arriving_later_updates_the_bar() {
        let events = fixture_events();
        let first = main_calls(&trace_of(events.clone()))[0].clone();
        let without_usage: Vec<TraceEvent> = events
            .iter()
            .cloned()
            .map(|mut event| {
                if event.id == first {
                    if let Node::ModelCall(call) = &mut event.node {
                        call.usage = None;
                    }
                }
                event
            })
            .collect();
        let captures = fixture_captures();
        let measured = measure_calls();

        let before = trace_of(without_usage);
        let early = session_context(&ContextFacts::of(&before), &captures);
        let bar = early
            .bars
            .get(&first)
            .expect("a captured bar without usage");
        assert!(!bar.total_is_reported);

        let after = trace_of(events);
        let late = session_context(&ContextFacts::of(&after), &captures);
        let bar = late.bars.get(&first).expect("a bar");
        assert!(bar.total_is_reported);
        assert_eq!(Some(bar.total_tokens), reported_total(&after, &first));

        let one = call_context(&ContextFacts::of_call(&after, &first), &captures, &first)
            .expect("a breakdown");
        assert!(one.total_is_reported);
        assert_eq!(measure_calls(), measured, "no capture was measured again");
    }

    #[test]
    fn unknown_call_is_none() {
        let trace = fixture_trace();
        let captures = fixture_captures();
        let facts = ContextFacts::of(&trace);
        assert!(call_context(&facts, &captures, "turn:x").is_none());
        assert!(
            call_context(
                &ContextFacts::of_call(&trace, "turn:x"),
                &captures,
                "turn:x"
            )
            .is_none()
        );
    }

    #[test]
    fn one_call_facts_match_the_session_facts() {
        let trace = fixture_trace();
        let captures = fixture_captures();
        let all = ContextFacts::of(&trace);
        for (_, calls) in model_calls_by_run(&trace) {
            for id in calls {
                assert_eq!(
                    call_context(&ContextFacts::of_call(&trace, &id), &captures, &id),
                    call_context(&all, &captures, &id),
                    "{id}"
                );
            }
        }
    }

    /// The id of the nearest run enclosing node `id`.
    fn enclosing_run(trace: &Trace, id: &str) -> Option<String> {
        let mut current = trace.get(id)?.parent_id.clone();
        while let Some(parent) = current {
            let event = trace.get(&parent)?;
            if matches!(event.node, Node::Run(_)) {
                return Some(parent);
            }
            current = event.parent_id.clone();
        }
        None
    }

    #[test]
    fn hidden_context_lists_the_main_runs_parts_without_text() {
        let trace = fixture_trace();
        let view = session_context(&ContextFacts::of(&trace), &SessionRecords::default());
        let count = |kind: ContextPartKind| {
            view.hidden_context
                .iter()
                .filter(|p| p.kind == kind)
                .count()
        };
        assert_eq!(count(ContextPartKind::Reminder), 11);
        assert_eq!(count(ContextPartKind::Instructions), 3);
        assert_eq!(count(ContextPartKind::SystemPrompt), 1);
        assert_eq!(count(ContextPartKind::ToolDefinitions), 1);
        assert_eq!(view.hidden_context.len(), 16);
        let tools = view
            .hidden_context
            .iter()
            .find(|p| p.kind == ContextPartKind::ToolDefinitions)
            .expect("a tool row");
        assert_eq!((tools.key.as_str(), tools.count), ("tools:built-in", 3));
        assert_eq!(tools.part_keys.len(), 3);

        let main_run = model_calls_by_run(&trace)
            .into_iter()
            .next()
            .expect("main run")
            .0;
        for part in &view.hidden_context {
            assert!(part.trace_id.starts_with("context:"), "{}", part.trace_id);
            assert!(part.chars > 0, "{}", part.key);
            let event = trace.get(&part.trace_id).expect("a node");
            assert!(matches!(event.node, Node::ContextUpdate(_)));
            assert_eq!(
                enclosing_run(&trace, &part.trace_id).as_deref(),
                Some(main_run.as_str()),
                "{} is not from a subagent",
                part.trace_id
            );
        }
        let json = serde_json::to_string(&view.hidden_context).expect("json");
        assert!(!json.contains("\"text\""));
        assert!(json.contains("\"part_keys\""));
        assert!(json.contains("\"count\""));
    }

    fn add_node(trace: &mut Trace, id: &str, parent: Option<&str>, node: Node) {
        trace
            .apply(TraceEvent {
                id: id.into(),
                parent_id: parent.map(String::from),
                node,
                started_at_ms: None,
                ended_at_ms: None,
                raw: Vec::new(),
                metadata: Default::default(),
            })
            .expect("apply");
    }

    fn test_run() -> Node {
        Node::Run(trace_core::Run {
            harness: "test".into(),
            title: None,
        })
    }

    fn test_part(
        key: &str,
        kind: ContextPartKind,
        label: &str,
        text: &str,
    ) -> trace_core::ContextPart {
        trace_core::ContextPart {
            key: key.into(),
            kind,
            label: label.into(),
            text: text.into(),
        }
    }

    fn test_update(parts: Vec<trace_core::ContextPart>, remove: Vec<String>) -> Node {
        Node::ContextUpdate(ContextUpdate { parts, remove })
    }

    /// A hidden row as (key, label, chars, count, part keys, trace id).
    type Row<'a> = (&'a str, &'a str, u64, u32, Vec<&'a str>, &'a str);

    #[test]
    fn hidden_tool_parts_collapse_into_one_row_per_group() {
        let tool = ContextPartKind::ToolDefinitions;
        let mut trace = Trace::new();
        add_node(&mut trace, "run:a", None, test_run());
        add_node(
            &mut trace,
            "context:1",
            Some("run:a"),
            test_update(
                vec![
                    test_part("tool:mcp__docs__a", tool.clone(), "mcp__docs__a", "aaa"),
                    test_part("reminder:1", ContextPartKind::Reminder, "date", "dd"),
                    test_part("tool:Bash", tool.clone(), "Bash", "b"),
                    test_part("tool:mcp__docs__b", tool.clone(), "mcp__docs__b", "bbbbb"),
                ],
                vec![],
            ),
        );
        add_node(
            &mut trace,
            "context:2",
            Some("run:a"),
            test_update(
                vec![test_part(
                    "tool:mcp__docs__a",
                    tool.clone(),
                    "mcp__docs__a",
                    "aaaa",
                )],
                vec![],
            ),
        );
        let view = session_context(&ContextFacts::of(&trace), &SessionRecords::default());
        let rows: Vec<Row> = view
            .hidden_context
            .iter()
            .map(|r| {
                (
                    r.key.as_str(),
                    r.label.as_str(),
                    r.chars,
                    r.count,
                    r.part_keys.iter().map(String::as_str).collect(),
                    r.trace_id.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "tools:MCP: docs",
                    "MCP: docs",
                    9,
                    2,
                    vec!["tool:mcp__docs__a", "tool:mcp__docs__b"],
                    "context:2",
                ),
                ("reminder:1", "date", 2, 1, vec!["reminder:1"], "context:1"),
                (
                    "tools:built-in",
                    "built-in",
                    1,
                    1,
                    vec!["tool:Bash"],
                    "context:1"
                ),
            ]
        );
        assert!(
            view.hidden_context[0].kind == tool && view.hidden_context[2].kind == tool,
            "group rows keep the tool kind"
        );
        assert_eq!(insights::tool_group("mcp__docs__a"), "MCP: docs");
        assert_eq!(insights::tool_group("Bash"), "built-in");
    }

    #[test]
    fn hidden_context_excludes_subagents_and_follows_removals() {
        fn part(key: &str, text: &str) -> trace_core::ContextPart {
            test_part(key, ContextPartKind::Instructions, key, text)
        }
        let mut trace = Trace::new();
        add_node(&mut trace, "run:a", None, test_run());
        add_node(
            &mut trace,
            "context:1",
            Some("run:a"),
            test_update(vec![part("a", "xx"), part("b", "y")], vec![]),
        );
        add_node(&mut trace, "run:sub", Some("run:a"), test_run());
        add_node(
            &mut trace,
            "context:sub",
            Some("run:sub"),
            test_update(vec![part("s", "zzz")], vec![]),
        );
        add_node(
            &mut trace,
            "context:2",
            Some("run:a"),
            test_update(vec![part("a", "xxxx")], vec![]),
        );
        add_node(
            &mut trace,
            "context:3",
            Some("run:a"),
            test_update(vec![], vec!["b".into()]),
        );
        let hidden = ContextFacts::of(&trace);
        let view = session_context(&hidden, &SessionRecords::default());
        assert_eq!(view.hidden_context.len(), 1);
        let only = &view.hidden_context[0];
        assert_eq!((only.key.as_str(), only.chars), ("a", 4));
        assert_eq!(only.trace_id, "context:2");
        assert_eq!(
            (only.count, only.part_keys.clone()),
            (1, vec!["a".to_string()])
        );
    }
}
