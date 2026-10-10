//! Tests the adapter against the invented fixture in
//! `fixtures/claude-code/compaction`: one prompt full of waste, a manual
//! `/compact` with re-attached files, and a prompt that reads a dropped file
//! again.

// Test-only file: panicking on a bad fixture is the right failure mode here.
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use adapter_claude_code::{Parser, flag_rules, load_session};
use insights::{FlagKind, session_flags};
use trace_core::{ContextPart, ContextPartKind, Node, Trace, TraceEvent};

const SESSION: &str = "00000000-0000-4000-8000-000000000003";
const CALC: &str = r"C:\work\demo\src\calc.py";
const MAIN: &str = r"C:\work\demo\src\main.py";
const NOTES: &str = r"C:\work\demo\NOTES.txt";
const CALC_TEXT: &str = "def add(a, b):\n    return a + b\n\ndef sub(a, b):\n    return a - b\n";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("claude-code")
        .join("compaction")
        .join(format!("{SESSION}.jsonl"))
}

fn load() -> (Trace, Vec<TraceEvent>) {
    let session = load_session(&fixture_path()).expect("load");
    assert!(
        session.skipped.is_empty(),
        "every fixture line type should be understood: {:#?}",
        session.skipped
    );
    let mut trace = Trace::new();
    for event in &session.events {
        trace
            .apply(event.clone())
            .expect("events must form a valid tree");
    }
    (trace, session.events)
}

/// Every node in the trace, depth first.
fn all_nodes(trace: &Trace) -> Vec<&TraceEvent> {
    let mut out = Vec::new();
    let mut stack: Vec<&TraceEvent> = trace.roots().collect();
    while let Some(event) = stack.pop() {
        out.push(event);
        stack.extend(trace.children(&event.id));
    }
    out
}

fn compaction_markers(trace: &Trace) -> Vec<&TraceEvent> {
    all_nodes(trace)
        .into_iter()
        .filter(|e| matches!(&e.node, Node::Marker(m) if m.kind == "compaction"))
        .collect()
}

fn context_parts(trace: &Trace) -> Vec<&ContextPart> {
    all_nodes(trace)
        .into_iter()
        .filter_map(|e| match &e.node {
            Node::ContextUpdate(u) => Some(u.parts.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn check_details(event: &TraceEvent) {
    let Node::Marker(m) = &event.node else {
        panic!("not a marker")
    };
    let c = m.compaction.as_ref().expect("compaction details");
    assert_eq!(c.tokens_before, Some(120_500));
    assert_eq!(c.tokens_after, Some(8_200));
    assert_eq!(c.files_restored, [CALC]);
    assert_eq!(c.files_referenced, [NOTES, MAIN]);
}

#[test]
fn compaction_marker_has_details() {
    let (trace, _) = load();
    let markers = compaction_markers(&trace);
    assert_eq!(markers.len(), 1);
    check_details(markers[0]);
    assert_eq!(
        markers[0].metadata["compact_metadata"]["cumulativeDroppedTokens"],
        112_300
    );
}

#[test]
fn re_sent_marker_keeps_one_id() {
    // Feed the lines one at a time, as a live session would arrive.
    let text = std::fs::read_to_string(fixture_path()).expect("read fixture");
    let mut parser = Parser::for_session("live.jsonl", SESSION);
    let mut trace = Trace::new();
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for event in parser.push_line(i as u64 + 1, line) {
            trace.apply(event.clone()).expect("valid tree");
            events.push(event);
        }
    }
    for event in parser.finish() {
        trace.apply(event).expect("valid tree");
    }
    assert!(parser.skipped().is_empty(), "{:#?}", parser.skipped());

    let markers = compaction_markers(&trace);
    assert_eq!(markers.len(), 1);
    let id = markers[0].id.clone();
    let sent = events.iter().filter(|e| e.id == id).count();
    assert!(sent > 1, "the marker is re-sent as files arrive ({sent})");
    check_details(markers[0]);
    // The re-sent marker keeps the raw lines that added to it.
    assert_eq!(markers[0].raw.len(), 4, "boundary plus three file lines");
}

#[test]
fn restored_file_is_a_file_part() {
    let (trace, _) = load();
    let key = format!("file:{CALC}");
    let parts = context_parts(&trace);
    let part = parts
        .iter()
        .find(|p| p.key == key)
        .expect("a file part for calc.py");
    assert_eq!(part.kind, ContextPartKind::File);
    assert_eq!(part.label, CALC);
    assert_eq!(part.text, CALC_TEXT);
    // A file named without content gives no part.
    assert!(!parts.iter().any(|p| p.key.contains("NOTES.txt")));
}

#[test]
fn invoked_skills_is_a_reminder() {
    let (trace, _) = load();
    let parts = context_parts(&trace);
    let part = parts
        .iter()
        .find(|p| p.label == "invoked skills")
        .expect("an invoked skills part");
    assert_eq!(part.kind, ContextPartKind::Reminder);
    assert!(part.key.starts_with("reminder:"));
    assert_eq!(
        part.text,
        "Example skill: keep functions short and named plainly."
    );
}

#[test]
fn fixture_flags() {
    let (trace, _) = load();
    let flags = session_flags(&trace, &flag_rules());
    let kinds: Vec<FlagKind> = flags.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FlagKind::RepeatedRead,
            FlagKind::FailedRetry,
            FlagKind::Loop,
            FlagKind::LargeResult,
            FlagKind::Compaction,
            FlagKind::RereadAfterCompaction,
        ],
        "{flags:#?}"
    );
    assert_eq!(
        flags[4].text,
        "Compaction dropped about 112k tokens (kept about 8k). Re-attached 1 file in full and 2 by name only."
    );
    assert_eq!(
        flags[5].text,
        format!("`{MAIN}` was read again after compaction dropped it.")
    );
    assert_eq!(flags[0].trace_ids.len(), 3, "three reads of calc.py");
}
