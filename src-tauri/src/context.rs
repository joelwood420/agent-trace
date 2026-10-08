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
    /// the parts were first set. Lengths only, never the text.
    pub hidden_context: Vec<HiddenPart>,
}

/// One part of the main agent's hidden context, without its text.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HiddenPart {
    /// The `context_update` node that last set this part.
    pub trace_id: String,
    /// Stable identifier of the part within the run.
    pub key: String,
    /// What sort of part this is.
    pub kind: ContextPartKind,
    /// Short name for people.
    pub label: String,
    /// Length of the text in characters.
    pub chars: u64,
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
/// model calls). Nested runs are not entered.
fn hidden_context(trace: &Trace, run_id: Option<&str>) -> Vec<HiddenPart> {
    let mut parts = Vec::new();
    if let Some(run_id) = run_id {
        collect_hidden(trace, run_id, &mut parts);
    }
    parts
}

fn collect_hidden(trace: &Trace, id: &str, parts: &mut Vec<HiddenPart>) {
    for child in trace.children(id) {
        if matches!(child.node, Node::Run(_)) {
            continue;
        }
        if let Node::ContextUpdate(update) = &child.node {
            apply_hidden(parts, &child.id, update);
        }
        collect_hidden(trace, &child.id, parts);
    }
}

fn apply_hidden(parts: &mut Vec<HiddenPart>, trace_id: &str, update: &ContextUpdate) {
    for part in &update.parts {
        let value = HiddenPart {
            trace_id: trace_id.to_string(),
            key: part.key.clone(),
            kind: part.kind.clone(),
            label: part.label.clone(),
            chars: part.text.chars().count() as u64,
        };
        match parts.iter_mut().find(|p| p.key == part.key) {
            Some(existing) => *existing = value,
            None => parts.push(value),
        }
    }
    parts.retain(|p| !update.remove.contains(&p.key));
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

    #[test]
    fn hidden_context_lists_the_main_runs_parts_without_text() {
        let trace = fixture_trace();
        let view = session_context(&ContextFacts::of(&trace), &SessionRecords::default());
        assert!(!view.hidden_context.is_empty());
        let kinds: Vec<&ContextPartKind> = view.hidden_context.iter().map(|p| &p.kind).collect();
        assert!(kinds.contains(&&ContextPartKind::Reminder) || kinds.len() > 1);
        for part in &view.hidden_context {
            assert!(part.trace_id.starts_with("context:"), "{}", part.trace_id);
            assert!(part.chars > 0, "{}", part.key);
            let event = trace.get(&part.trace_id).expect("a node");
            assert!(matches!(event.node, Node::ContextUpdate(_)));
        }
        let json = serde_json::to_string(&view.hidden_context).expect("json");
        assert!(!json.contains("\"text\""));
    }

    #[test]
    fn hidden_context_excludes_subagents_and_follows_removals() {
        use trace_core::{ContextPart, Run};
        fn add(trace: &mut Trace, id: &str, parent: Option<&str>, node: Node) {
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
        fn part(key: &str, text: &str) -> ContextPart {
            ContextPart {
                key: key.into(),
                kind: ContextPartKind::Instructions,
                label: key.into(),
                text: text.into(),
            }
        }
        let mut trace = Trace::new();
        add(
            &mut trace,
            "run:a",
            None,
            Node::Run(Run {
                harness: "test".into(),
                title: None,
            }),
        );
        let update = |parts, remove| Node::ContextUpdate(ContextUpdate { parts, remove });
        add(
            &mut trace,
            "context:1",
            Some("run:a"),
            update(vec![part("a", "xx"), part("b", "y")], vec![]),
        );
        add(
            &mut trace,
            "run:sub",
            Some("run:a"),
            Node::Run(Run {
                harness: "test".into(),
                title: None,
            }),
        );
        add(
            &mut trace,
            "context:sub",
            Some("run:sub"),
            update(vec![part("s", "zzz")], vec![]),
        );
        add(
            &mut trace,
            "context:2",
            Some("run:a"),
            update(vec![part("a", "xxxx")], vec![]),
        );
        add(
            &mut trace,
            "context:3",
            Some("run:a"),
            update(vec![], vec!["b".into()]),
        );
        let hidden = ContextFacts::of(&trace);
        let view = session_context(&hidden, &SessionRecords::default());
        assert_eq!(view.hidden_context.len(), 1);
        let only = &view.hidden_context[0];
        assert_eq!((only.key.as_str(), only.chars), ("a", 4));
        assert_eq!(only.trace_id, "context:2");
    }
}
