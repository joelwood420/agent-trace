//! Views of a session's captured API calls: which model calls have one, an
//! overview of the whole session, and the detail of one call with the
//! changes since the call before it. Everything here works on records that
//! are already loaded; reading the store is the caller's job.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use adapter_claude_code::{context_rules, model_call_id};
use capture_core::{CaptureRecord, RequestDiff, RequestSummary, summarise};
use insights::{ContextMeasure, ContextRules, measure_request};
use serde::Serialize;
use trace_core::{Node, Trace};
use trace_view::model_calls_by_run;

/// One capture in the index of the open session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureIndexEntry {
    /// The capture record id.
    pub capture_id: String,
    /// The model call trace id this capture belongs to, if that call is in
    /// the trace.
    pub trace_id: Option<String>,
    /// When the request was received, milliseconds since 1970-01-01 UTC.
    pub started_at_ms: i64,
}

/// A small index of a session's captures, in time order, kept in memory by
/// the open session. Building it only maps message ids to model calls, so
/// it stays cheap however large the requests are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureIndex {
    entries: Vec<CaptureIndexEntry>,
}

impl CaptureIndex {
    /// Indexes `records` against `trace`. A capture gets a trace id when its
    /// response message id names a model call that exists in the trace.
    pub fn build(trace: &Trace, records: &[CaptureRecord]) -> Self {
        let entries = in_time_order(records)
            .into_iter()
            .map(|record| CaptureIndexEntry {
                capture_id: record.id.clone(),
                trace_id: mapped_id(record, |id| trace.get(id).is_some()),
                started_at_ms: record.started_at_ms,
            })
            .collect();
        Self { entries }
    }

    /// Model call trace ids that have a capture, in time order, each once.
    pub fn captured_trace_ids(&self) -> Vec<String> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.entries
            .iter()
            .filter_map(|e| e.trace_id.as_deref())
            .filter(|id| seen.insert(id))
            .map(String::from)
            .collect()
    }

    /// The capture of one model call, if it has one. If a call somehow has
    /// several, the first is returned. Only the tests need it.
    #[cfg(test)]
    pub fn capture_for(&self, trace_id: &str) -> Option<&CaptureIndexEntry> {
        self.entries
            .iter()
            .find(|e| e.trace_id.as_deref() == Some(trace_id))
    }

    /// Every entry, in time order. Only the tests need the whole list.
    #[cfg(test)]
    pub fn entries(&self) -> &[CaptureIndexEntry] {
        &self.entries
    }
}

/// The trace id a record maps to: the model call named by its response
/// message id, if `known` says that call is in the trace.
pub(crate) fn mapped_id(record: &CaptureRecord, known: impl Fn(&str) -> bool) -> Option<String> {
    record
        .message_id
        .as_deref()
        .map(model_call_id)
        .filter(|id| known(id))
}

/// What the capture views need from a trace, copied out of it so the views
/// can be built without holding the open session's lock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraceFacts {
    /// Every model call id in the trace.
    model_calls: HashSet<String>,
    /// For each model call, the model call before it in the same run.
    previous: HashMap<String, String>,
}

impl TraceFacts {
    /// Copies the model calls of `trace` and their order within each run.
    pub fn of(trace: &Trace) -> Self {
        let mut facts = Self::default();
        for (_, calls) in model_calls_by_run(trace) {
            for pair in calls.windows(2) {
                facts.previous.insert(pair[1].clone(), pair[0].clone());
            }
        }
        let mut stack: Vec<&str> = trace.roots().map(|e| e.id.as_str()).collect();
        while let Some(id) = stack.pop() {
            stack.extend(trace.children(id).map(|child| child.id.as_str()));
            if trace
                .get(id)
                .is_some_and(|event| matches!(event.node, Node::ModelCall(_)))
            {
                facts.model_calls.insert(id.to_string());
            }
        }
        facts
    }

    /// The trace id `record` maps to, if any.
    fn trace_id(&self, record: &CaptureRecord) -> Option<String> {
        mapped_id(record, |id| self.model_calls.contains(id))
    }
}

/// A session's records as the app caches them, with each record's summary
/// once it has been built and the reasons for stored data that could not
/// be read. Cloning only copies shared pointers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionRecords {
    /// The records, in stored order.
    pub records: Arc<Vec<CaptureRecord>>,
    /// Summaries by capture id; `None` for a record whose body is not a
    /// JSON object. A record without an entry is not summarised yet.
    pub summaries: Arc<HashMap<String, Option<RequestSummary>>>,
    /// Context measures by capture id, built with the summaries; `None` for
    /// a record whose body is not JSON. A record without an entry is not
    /// measured yet.
    pub measures: Arc<HashMap<String, Option<ContextMeasure>>>,
    /// Reasons for stored data that could not be read.
    pub skipped: Arc<Vec<String>>,
}

