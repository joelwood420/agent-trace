//! Checks the invented capture fixture against the sanitised sample session.
//! Regenerate the fixture with `cargo run -p capture --example make_fixture`.

#![allow(clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use adapter_claude_code::{SESSION_HEADER, load_session, model_call_id};
use capture::CaptureStore;
use capture_core::{CaptureRecord, SYSTEM_KEY, TOOLS_KEY, content_hash, diff};
use trace_core::Trace;
use trace_view::model_calls_by_run;

const SESSION_ID: &str = "00000000-0000-4000-8000-000000000002";
const TITLE_MESSAGE_ID: &str = "msg_title_test";

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn fixture_records() -> Vec<CaptureRecord> {
    let text = fs::read_to_string(repo_path("fixtures/captures/basic/calls.jsonl"))
        .expect("read the capture fixture");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("each line is a CaptureRecord"))
        .collect()
}

fn fixture_trace() -> Trace {
    let path = repo_path(&format!("fixtures/claude-code/basic/{SESSION_ID}.jsonl"));
    let session = load_session(&path).expect("load the sample session");
    let mut trace = Trace::new();
    for event in session.events {
        trace.apply(event).expect("fixture events form a tree");
    }
    trace
}

#[test]
fn every_model_call_has_one_record_plus_the_title_call() {
    let records = fixture_records();
    let trace = fixture_trace();
    let runs = model_calls_by_run(&trace);
    let call_ids: HashSet<String> = runs.iter().flat_map(|(_, c)| c.clone()).collect();

    assert_eq!(records.len(), call_ids.len() + 1);

    let mut seen = HashSet::new();
    for record in &records {
        assert_eq!(record.header(SESSION_HEADER), Some(SESSION_ID));
        let message_id = record
            .message_id
            .as_deref()
            .expect("every record has an id");
        if message_id == TITLE_MESSAGE_ID {
            continue;
        }
        let id = model_call_id(message_id);
        assert!(call_ids.contains(&id), "{id} is not a model call");
        assert!(seen.insert(id), "{message_id} appears twice");
    }
    assert_eq!(seen.len(), call_ids.len());
}

#[test]
fn consecutive_main_agent_requests_share_a_prefix() {
    let records = fixture_records();
    let trace = fixture_trace();
    let runs = model_calls_by_run(&trace);
    let main_calls = &runs.first().expect("a main run").1;
    let main: Vec<&CaptureRecord> = main_calls
        .iter()
        .map(|call| {
            records
                .iter()
                .find(|r| r.message_id.as_deref().map(model_call_id).as_deref() == Some(call))
                .expect("a record for every main-agent call")
        })
        .collect();
    assert!(main.len() > 3);
    for pair in main.windows(2) {
        let d = diff(pair[0], pair[1]).expect("both bodies are JSON objects");
        assert!(d.shared_prefix > 0, "{} -> {}", pair[0].id, pair[1].id);
        assert!(!d.added.is_empty(), "{} adds messages", pair[1].id);
    }
    let d = diff(main[2], main[3]).expect("diff");
    assert_eq!(d.tools_added, vec!["WebFetch".to_string()]);
}

#[test]
fn the_store_round_trips_every_fixture_record() {
    let root = std::env::temp_dir().join(format!(
        "snitchcraft-fixture-round-trip-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let store = CaptureStore::new(root.clone());
    let records = fixture_records();
    for record in &records {
        store.append(SESSION_ID, record).expect("append");
    }
    let loaded = store.load(SESSION_ID).expect("load");
    let _ = fs::remove_dir_all(&root);
    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    assert_eq!(loaded.records, records);
}

#[test]
fn the_fixture_has_two_system_prompts_and_two_tool_sets() {
    let records = fixture_records();
    let distinct = |key: &str| {
        records
            .iter()
            .filter_map(|r| r.request.body.json()?.get(key))
            .map(content_hash)
            .collect::<HashSet<_>>()
            .len()
    };
    assert_eq!(distinct(SYSTEM_KEY), 2, "main agent and subagent");
    assert_eq!(
        distinct(TOOLS_KEY),
        2,
        "base set, and base set plus WebFetch"
    );
}
