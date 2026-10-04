//! Tests for following a session while it is written, using temp copies of
//! the sanitised fixture in `fixtures/claude-code/basic`.

// Test-only file: panicking on a bad fixture is the right failure mode here.
#![allow(clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};

use adapter_claude_code::{PollOutcome, ReadMode, SessionFollower, load_session};
use trace_core::{Node, Trace, TraceEvent};

const SESSION: &str = "00000000-0000-4000-8000-000000000002";
const AGENT: &str = "a0000000000000159";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("claude-code")
        .join("basic")
}

fn fixture_main() -> Vec<u8> {
    std::fs::read(fixture_dir().join(format!("{SESSION}.jsonl"))).expect("main")
}

fn fixture_agent() -> Vec<u8> {
    std::fs::read(
        fixture_dir()
            .join(SESSION)
            .join("subagents")
            .join(format!("agent-{AGENT}.jsonl")),
    )
    .expect("agent")
}

/// An empty project folder in the temp dir. Returns (main path, subagents dir).
fn temp_session(name: &str) -> (PathBuf, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("snitchcraft-follow-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let subagents = dir.join(SESSION).join("subagents");
    std::fs::create_dir_all(&subagents).expect("mkdir");
    (dir.join(format!("{SESSION}.jsonl")), subagents)
}

fn append(path: &Path, bytes: &[u8]) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open");
    f.write_all(bytes).expect("write");
}

fn changed(outcome: PollOutcome) -> Vec<TraceEvent> {
    match outcome {
        PollOutcome::Changed(session) => {
            assert!(session.skipped.is_empty(), "{:#?}", session.skipped);
            session.events
        }
        PollOutcome::Rewritten => panic!("unexpected rewrite"),
        PollOutcome::Missing => panic!("unexpected missing file"),
    }
}

/// Every node of a trace, depth first from the roots, in child order.
fn walk(trace: &Trace) -> Vec<TraceEvent> {
    fn visit(trace: &Trace, event: &TraceEvent, out: &mut Vec<TraceEvent>) {
        out.push(event.clone());
        for child in trace.children(&event.id) {
            visit(trace, child, out);
        }
    }
    let mut out = Vec::new();
    for root in trace.roots() {
        visit(trace, root, &mut out);
    }
    out
}

fn apply_all(trace: &mut Trace, events: Vec<TraceEvent>) {
    for event in events {
        trace.apply(event).expect("events must form a valid tree");
    }
}

/// Small deterministic pseudo-random sizes, so failures can be replayed.
struct Lcg(u64);
impl Lcg {
    fn next_size(&mut self, max: u64) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % max + 1) as usize
    }
}

#[test]
fn follower_fed_in_chunks_matches_one_shot_load() {
    let expected = {
        let session = load_session(&fixture_dir().join(format!("{SESSION}.jsonl"))).expect("load");
        let mut trace = Trace::new();
        apply_all(&mut trace, session.events);
        walk(&trace)
    };

    for seed in [1_u64, 7, 42] {
        let (main_path, subagents) = temp_session(&format!("chunks-{seed}"));
        let agent_path = subagents.join(format!("agent-{AGENT}.jsonl"));
        std::fs::copy(
            fixture_dir()
                .join(SESSION)
                .join("subagents")
                .join(format!("agent-{AGENT}.meta.json")),
            subagents.join(format!("agent-{AGENT}.meta.json")),
        )
        .expect("meta");
        let (main, agent) = (fixture_main(), fixture_agent());
        let (mut m, mut a) = (0_usize, 0_usize);
        let mut rng = Lcg(seed);
        let mut follower = SessionFollower::new(&main_path).expect("follower");
        let mut trace = Trace::new();
        // The main file must exist before the first poll.
        append(&main_path, b"");
        while m < main.len() || a < agent.len() {
            let n = rng.next_size(97).min(main.len() - m);
            append(&main_path, &main[m..m + n]);
            m += n;
            let n = rng.next_size(97).min(agent.len() - a);
            append(&agent_path, &agent[a..a + n]);
            a += n;
            apply_all(
                &mut trace,
                changed(follower.poll(ReadMode::Live).expect("poll")),
            );
        }
        apply_all(
            &mut trace,
            changed(follower.poll(ReadMode::Live).expect("poll")),
        );
        assert_eq!(walk(&trace), expected, "seed {seed}");
    }
}