impl SessionRecords {
    /// Records with no summaries built yet.
    pub fn new(records: Vec<CaptureRecord>, skipped: Vec<String>) -> Self {
        Self {
            records: Arc::new(records),
            summaries: Arc::new(HashMap::new()),
            measures: Arc::new(HashMap::new()),
            skipped: Arc::new(skipped),
        }
    }

    /// Adds one record at the end, without summarising it.
    pub fn push(&mut self, record: CaptureRecord) {
        Arc::make_mut(&mut self.records).push(record);
    }

    /// Builds the summaries and context measures of the records that have
    /// none yet, so each record is summarised and measured at most once.
    pub fn summarise_missing(&mut self) {
        let built: Vec<(String, Option<RequestSummary>)> = self
            .records
            .iter()
            .filter(|r| !self.summaries.contains_key(&r.id))
            .map(|r| (r.id.clone(), summary_of(r)))
            .collect();
        if !built.is_empty() {
            Arc::make_mut(&mut self.summaries).extend(built);
        }
        let missing: Vec<&CaptureRecord> = self
            .records
            .iter()
            .filter(|r| !self.measures.contains_key(&r.id))
            .collect();
        if missing.is_empty() {
            return;
        }
        let rules = context_rules();
        let measured: Vec<(String, Option<ContextMeasure>)> = missing
            .into_iter()
            .map(|r| (r.id.clone(), measure_of(r, &rules)))
            .collect();
        Arc::make_mut(&mut self.measures).extend(measured);
    }

    /// The summary of one record. One that was not summarised yet is
    /// summarised here (and not kept).
    fn summary(&self, record: &CaptureRecord) -> Option<RequestSummary> {
        match self.summaries.get(&record.id) {
            Some(summary) => summary.clone(),
            None => summary_of(record),
        }
    }

    /// The system prompt hash of one record, if it has one.
    fn system_hash(&self, record: &CaptureRecord) -> Option<String> {
        match self.summaries.get(&record.id) {
            Some(summary) => summary.as_ref().and_then(|s| s.system_hash.clone()),
            None => summary_of(record).and_then(|s| s.system_hash),
        }
    }
}

/// One captured call in the session overview.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallSummary {
    /// The capture record id.
    pub capture_id: String,
    /// The model call trace id, if the call maps to one.
    pub trace_id: Option<String>,
    /// When the request was received, milliseconds since 1970-01-01 UTC.
    pub started_at_ms: i64,
    /// From request received to response finished, if it finished.
    pub duration_ms: Option<i64>,
    /// The requested model.
    pub model: Option<String>,
    /// The HTTP status of the response, if there was one.
    pub status: Option<u16>,
    /// A failure description, if the call failed.
    pub error: Option<String>,
    /// The system prompt version number (from 1), if the request has one.
    pub system_version: Option<usize>,
    /// The tool set version number (from 1), if the request has one.
    pub tools_version: Option<usize>,
    /// Number of messages in the request.
    pub message_count: usize,
}

/// One distinct system prompt or tool set seen in the session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionSummary {
    /// Version number, from 1 in order of first appearance.
    pub version: usize,
    /// Content hash of this version.
    pub hash: String,
    /// The first capture that used it.
    pub first_capture_id: String,
    /// How many captures used it.
    pub call_count: usize,
    /// Length of the system prompt text, for system prompt versions only.
    pub system_chars: Option<usize>,
    /// The tool names in order, for tool set versions only.
    pub tool_names: Option<Vec<String>>,
}

/// Everything the session panel shows about a session's captures.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureOverview {
    /// The capture store key of the session, if it has a usable one.
    pub session_key: Option<String>,
    /// Bytes stored for the session.
    pub total_bytes: u64,
    /// Every captured call, in time order.
    pub calls: Vec<CallSummary>,
    /// Each distinct system prompt, in order of first appearance.
    pub system_versions: Vec<VersionSummary>,
    /// Each distinct tool set, in order of first appearance.
    pub tool_versions: Vec<VersionSummary>,
    /// Captures that match no model call in the trace (for example title
    /// calls), in time order.
    pub other_call_ids: Vec<String>,
    /// Reasons for stored data that could not be read.
    pub skipped: Vec<String>,
}

/// One captured call in full, with the changes since the call before it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureDetail {
    /// The whole record.
    pub record: CaptureRecord,
    /// A short description of the request, if its body is a JSON object.
    pub summary: Option<RequestSummary>,
    /// The capture this one is compared with, if there is one.
    pub previous_capture_id: Option<String>,
    /// What changed since the previous capture, when both bodies are JSON
    /// objects.
    pub diff: Option<RequestDiff>,
}

/// The overview of a session's captures. `key` is the session's capture
/// store key and `total_bytes` the size stored for it. Uses the summaries
/// kept in `captures`; call `summarise_missing` first so none are rebuilt.
pub fn overview(
    facts: &TraceFacts,
    captures: &SessionRecords,
    key: Option<&str>,
    total_bytes: u64,
) -> CaptureOverview {
    let mut systems = Versions::default();
    let mut tools = Versions::default();
    let mut calls = Vec::new();
    let mut other_call_ids = Vec::new();
    for record in in_time_order(&captures.records) {
        let summary = captures.summary(record);
        let trace_id = facts.trace_id(record);
        let system_version = summary.as_ref().and_then(|s| {
            let hash = s.system_hash.as_deref()?;
            Some(systems.count(hash, &record.id, || (Some(s.system_chars), None)))
        });
        let tools_version = summary.as_ref().and_then(|s| {
            let hash = s.tools_hash.as_deref()?;
            Some(tools.count(hash, &record.id, || (None, Some(s.tool_names.clone()))))
        });
        if trace_id.is_none() {
            other_call_ids.push(record.id.clone());
        }
        calls.push(CallSummary {
            capture_id: record.id.clone(),
            trace_id,
            started_at_ms: record.started_at_ms,
            duration_ms: record.ended_at_ms.map(|end| end - record.started_at_ms),
            model: summary.as_ref().and_then(|s| s.model.clone()),
            status: record.response.as_ref().map(|r| r.status),
            error: record.error.clone(),
            system_version,
            tools_version,
            message_count: summary.as_ref().map_or(0, |s| s.message_count),
        });
    }
    CaptureOverview {
        session_key: key.map(String::from),
        total_bytes,
        calls,
        system_versions: systems.list,
        tool_versions: tools.list,
        other_call_ids,
        skipped: captures.skipped.to_vec(),
    }
}

/// One capture in full, or `None` if no record has that id.
///
/// The previous capture is the one of the previous model call of the same
/// agent when this capture maps to a model call. Otherwise it is the latest
/// earlier capture with the same system prompt.
pub fn detail(
    facts: &TraceFacts,
    captures: &SessionRecords,
    capture_id: &str,
) -> Option<CaptureDetail> {
    let record = captures.records.iter().find(|r| r.id == capture_id)?;
    let ordered = in_time_order(&captures.records);
    let previous = match facts.trace_id(record) {
        Some(trace_id) => facts.previous.get(&trace_id).and_then(|previous| {
            ordered
                .iter()
                .copied()
                .find(|r| facts.trace_id(r).as_deref() == Some(previous.as_str()))
        }),
        None => {
            let position = ordered.iter().position(|r| r.id == capture_id)?;
            captures.system_hash(record).and_then(|system| {
                ordered[..position].iter().rev().copied().find(|r| {
                    r.started_at_ms < record.started_at_ms
                        && captures.system_hash(r).as_deref() == Some(system.as_str())
                })
            })
        }
    };
    Some(CaptureDetail {
        record: record.clone(),
        summary: captures.summary(record),
        previous_capture_id: previous.map(|p| p.id.clone()),
        diff: previous.and_then(|p| capture_core::diff(p, record)),
    })
}