#[test]
fn subagent_file_appearing_later_is_picked_up() {
    let (main_path, subagents) = temp_session("later");
    append(&main_path, &fixture_main());
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(
        &mut trace,
        changed(follower.poll(ReadMode::Live).expect("poll")),
    );
    let run_id = format!("run:agent-{AGENT}");
    assert!(trace.get(&run_id).is_none());

    append(
        &subagents.join(format!("agent-{AGENT}.jsonl")),
        &fixture_agent(),
    );
    apply_all(
        &mut trace,
        changed(follower.poll(ReadMode::Live).expect("poll")),
    );
    assert!(
        trace.get(&run_id).is_some(),
        "subagent run appears once its file exists"
    );
    assert!(trace.children(&run_id).count() > 0);
}

/// The fixture's tool result that links the subagent, as (line index, tool_use_id).
fn linking_result() -> (usize, String) {
    let text = String::from_utf8(fixture_main()).expect("utf8");
    for (i, line) in text.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value
            .pointer("/toolUseResult/agentId")
            .and_then(|v| v.as_str())
            != Some(AGENT)
        {
            continue;
        }
        let blocks = value
            .pointer("/message/content")
            .and_then(|c| c.as_array())
            .expect("content");
        for block in blocks {
            if let Some(id) = block.get("tool_use_id").and_then(|v| v.as_str()) {
                return (i, id.to_string());
            }
        }
    }
    panic!("fixture has no tool result linking {AGENT}");
}

#[test]
fn meta_json_links_a_running_subagent() {
    let (index, tool_use_id) = linking_result();
    let (main_path, subagents) = temp_session("meta");
    let text = String::from_utf8(fixture_main()).expect("utf8");
    let before: String = text.lines().take(index).map(|l| format!("{l}\n")).collect();
    append(&main_path, before.as_bytes());
    append(
        &subagents.join(format!("agent-{AGENT}.jsonl")),
        &fixture_agent(),
    );
    let meta = serde_json::json!({ "description": "Invented task", "toolUseId": tool_use_id });
    append(
        &subagents.join(format!("agent-{AGENT}.meta.json")),
        meta.to_string().as_bytes(),
    );

    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(
        &mut trace,
        changed(follower.poll(ReadMode::Live).expect("poll")),
    );
    let run = trace
        .get(&format!("run:agent-{AGENT}"))
        .expect("linked from meta.json");
    assert_eq!(
        run.parent_id.as_deref(),
        Some(format!("tool:{tool_use_id}").as_str())
    );
    let Node::Run(r) = &run.node else {
        panic!("not a run")
    };
    assert_eq!(r.title.as_deref(), Some("Invented task"));
}

#[test]
fn result_link_after_meta_link_does_not_duplicate() {
    let (index, tool_use_id) = linking_result();
    let (main_path, subagents) = temp_session("no-dup");
    let main = fixture_main();
    let text = String::from_utf8(main.clone()).expect("utf8");
    let before: String = text.lines().take(index).map(|l| format!("{l}\n")).collect();
    append(&main_path, before.as_bytes());
    append(
        &subagents.join(format!("agent-{AGENT}.jsonl")),
        &fixture_agent(),
    );
    let meta = serde_json::json!({ "description": "Invented task", "toolUseId": tool_use_id });
    append(
        &subagents.join(format!("agent-{AGENT}.meta.json")),
        meta.to_string().as_bytes(),
    );

    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(
        &mut trace,
        changed(follower.poll(ReadMode::Live).expect("poll")),
    );
    assert!(
        trace.get(&format!("run:agent-{AGENT}")).is_some(),
        "linked from meta.json"
    );
    let nodes_before = trace.len();

    // Now the tool result arrives, which links the same subagent again.
    append(&main_path, &main[before.len()..]);
    let events = changed(follower.poll(ReadMode::Live).expect("poll"));
    // The subagent file was fully read in the first poll and has not grown,
    // so a second parser for it is the only way subagent lines or its run
    // could show up again here.
    assert!(
        events.iter().all(|e| e.id != format!("run:agent-{AGENT}")),
        "subagent run is not started a second time"
    );
    assert!(
        events
            .iter()
            .all(|e| e.raw.iter().all(|r| !r.source.starts_with("subagents/"))),
        "subagent lines are not parsed a second time"
    );
    apply_all(&mut trace, events);
    assert!(
        trace.len() > nodes_before,
        "the rest of the main transcript was read"
    );
}

#[test]
fn rewritten_and_missing_main_file_are_reported() {
    let (main_path, _) = temp_session("rewrite");
    append(&main_path, &fixture_main());
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    changed(follower.poll(ReadMode::Live).expect("poll"));
    std::fs::write(&main_path, b"").expect("truncate");
    assert!(matches!(
        follower.poll(ReadMode::Live).expect("poll"),
        PollOutcome::Rewritten
    ));

    std::fs::remove_file(&main_path).expect("delete");
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    assert!(matches!(
        follower.poll(ReadMode::Live).expect("poll"),
        PollOutcome::Missing
    ));
}