#[cfg(test)]
thread_local! {
    /// How many records this thread summarised, so tests can check that
    /// summaries are built only when needed.
    static SUMMARISE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many records this thread has summarised so far.
#[cfg(test)]
pub(crate) fn summarise_calls() -> usize {
    SUMMARISE_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// How many records this thread measured, so tests can check that
    /// measures are built only once.
    static MEASURE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many records this thread has measured so far.
#[cfg(test)]
pub(crate) fn measure_calls() -> usize {
    MEASURE_CALLS.with(std::cell::Cell::get)
}

/// Measures what fills one record's request. `None` when the body is not
/// JSON. Every capture measure in the app is built here.
fn measure_of(record: &CaptureRecord, rules: &ContextRules) -> Option<ContextMeasure> {
    #[cfg(test)]
    MEASURE_CALLS.with(|c| c.set(c.get() + 1));
    record
        .request
        .body
        .json()
        .map(|body| measure_request(body, rules))
}

/// Summarises one record. Every summary in the app is built here.
fn summary_of(record: &CaptureRecord) -> Option<RequestSummary> {
    #[cfg(test)]
    SUMMARISE_CALLS.with(|c| c.set(c.get() + 1));
    summarise(record)
}

/// The records sorted by `started_at_ms`, keeping stored order for ties.
fn in_time_order(records: &[CaptureRecord]) -> Vec<&CaptureRecord> {
    let mut sorted: Vec<&CaptureRecord> = records.iter().collect();
    sorted.sort_by_key(|r| r.started_at_ms);
    sorted
}

/// Numbers distinct hashes from 1 in order of first appearance and counts
/// their uses.
#[derive(Default)]
struct Versions {
    list: Vec<VersionSummary>,
}

impl Versions {
    /// Counts one use of `hash` by `capture_id` and returns its version
    /// number. `details` gives `(system_chars, tool_names)` for a new hash.
    fn count(
        &mut self,
        hash: &str,
        capture_id: &str,
        details: impl FnOnce() -> (Option<usize>, Option<Vec<String>>),
    ) -> usize {
        if let Some(existing) = self.list.iter_mut().find(|v| v.hash == hash) {
            existing.call_count += 1;
            return existing.version;
        }
        let (system_chars, tool_names) = details();
        let version = self.list.len() + 1;
        self.list.push(VersionSummary {
            version,
            hash: hash.to_string(),
            first_capture_id: capture_id.to_string(),
            call_count: 1,
            system_chars,
            tool_names,
        });
        version
    }
}

/// The invented capture fixture (`fixtures/captures/basic/calls.jsonl`)
/// for the sample session, one record per line.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn fixture_records() -> Vec<CaptureRecord> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("captures")
        .join("basic")
        .join("calls.jsonl");
    let text = std::fs::read_to_string(path).expect("read capture fixture");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a CaptureRecord"))
        .collect()
}

/// `count` invented records shaped like real ones: a large system prompt,
/// 180 tools and a growing message list. Every record copies the message id
/// of a fixture record in turn, so most map to fixture model calls.
#[cfg(test)]
pub(crate) fn synthetic_records(count: usize) -> Vec<CaptureRecord> {
    use serde_json::json;
    let base = fixture_records();
    let system = json!([{ "type": "text", "text": "An invented system prompt. ".repeat(2_000) }]);
    let tools: Vec<serde_json::Value> = (0..180)
        .map(|i| {
            json!({
                "name": format!("Tool{i}"),
                "description": "An invented tool description. ".repeat(40),
                "input_schema": { "type": "object", "properties": { "path": { "type": "string" } } },
            })
        })
        .collect();
    (0..count)
        .map(|i| {
            let mut record = base[i % base.len()].clone();
            record.id = format!("synthetic-{i:04}");
            record.started_at_ms = 1_000_000 + i as i64;
            let messages: Vec<serde_json::Value> = (0..=i % 50)
                .map(|m| json!({ "role": "user", "content": format!("message {m}") }))
                .collect();
            record.request.body = capture_core::Body::Json(json!({
                "model": "test-model",
                "system": system,
                "tools": tools,
                "messages": messages,
            }));
            record
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use trace_view::model_calls_by_run;

    const SESSION: &str = "00000000-0000-4000-8000-000000000002";
    const TITLE_MESSAGE_ID: &str = "msg_title_test";

    fn repo_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn fixture_trace() -> Trace {
        let path = repo_path()
            .join("fixtures")
            .join("claude-code")
            .join("basic")
            .join(format!("{SESSION}.jsonl"));
        let session = adapter_claude_code::load_session(&path).expect("load fixture");
        let mut trace = Trace::new();
        for event in session.events {
            trace.apply(event).expect("fixture events form a tree");
        }
        trace
    }

    fn capture_of(records: &[CaptureRecord], trace_id: &str) -> String {
        records
            .iter()
            .find(|r| r.message_id.as_deref().map(model_call_id).as_deref() == Some(trace_id))
            .map(|r| r.id.clone())
            .expect("a capture for the call")
    }

    fn title_capture(records: &[CaptureRecord]) -> String {
        records
            .iter()
            .find(|r| r.message_id.as_deref() == Some(TITLE_MESSAGE_ID))
            .map(|r| r.id.clone())
            .expect("the title call")
    }

    /// The detail of one capture among `records`, summarised up front as
    /// the app does.
    fn detail_of(trace: &Trace, records: &[CaptureRecord], id: &str) -> Option<CaptureDetail> {
        let mut captures = SessionRecords::new(records.to_vec(), Vec::new());
        captures.summarise_missing();
        detail(&TraceFacts::of(trace), &captures, id)
    }

    /// (main agent calls, subagent calls), each in order.
    fn calls_by_agent(trace: &Trace) -> (Vec<String>, Vec<String>) {
        let runs = model_calls_by_run(trace);
        let main = runs.first().expect("main run").1.clone();
        let sub: Vec<String> = runs[1..].iter().flat_map(|(_, c)| c.clone()).collect();
        (main, sub)
    }

    #[test]
    fn index_maps_every_fixture_call_but_the_title_call() {
        let trace = fixture_trace();
        let records = fixture_records();
        let index = CaptureIndex::build(&trace, &records);
        assert_eq!(index.entries().len(), records.len());

        let mut expected: Vec<String> = model_calls_by_run(&trace)
            .into_iter()
            .flat_map(|(_, calls)| calls)
            .collect();
        expected.sort();
        let mut captured = index.captured_trace_ids();
        captured.sort();
        assert_eq!(captured, expected);

        let unmapped: Vec<&CaptureIndexEntry> = index
            .entries()
            .iter()
            .filter(|e| e.trace_id.is_none())
            .collect();
        assert_eq!(unmapped.len(), 1);
        assert_eq!(unmapped[0].capture_id, title_capture(&records));

        let first = &expected[0];
        let entry = index.capture_for(first).expect("an entry");
        assert_eq!(entry.capture_id, capture_of(&records, first));
        assert!(index.capture_for("model:nope").is_none());

        let times: Vec<i64> = index.entries().iter().map(|e| e.started_at_ms).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "time order");
    }

    #[test]
    fn overview_lists_versions_and_other_calls() {
        let trace = fixture_trace();
        let records = fixture_records();
        let mut captures = SessionRecords::new(records.clone(), vec!["a reason".into()]);
        captures.summarise_missing();
        let view = overview(&TraceFacts::of(&trace), &captures, Some(SESSION), 42);
        assert_eq!(view.session_key.as_deref(), Some(SESSION));
        assert_eq!(view.total_bytes, 42);
        assert_eq!(view.skipped, vec!["a reason".to_string()]);
        assert_eq!(view.calls.len(), records.len());
        assert_eq!(view.other_call_ids, vec![title_capture(&records)]);

        assert_eq!(view.system_versions.len(), 2, "main agent and subagent");
        assert_eq!(view.tool_versions.len(), 2, "base set and base + WebFetch");
        for (i, v) in view.system_versions.iter().enumerate() {
            assert_eq!(v.version, i + 1);
            assert!(v.system_chars.is_some_and(|n| n > 0));
            assert!(v.tool_names.is_none());
        }
        let tool_names: Vec<&Vec<String>> = view
            .tool_versions
            .iter()
            .map(|v| v.tool_names.as_ref().expect("tool names"))
            .collect();
        assert!(!tool_names[0].contains(&"WebFetch".to_string()));
        assert!(tool_names[1].contains(&"WebFetch".to_string()));
        assert!(view.tool_versions.iter().all(|v| v.system_chars.is_none()));

        let calls_with_system = view
            .calls
            .iter()
            .filter(|c| c.system_version.is_some())
            .count();
        let counted: usize = view.system_versions.iter().map(|v| v.call_count).sum();
        assert_eq!(counted, calls_with_system);

        let (main, _) = calls_by_agent(&trace);
        let first_main = &view.calls[0];
        assert_eq!(first_main.trace_id.as_ref(), Some(&main[0]));
        assert_eq!(first_main.system_version, Some(1));
        assert_eq!(first_main.tools_version, Some(1));
        assert_eq!(
            view.system_versions[0].first_capture_id,
            first_main.capture_id
        );
        let first_record = records
            .iter()
            .find(|r| r.id == first_main.capture_id)
            .expect("record");
        assert_eq!(
            first_main.duration_ms,
            first_record
                .ended_at_ms
                .map(|e| e - first_record.started_at_ms)
        );
        assert_eq!(first_main.status, Some(200));
        assert!(first_main.model.is_some());
        assert!(first_main.message_count > 0);

        let times: Vec<i64> = view.calls.iter().map(|c| c.started_at_ms).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "time order");
    }

    #[test]
    fn detail_pairs_with_the_previous_call_of_the_same_agent() {
        let trace = fixture_trace();
        let records = fixture_records();
        let (main, sub) = calls_by_agent(&trace);

        let second = detail_of(&trace, &records, &capture_of(&records, &main[1])).expect("detail");
        assert_eq!(
            second.previous_capture_id,
            Some(capture_of(&records, &main[0]))
        );
        let diff = second.diff.expect("a diff");
        assert!(diff.shared_prefix > 0);
        assert!(second.summary.is_some());

        let first_sub =
            detail_of(&trace, &records, &capture_of(&records, &sub[0])).expect("detail");
        assert_eq!(first_sub.previous_capture_id, None);
        assert!(first_sub.diff.is_none());

        let webfetch =
            detail_of(&trace, &records, &capture_of(&records, &main[3])).expect("detail");
        assert_eq!(
            webfetch.diff.expect("a diff").tools_added,
            vec!["WebFetch".to_string()]
        );
    }

    #[test]
    fn detail_of_an_unmapped_call_pairs_by_system_prompt() {
        let trace = fixture_trace();
        let records = fixture_records();
        let title_id = title_capture(&records);
        let title = detail_of(&trace, &records, &title_id).expect("detail");
        let this = records.iter().find(|r| r.id == title_id).expect("record");
        let system = title.summary.as_ref().and_then(|s| s.system_hash.clone());
        let expected = records
            .iter()
            .filter(|r| r.started_at_ms < this.started_at_ms)
            .filter(|r| {
                system.is_some() && capture_core::summarise(r).and_then(|s| s.system_hash) == system
            })
            .max_by_key(|r| r.started_at_ms)
            .map(|r| r.id.clone());
        assert_eq!(title.previous_capture_id, expected);
        assert_eq!(title.diff.is_some(), expected.is_some());
    }

    /// A copy of `record` with a new id, no message id (so it maps to no
    /// model call) and the given start time.
    fn unmapped_copy(record: &CaptureRecord, id: &str, started_at_ms: i64) -> CaptureRecord {
        let mut copy = record.clone();
        copy.id = id.to_string();
        copy.message_id = None;
        copy.started_at_ms = started_at_ms;
        copy.ended_at_ms = Some(started_at_ms + 1_000);
        copy
    }

    #[test]
    fn unmapped_call_pairs_with_the_latest_earlier_call_of_the_same_system_prompt() {
        let trace = fixture_trace();
        let mut records = fixture_records();
        let (main, _) = calls_by_agent(&trace);
        let main_records: Vec<&CaptureRecord> = main
            .iter()
            .map(|call| {
                let id = capture_of(&records, call);
                records.iter().find(|r| r.id == id).expect("record")
            })
            .collect();
        let latest_main = main_records
            .iter()
            .max_by_key(|r| r.started_at_ms)
            .expect("main calls");
        let last_start = records
            .iter()
            .map(|r| r.started_at_ms)
            .max()
            .expect("records");
        let extra = unmapped_copy(main_records[0], "extra-001", last_start + 10_000);
        let expected_previous = latest_main.id.clone();
        records.push(extra);

        let view = detail_of(&trace, &records, "extra-001").expect("detail");
        assert_eq!(view.previous_capture_id, Some(expected_previous));
        assert!(view.diff.is_some());
    }

    #[test]
    fn unmapped_call_does_not_pair_with_a_call_that_starts_at_the_same_time() {
        let trace = fixture_trace();
        let records = fixture_records();
        let (main, _) = calls_by_agent(&trace);
        let first_id = capture_of(&records, &main[0]);
        let first = records
            .iter()
            .find(|r| r.id == first_id)
            .expect("record")
            .clone();
        // The first main call is the only earlier call with its system
        // prompt once the later main calls are left out.
        let mut set = vec![first.clone()];
        set.push(unmapped_copy(&first, "extra-tie", first.started_at_ms));
        let view = detail_of(&trace, &set, "extra-tie").expect("detail");
        assert_eq!(view.previous_capture_id, None);
        assert!(view.diff.is_none());

        // One millisecond later it does pair.
        set[1].started_at_ms += 1;
        let view = detail_of(&trace, &set, "extra-tie").expect("detail");
        assert_eq!(view.previous_capture_id, Some(first_id));
    }

    #[test]
    fn detail_of_an_unknown_id_is_none() {
        let trace = fixture_trace();
        let records = fixture_records();
        assert!(detail_of(&trace, &records, "no-such-capture").is_none());
    }
}
