# M3 Live Watching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The open session's diagram and the session list update by themselves while Claude Code writes transcripts.

**Architecture:** The adapter gains `TranscriptTail` (reads only new bytes of one file) and `SessionFollower` (follows a main transcript plus its subagent files, linking subagents as they appear). The app gains `LiveSession` (follower + trace + rebuilt diagram), and a background worker thread that owns one recursive `notify` watcher on the projects folder, polls the open session once a second as a safety net, and pushes updates to the UI through Tauri channels. The UI swaps in each new diagram, keeping expanded boxes and selection by id.

**Tech Stack:** Rust stable, Tauri 2 channels (`tauri::ipc::Channel`), `notify` 8, React + TypeScript, Node's built-in test runner.

**Spec:** `docs/superpowers/specs/2026-10-05-m3-live-watching-design.md`. Read it before starting any task.

## Global Constraints

- Every command must work in PowerShell on native Windows. No bash-only syntax in docs or scripts.
- Never write to, move, or delete anything under the user's `.claude` folder. Tests use temp folders and `fixtures/` only.
- No `unwrap()` or `expect()` outside tests. Test files and `#[cfg(test)]` modules may use them (integration test files start with `#![allow(clippy::expect_used)]`).
- No em dashes anywhere: code, comments, docs, commit messages.
- `trace-core` gets no changes. If a task seems to need a schema change, stop and report back.
- Parse defensively: unknown data is skipped and logged, never a panic. An unfinished last line is "not ready yet", never an error.
- Open files only with `std::fs::File::open` (on Windows, std shares read, write and delete, so Claude Code is never blocked).
- Keep the raw source line on every event (already done by `Parser`; do not drop it).
- Paths built with `PathBuf`/`Path::join`. No hardcoded paths, usernames, or separators.
- Every new dependency is added to the table in `docs/DECISIONS.md` with the reason.
- Doc comments on all new public items.
- Before committing, all of these must pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - in `ui`: `npm run lint; npm run typecheck; npm test; npm run build` (only for tasks touching `ui/`, plus Task 3 because it regenerates mock data)
- Commit with a clear message for a public reader, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- Only commit sanitised data. Nothing from the real `.claude` folder goes into the repo.

## Decisions taken while planning (not in the spec)

These refine the spec. Each task that implements one records it in `docs/DECISIONS.md`.

1. **Channels, not events.** Tauri events need the window to be granted `core:event` listen permissions. Channels passed as command arguments need no extra permission. `load_session` takes a channel for its live updates, and a new `watch_sessions` command takes a channel for list changes. The only new window permission is `allow-watch-sessions`.
2. **Subagents are linked from their `.meta.json` too.** Real transcripts show `agent-<id>.meta.json` carries `toolUseId`. A normal (foreground) Agent call only reports its `agentId` in the tool result when the subagent has finished, so without this a running subagent would stay invisible until it ends. The follower links a subagent as soon as its meta file names a tool call that is already in the trace. Result-based linking stays.
3. **A view has a `version`.** Every `SessionView` carries a number that goes up with each update of the open session. The UI drops any view older than the one it shows, because a channel update can arrive before the `load_session` reply.
4. **If the watcher cannot start, polling keeps the open session live.** The status is `no_watcher` and the header says the session list needs Refresh. (The spec said "M2 behaviour"; polling makes it strictly better at no cost.)
5. **The session list change signal is sent at most every 2 seconds**, not 1. `list_sessions` reads every transcript to find titles, so it should not run every second while Claude Code writes.
6. **Live reading holds back a last line with no newline**, even if it is valid JSON. Claude Code ends every line with a newline. Loading a finished file in one go (`adapter_claude_code::load_session`, used by the examples) still accepts such a line, as today.
7. **A plain worker thread, not tokio.** The worker only waits on a channel with a timeout; a `std::thread` is simpler than an async task and needs nothing new.
8. **Closing a session in the UI does not stop polling it.** Polling one file a second costs nothing worth a new command; the UI ignores messages for a session it is not showing. Loading another session replaces it.

## Review Focus

- **Writes that end mid-line or mid-character:** a read that lands in the middle of a line or a UTF-8 character must hold the bytes back and produce the same events once the rest arrives. Pinned by Task 1 (`splits_inside_a_utf8_character`) and Task 2 (`follower_fed_in_chunks_matches_one_shot_load`).
- **A subagent whose file appears before, after, or without its link:** the subagent must appear once, under the right tool call, never twice. Pinned by Task 2 (`subagent_file_appearing_later_is_picked_up`, `meta_json_links_a_running_subagent`, `result_link_after_meta_link_does_not_duplicate`).
- **Watcher paths that do not look like the paths the app built:** `session_path` canonicalises (`\\?\C:\...` on Windows) but `notify` reports paths under the root as given. Matching must use names relative to the watched root. Pinned by Task 4 (`touches_session_matches_relative_names`).
- **A stale reply overwriting a newer update:** the UI must keep the newest version. Pinned by Task 5 (`isNewer_rejects_older_versions`).
- **The selected box disappears or changes after an update:** the details panel must clear when the node is gone and refetch when its content changed, and otherwise not refetch. Pinned by Task 5 (`findNode`, `needsDetailRefetch` tests).

---

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `crates/adapter-claude-code/src/tail.rs` (new) | 1 | `TranscriptTail`: read only new complete lines of one growing file |
| `crates/adapter-claude-code/src/follow.rs` (new) | 2 | `SessionFollower`: main transcript + subagents, subagent linking |
| `crates/adapter-claude-code/src/lib.rs` | 1, 2 | exports; `load_session` rebuilt on the follower |
| `crates/adapter-claude-code/tests/follow.rs` (new) | 2 | chunked-feed and subagent tests against the fixture |
| `src-tauri/src/live.rs` (new) | 3 | `LiveSession`, `LiveMessage`, `LiveStatus`, `is_live` |
| `src-tauri/src/sessions.rs` | 3 | `live`/`version` fields, remove `SessionCache`, snapshot tests |
| `src-tauri/src/watch.rs` (new) | 4 | worker thread, `Scheduler`, `Sink`, `Shared`, path matching |
| `src-tauri/src/main.rs` | 3, 4 | commands and startup |
| `src-tauri/build.rs`, `src-tauri/capabilities/default.json` | 4 | `watch_sessions` permission |
| `ui/src/mock/live-steps.json` (new, generated) | 3 | mock replay of a growing session |
| `ui/src/types.ts`, `ui/src/api.ts` | 5 | new fields, channel-based API |
| `ui/src/live.ts`, `ui/src/live.test.ts` (new) | 5 | pure live-update logic |
| `ui/src/mock/mockApi.ts` | 5 | `?mock&live` replay |
| `ui/src/App.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/components/DetailsPanel.tsx`, `ui/src/app.css` | 6 | wiring and visible live state |
| docs, README, CLAUDE.md | 2, 3, 4, 7 | as listed per task |

---

### Task 1: `TranscriptTail` in the adapter

**Files:**
- Create: `crates/adapter-claude-code/src/tail.rs`
- Modify: `crates/adapter-claude-code/src/lib.rs` (add `mod tail;` and `pub use tail::{TailRead, TranscriptTail};`)

**Interfaces:**
- Consumes: `AdapterError` from `lib.rs` (variant `Io { path: PathBuf, source: std::io::Error }`).
- Produces:
  ```rust
  pub enum TailRead { Lines(Vec<(u64, String)>), Rewritten, Missing }
  pub struct TranscriptTail { .. }
  impl TranscriptTail {
      pub fn new(path: &Path) -> Self;
      pub fn path(&self) -> &Path;
      pub fn read_new(&mut self) -> Result<TailRead, AdapterError>;
      pub fn take_final_line(&mut self) -> Option<(u64, String)>;
  }
  ```

- [ ] **Step 1: Write the failing tests** at the bottom of the new `tail.rs` (the file needs only the test module for now plus `use super::*;`, and `mod tail;` in `lib.rs`).

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "snitchcraft-tail-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join("t.jsonl")
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open");
        f.write_all(bytes).expect("write");
    }

    fn lines(read: TailRead) -> Vec<(u64, String)> {
        match read {
            TailRead::Lines(l) => l,
            other => panic!("expected lines, got {other:?}"),
        }
    }

    #[test]
    fn missing_file_is_reported() {
        let path = temp_file("missing");
        let mut tail = TranscriptTail::new(&path);
        assert_eq!(tail.read_new().expect("read"), TailRead::Missing);
    }

    #[test]
    fn returns_only_new_complete_lines() {
        let path = temp_file("new-lines");
        append(&path, b"{\"a\":1}\n{\"b\":");
        let mut tail = TranscriptTail::new(&path);
        assert_eq!(lines(tail.read_new().expect("read")), [(1, "{\"a\":1}".to_string())]);
        assert_eq!(lines(tail.read_new().expect("read")), []);
        append(&path, b"2}\r\n\n{\"c\":3}\n");
        assert_eq!(
            lines(tail.read_new().expect("read")),
            [
                (2, "{\"b\":2}".to_string()),
                (3, String::new()),
                (4, "{\"c\":3}".to_string()),
            ]
        );
    }

    #[test]
    fn splits_inside_a_utf8_character() {
        let path = temp_file("utf8");
        let line = "{\"t\":\"caf\u{e9}\"}\n".as_bytes().to_vec();
        // Cut between the two bytes of the e-acute.
        let cut = line.iter().position(|&b| b == 0xC3).expect("multibyte") + 1;
        append(&path, &line[..cut]);
        let mut tail = TranscriptTail::new(&path);
        assert_eq!(lines(tail.read_new().expect("read")), []);
        append(&path, &line[cut..]);
        assert_eq!(
            lines(tail.read_new().expect("read")),
            [(1, "{\"t\":\"caf\u{e9}\"}".to_string())]
        );
    }

    #[test]
    fn shorter_file_is_reported_as_rewritten() {
        let path = temp_file("rewritten");
        append(&path, b"{\"a\":1}\n{\"b\":2}\n");
        let mut tail = TranscriptTail::new(&path);
        lines(tail.read_new().expect("read"));
        std::fs::write(&path, b"{\"a\":1}\n").expect("rewrite");
        assert_eq!(tail.read_new().expect("read"), TailRead::Rewritten);
    }

    #[test]
    fn final_line_without_newline_is_taken_only_if_valid_json() {
        let path = temp_file("final");
        append(&path, b"{\"a\":1}\n{\"b\":2}");
        let mut tail = TranscriptTail::new(&path);
        assert_eq!(lines(tail.read_new().expect("read")), [(1, "{\"a\":1}".to_string())]);
        assert_eq!(tail.take_final_line(), Some((2, "{\"b\":2}".to_string())));

        let path = temp_file("final-partial");
        append(&path, b"{\"a\":1}\n{\"b\":");
        let mut tail = TranscriptTail::new(&path);
        lines(tail.read_new().expect("read"));
        assert_eq!(tail.take_final_line(), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p adapter-claude-code tail`
Expected: compile errors (`TranscriptTail`, `TailRead` not found).

- [ ] **Step 3: Implement `tail.rs`** above the test module.

```rust
//! Reads a transcript a piece at a time while Claude Code is still writing
//! it. Each read returns only the complete lines added since the last read.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::AdapterError;

/// What one read of a growing transcript found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailRead {
    /// Complete new lines: 1-based line number and text without the line
    /// ending. Empty if nothing new was finished. Blank lines are included
    /// so line numbers match the file.
    Lines(Vec<(u64, String)>),
    /// The file is now shorter than what was already read, so it was
    /// rewritten. The caller should start over.
    Rewritten,
    /// The file does not exist (yet, or any more).
    Missing,
}

/// Follows one transcript file. Bytes after the last newline are held back
/// until the line is finished, which also covers a write that ends in the
/// middle of a UTF-8 character.
#[derive(Debug)]
pub struct TranscriptTail {
    path: PathBuf,
    /// Bytes read from the file so far, including `pending`.
    offset: u64,
    /// Bytes after the last newline, not yet returned.
    pending: Vec<u8>,
    /// Number the next complete line gets.
    next_line: u64,
}

impl TranscriptTail {
    /// A tail that has read nothing yet.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            offset: 0,
            pending: Vec::new(),
            next_line: 1,
        }
    }

    /// The file this tail follows.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads whatever was added since the last call.
    pub fn read_new(&mut self) -> Result<TailRead, AdapterError> {
        // std on Windows opens files with read, write and delete sharing, so
        // this never blocks Claude Code from writing the transcript.
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TailRead::Missing);
            }
            Err(source) => return Err(self.io(source)),
        };
        let len = file.metadata().map_err(|e| self.io(e))?.len();
        if len < self.offset {
            return Ok(TailRead::Rewritten);
        }
        if len == self.offset {
            return Ok(TailRead::Lines(Vec::new()));
        }
        file.seek(SeekFrom::Start(self.offset))
            .map_err(|e| self.io(e))?;
        let mut buf = Vec::new();
        // Read only up to the length seen above, so a write landing during
        // the read is picked up next time rather than half now.
        file.take(len - self.offset)
            .read_to_end(&mut buf)
            .map_err(|e| self.io(e))?;
        self.offset += buf.len() as u64;
        self.pending.extend_from_slice(&buf);
        Ok(TailRead::Lines(self.drain_complete_lines()))
    }

    /// For a file that is known to be finished: the held-back last line, if
    /// it is valid JSON (a line that was written without a final newline).
    /// A partial line is dropped and logged as "not ready yet".
    pub fn take_final_line(&mut self) -> Option<(u64, String)> {
        let bytes = std::mem::take(&mut self.pending);
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_suffix('\r').unwrap_or(&text);
        if text.trim().is_empty() {
            return None;
        }
        if serde_json::from_str::<serde_json::Value>(text).is_err() {
            tracing::debug!(line = self.next_line, "last line incomplete, not ready yet");
            return None;
        }
        let line = (self.next_line, text.to_string());
        self.next_line += 1;
        Some(line)
    }

    fn drain_complete_lines(&mut self) -> Vec<(u64, String)> {
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + pos;
            let mut line = &self.pending[start..end];
            if let Some(stripped) = line.strip_suffix(b"\r") {
                line = stripped;
            }
            lines.push((self.next_line, String::from_utf8_lossy(line).into_owned()));
            self.next_line += 1;
            start = end + 1;
        }
        self.pending.drain(..start);
        lines
    }

    fn io(&self, source: std::io::Error) -> AdapterError {
        AdapterError::Io {
            path: self.path.clone(),
            source,
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p adapter-claude-code tail`
Expected: 5 tests pass.

- [ ] **Step 5: Run all checks** (fmt, clippy, `cargo test --workspace`). Fix anything they report.

- [ ] **Step 6: Commit**

```powershell
git add crates/adapter-claude-code/src/tail.rs crates/adapter-claude-code/src/lib.rs
git commit -m "Add TranscriptTail for reading a transcript as it grows" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `SessionFollower` and `load_session` on top of it

**Files:**
- Create: `crates/adapter-claude-code/src/follow.rs`
- Create: `crates/adapter-claude-code/tests/follow.rs`
- Modify: `crates/adapter-claude-code/src/lib.rs` (rewrite `load_session`, move `subagent_title` and `file_name` into `follow.rs`, delete `feed_file`, `complete_lines` and their tests since `tail.rs` now covers that)
- Modify: `docs/HARNESS-NOTES.md`, `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `TranscriptTail`, `TailRead` (Task 1); `Parser::{for_session, for_subagent, push_line, finish, subagents, take_skipped}`, `SubagentLink { agent_id, tool_node_id }`, `Session { events, skipped }`, `AdapterError` (existing).
- Produces:
  ```rust
  pub enum ReadMode { Live, Finished }
  pub enum PollOutcome { Changed(Session), Rewritten, Missing }
  pub struct SessionFollower { .. }
  impl SessionFollower {
      pub fn new(path: &Path) -> Result<Self, AdapterError>;
      pub fn main_path(&self) -> &Path;
      pub fn poll(&mut self, mode: ReadMode) -> Result<PollOutcome, AdapterError>;
  }
  ```
  Exported from `lib.rs`: `pub use follow::{PollOutcome, ReadMode, SessionFollower};`
  `load_session(path: &Path) -> Result<Session, AdapterError>` keeps its signature and behaviour.

Behaviour of `poll`:
1. Read new lines of the main transcript. `Missing` returns `PollOutcome::Missing`; `Rewritten` returns `PollOutcome::Rewritten`. In `Finished` mode, also feed `take_final_line()`.
2. Feed lines to the main parser. If any line was fed, or this is the first poll, append `parser.finish()` (re-sends the `Run` with its current end time).
3. Remember the id of every `ToolCall` event seen so far (any parser).
4. Find new subagent links, skipping agent ids already linked:
   - from `parser.subagents()` of every parser (result-based, existing behaviour);
   - from every `agent-<id>.meta.json` in `<session id>/subagents/` whose JSON has a string `toolUseId` such that `tool:<toolUseId>` is a known tool call id. Read with `std::fs::read_to_string`; a bad or partial file is skipped silently (retried next poll).
5. A new link whose `agent-<id>.jsonl` exists gets a subagent source now (`Parser::for_subagent` with the title from the meta file's `description`). Otherwise it waits and is checked on every later poll. In `Finished` mode a missing file is logged with `tracing::warn!` (as today) and dropped.
6. Read every subagent source like the main file (step 1 and 2 rules, but `Missing` for a subagent is ignored and `Rewritten` returns `PollOutcome::Rewritten` for the whole session).
7. Repeat 3 to 6 until a pass starts no new subagent source (subagents can start subagents).
8. Return `Changed(Session { events, skipped })` with main events first, then subagent events, and all parsers' newly skipped lines.

- [ ] **Step 1: Write the failing integration tests** in `crates/adapter-claude-code/tests/follow.rs`.

```rust
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
    let dir = std::env::temp_dir().join(format!(
        "snitchcraft-follow-{}-{name}",
        std::process::id()
    ));
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
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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
            fixture_dir().join(SESSION).join("subagents").join(format!("agent-{AGENT}.meta.json")),
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
            apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
        }
        apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
        assert_eq!(walk(&trace), expected, "seed {seed}");
    }
}

#[test]
fn subagent_file_appearing_later_is_picked_up() {
    let (main_path, subagents) = temp_session("later");
    append(&main_path, &fixture_main());
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
    let run_id = format!("run:agent-{AGENT}");
    assert!(trace.get(&run_id).is_none());

    append(&subagents.join(format!("agent-{AGENT}.jsonl")), &fixture_agent());
    apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
    assert!(trace.get(&run_id).is_some(), "subagent run appears once its file exists");
    assert!(trace.children(&run_id).count() > 0);
}

/// The fixture's tool result that links the subagent, as (line index, tool_use_id).
fn linking_result() -> (usize, String) {
    let text = String::from_utf8(fixture_main()).expect("utf8");
    for (i, line) in text.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if value.pointer("/toolUseResult/agentId").and_then(|v| v.as_str()) != Some(AGENT) {
            continue;
        }
        let blocks = value.pointer("/message/content").and_then(|c| c.as_array()).expect("content");
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
    append(&subagents.join(format!("agent-{AGENT}.jsonl")), &fixture_agent());
    let meta = serde_json::json!({ "description": "Invented task", "toolUseId": tool_use_id });
    append(&subagents.join(format!("agent-{AGENT}.meta.json")), meta.to_string().as_bytes());

    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
    let run = trace.get(&format!("run:agent-{AGENT}")).expect("linked from meta.json");
    assert_eq!(run.parent_id.as_deref(), Some(format!("tool:{tool_use_id}").as_str()));
    let Node::Run(r) = &run.node else { panic!("not a run") };
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
    append(&subagents.join(format!("agent-{AGENT}.jsonl")), &fixture_agent());
    let meta = serde_json::json!({ "description": "Invented task", "toolUseId": tool_use_id });
    append(&subagents.join(format!("agent-{AGENT}.meta.json")), meta.to_string().as_bytes());

    let mut follower = SessionFollower::new(&main_path).expect("follower");
    let mut trace = Trace::new();
    apply_all(&mut trace, changed(follower.poll(ReadMode::Live).expect("poll")));
    assert!(trace.get(&format!("run:agent-{AGENT}")).is_some(), "linked from meta.json");
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
    assert!(trace.len() > nodes_before, "the rest of the main transcript was read");
}

#[test]
fn rewritten_and_missing_main_file_are_reported() {
    let (main_path, _) = temp_session("rewrite");
    append(&main_path, &fixture_main());
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    changed(follower.poll(ReadMode::Live).expect("poll"));
    std::fs::write(&main_path, b"").expect("truncate");
    assert!(matches!(follower.poll(ReadMode::Live).expect("poll"), PollOutcome::Rewritten));

    std::fs::remove_file(&main_path).expect("delete");
    let mut follower = SessionFollower::new(&main_path).expect("follower");
    assert!(matches!(follower.poll(ReadMode::Live).expect("poll"), PollOutcome::Missing));
}
```

Note on `result_link_after_meta_link_does_not_duplicate`: a second parser would re-emit the same node ids, which the trace silently accepts as replacements, so the test checks the events of the second poll instead of the trace. The unit test `a_link_found_twice_is_started_once` in Step 3 pins the same rule from the inside.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p adapter-claude-code --test follow`
Expected: compile errors (`SessionFollower`, `ReadMode`, `PollOutcome` not found).

- [ ] **Step 3: Implement `follow.rs`.**

```rust
//! Follows one session while Claude Code writes it: the main transcript plus
//! every subagent transcript linked from it. Each poll returns only the new
//! events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use trace_core::{Node, TraceEvent};

use crate::parser::{Parser, SubagentLink};
use crate::tail::{TailRead, TranscriptTail};
use crate::{AdapterError, Session};

/// How to treat a last line that has no newline yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadMode {
    /// The file may still be written: hold the line back.
    Live,
    /// The file is finished: use the line if it is valid JSON.
    Finished,
}

/// What one poll found.
#[derive(Debug)]
pub enum PollOutcome {
    /// New events and newly skipped lines since the last poll. Both may be
    /// empty when nothing changed.
    Changed(Session),
    /// A transcript got shorter, so it was rewritten. Start over with a new
    /// follower.
    Rewritten,
    /// The main transcript does not exist.
    Missing,
}

/// One transcript file and its parser.
struct Source {
    parser: Parser,
    tail: TranscriptTail,
    polled: bool,
}

/// Follows a main session transcript and its subagents.
pub struct SessionFollower {
    main: Source,
    subagent_dir: PathBuf,
    subagents: Vec<Source>,
    /// Agent ids that have a source or are waiting for their file.
    linked: HashSet<String>,
    /// Linked subagents whose transcript does not exist yet.
    waiting: Vec<SubagentLink>,
    /// Trace ids of every tool call seen, for linking from meta files.
    tool_ids: HashSet<String>,
}

impl SessionFollower {
    /// A follower for the main transcript at `path`. Reads nothing yet.
    pub fn new(path: &Path) -> Result<Self, AdapterError> {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AdapterError::NotATranscript(path.to_path_buf()))?;
        Ok(Self {
            main: Source {
                parser: Parser::for_session(&file_name(path), session_id),
                tail: TranscriptTail::new(path),
                polled: false,
            },
            subagent_dir: path.with_file_name(session_id).join("subagents"),
            subagents: Vec::new(),
            linked: HashSet::new(),
            waiting: Vec::new(),
            tool_ids: HashSet::new(),
        })
    }

    /// The main transcript's path.
    pub fn main_path(&self) -> &Path {
        self.main.tail.path()
    }

    /// Reads everything new in the session's files.
    pub fn poll(&mut self, mode: ReadMode) -> Result<PollOutcome, AdapterError> {
        let mut session = Session::default();
        match read_source(&mut self.main, mode, &mut session)? {
            TailRead::Missing => return Ok(PollOutcome::Missing),
            TailRead::Rewritten => return Ok(PollOutcome::Rewritten),
            TailRead::Lines(_) => {}
        }
        let mut checked = 0;
        loop {
            self.note_tool_calls(&session.events[checked..]);
            checked = session.events.len();
            let started = self.link_subagents(mode);
            for source in &mut self.subagents {
                if let TailRead::Rewritten = read_source(source, mode, &mut session)? {
                    return Ok(PollOutcome::Rewritten);
                }
            }
            if started == 0 && checked == session.events.len() {
                break;
            }
        }
        Ok(PollOutcome::Changed(session))
    }

    fn note_tool_calls(&mut self, events: &[TraceEvent]) {
        for event in events {
            if matches!(event.node, Node::ToolCall(_)) {
                self.tool_ids.insert(event.id.clone());
            }
        }
    }

    /// Finds new links and starts sources for those whose file exists.
    /// Returns how many sources were started.
    fn link_subagents(&mut self, mode: ReadMode) -> usize {
        let mut found: Vec<SubagentLink> = self.main.parser.subagents().to_vec();
        for source in &self.subagents {
            found.extend(source.parser.subagents().iter().cloned());
        }
        found.extend(self.meta_links());
        for link in found {
            if self.linked.insert(link.agent_id.clone()) {
                self.waiting.push(link);
            }
        }

        let mut started = 0;
        let mut still_waiting = Vec::new();
        for link in std::mem::take(&mut self.waiting) {
            let file = self.subagent_dir.join(format!("agent-{}.jsonl", link.agent_id));
            if file.is_file() {
                let title = subagent_title(&self.subagent_dir, &link.agent_id);
                let source = format!("subagents/{}", file_name(&file));
                self.subagents.push(Source {
                    parser: Parser::for_subagent(&source, &link, title),
                    tail: TranscriptTail::new(&file),
                    polled: false,
                });
                started += 1;
            } else if mode == ReadMode::Finished {
                tracing::warn!(agent_id = %link.agent_id, "linked subagent transcript not found");
            } else {
                still_waiting.push(link);
            }
        }
        self.waiting = still_waiting;
        started
    }

    /// Links from `agent-<id>.meta.json` files whose `toolUseId` names a
    /// tool call already seen. Lets a running subagent appear before its
    /// tool result arrives.
    fn meta_links(&self) -> Vec<SubagentLink> {
        let Ok(entries) = std::fs::read_dir(&self.subagent_dir) else {
            return Vec::new();
        };
        let mut links = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(agent_id) = name
                .to_str()
                .and_then(|n| n.strip_prefix("agent-"))
                .and_then(|n| n.strip_suffix(".meta.json"))
            else {
                continue;
            };
            if self.linked.contains(agent_id) {
                continue;
            }
            let Some(tool_use_id) = read_meta(&entry.path())
                .and_then(|m| m.get("toolUseId").and_then(|v| v.as_str()).map(str::to_string))
            else {
                continue;
            };
            let tool_node_id = format!("tool:{tool_use_id}");
            if self.tool_ids.contains(&tool_node_id) {
                links.push(SubagentLink {
                    agent_id: agent_id.to_string(),
                    tool_node_id,
                });
            }
        }
        links
    }
}

/// Reads one source's new lines into `session`. Returns what the tail
/// reported, with the lines already consumed.
fn read_source(
    source: &mut Source,
    mode: ReadMode,
    session: &mut Session,
) -> Result<TailRead, AdapterError> {
    let mut lines = match source.tail.read_new()? {
        TailRead::Lines(lines) => lines,
        other => return Ok(other),
    };
    if mode == ReadMode::Finished {
        lines.extend(source.tail.take_final_line());
    }
    let fed = !lines.is_empty();
    for (line_no, text) in &lines {
        session.events.extend(source.parser.push_line(*line_no, text));
    }
    if fed || !source.polled {
        session.events.extend(source.parser.finish());
    }
    source.polled = true;
    session.skipped.extend(source.parser.take_skipped());
    Ok(TailRead::Lines(Vec::new()))
}

fn read_meta(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The subagent's description from its `.meta.json` sidecar, if present.
fn subagent_title(dir: &Path, agent_id: &str) -> Option<String> {
    read_meta(&dir.join(format!("agent-{agent_id}.meta.json")))?
        .get("description")
        .and_then(|d| d.as_str())
        .map(str::to_string)
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
```

Check the loop in `poll` terminates: each pass either starts a source or reads new events; when a pass does neither it breaks. Subagent tails return no lines on a second read in the same poll, so the loop ends after at most one extra pass per nesting level.

Add a unit test at the bottom of `follow.rs` that pins "linked once":

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_found_twice_is_started_once() {
        let dir = std::env::temp_dir().join(format!("snitchcraft-follow-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("s").join("subagents")).expect("mkdir");
        std::fs::write(dir.join("s").join("subagents").join("agent-x.jsonl"), b"").expect("write");
        std::fs::write(dir.join("s.jsonl"), b"").expect("write");
        let mut follower = SessionFollower::new(&dir.join("s.jsonl")).expect("follower");
        let link = SubagentLink { agent_id: "x".into(), tool_node_id: "tool:t".into() };
        for _ in 0..2 {
            if follower.linked.insert(link.agent_id.clone()) {
                follower.waiting.push(link.clone());
            }
            follower.link_subagents(ReadMode::Live);
        }
        assert_eq!(follower.subagents.len(), 1);
    }
}
```

- [ ] **Step 4: Rewrite `load_session` in `lib.rs`** and remove the old helpers.

```rust
mod follow;
mod parser;
mod tail;
mod time;

pub use follow::{PollOutcome, ReadMode, SessionFollower};
pub use parser::{HARNESS, Parser, Skipped, SubagentLink};
pub use tail::{TailRead, TranscriptTail};

/// Loads a finished session: the main transcript at `path` plus every
/// subagent transcript linked to it (`<session id>/subagents/agent-<id>.jsonl`),
/// whether the link comes from a tool result or from the subagent's
/// `.meta.json`.
pub fn load_session(path: &Path) -> Result<Session, AdapterError> {
    let mut follower = SessionFollower::new(path)?;
    match follower.poll(ReadMode::Finished)? {
        PollOutcome::Changed(session) => Ok(session),
        PollOutcome::Missing => Err(AdapterError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }),
        // A first read starts at offset 0, so it cannot see a shorter file.
        PollOutcome::Rewritten => Ok(Session::default()),
    }
}
```

Delete `feed_file`, `complete_lines`, the old `subagent_title`, the old `file_name`, and the `complete_lines` tests in `lib.rs`. Keep `AdapterError` and `Session` as they are.

- [ ] **Step 5: Run all adapter tests**

Run: `cargo test -p adapter-claude-code`
Expected: all pass, including the existing `tests/basic_session.rs` unchanged (this proves `load_session` still behaves the same on the fixture) and the new `tests/follow.rs`.
If `follower_fed_in_chunks_matches_one_shot_load` fails, print the first differing node id and investigate; do not loosen the comparison.

- [ ] **Step 6: Run the rest of the workspace tests and the example**

Run: `cargo test --workspace`
Run: `cargo run -p adapter-claude-code --example print_tree -- fixtures/claude-code/basic/00000000-0000-4000-8000-000000000002.jsonl --stats`
Expected: tests pass (including `ui_mock_data_matches_the_commands` in `src-tauri`, which proves the diagram did not change); the example prints counts with no skipped lines.

- [ ] **Step 7: Update docs.**

Add to `docs/HARNESS-NOTES.md` under "Subagents from forked skills", or a new section "Subagent sidecar files":

```markdown
## Linking subagents early

- `agent-<id>.meta.json` for a subagent started with the Agent tool has `toolUseId`: the id of the `tool_use` block that started it. It also has `agentType`, `description` and `spawnDepth`, and for background agents `requestShape: "background"`.
- A background Agent call returns at once with `toolUseResult.status = "async_launched"` and the `agentId`. A foreground call only reports its `agentId` in the tool result once the subagent has finished.
- So while a session is running, the meta file is the only way to know which tool call a running subagent belongs to. Snitchcraft links a subagent from either source, whichever comes first. The meta files of forked skills seen so far have no `toolUseId`.
```

Add to `docs/DECISIONS.md` under Decisions:

```markdown
### 2026-10-05: Subagents are linked from their meta file as well as the tool result

A foreground Agent call only reports its subagent's id in the tool result when the subagent has finished, so a live view would not show a running subagent. The subagent's `.meta.json` names the starting tool call in `toolUseId`, so the adapter also links from there, as soon as that tool call is in the trace. Result-based linking is kept for subagents without that field (forked skills). This also applies to loading a finished session, so a subagent whose tool result is missing (for example an interrupted session) now shows up.

### 2026-10-05: Live reading holds back a last line with no newline

`TranscriptTail` only returns lines that end in a newline. Claude Code ends every line with one, so a line without it is still being written. Loading a finished file in one go (`load_session`) still accepts a valid JSON last line without a newline, as before.
```

- [ ] **Step 8: Run all checks** (fmt, clippy, `cargo test --workspace`).

- [ ] **Step 9: Commit**

```powershell
git add crates/adapter-claude-code docs/HARNESS-NOTES.md docs/DECISIONS.md
git commit -m "Follow a session and its subagents as the transcripts grow" -m "load_session now runs on the same follower, and subagents are also linked from their meta file so running ones can be shown." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `LiveSession` in the app, plus mock data for a growing session

**Files:**
- Create: `src-tauri/src/live.rs`
- Modify: `src-tauri/src/sessions.rs` (add `live` to `SessionSummary`, `live` and `version` to `SessionView`, factor out `modified_ms`, remove `SessionCache` and `view_of`, update tests and the mock snapshot test, add the live-steps snapshot)
- Modify: `src-tauri/src/main.rs` (replace `AppCache` with a temporary `Mutex<Option<LiveSession>>` so the app still builds; Task 4 replaces this again)
- Create (generated): `ui/src/mock/live-steps.json`
- Modify (generated): `ui/src/mock/fixture-data.json`
- Modify: `ui/src/types.ts` (only add the new fields so `npm run typecheck` passes: `live: boolean` on `SessionSummary`, `live: boolean` and `version: number` on `SessionView`)
- Modify: `docs/DECISIONS.md`, `CLAUDE.md` (the snapshot command now also refreshes `live-steps.json`)

**Interfaces:**
- Consumes: `SessionFollower`, `ReadMode`, `PollOutcome` (Task 2); `session_path`, `SessionError`, `SkippedLine`, `check_trace_id` (existing in `sessions.rs`); `trace_view::{build_session, node_detail, NodeDetail}`.
- Produces (in `src-tauri/src/live.rs`):
  ```rust
  pub const LIVE_WINDOW_MS: i64 = 10 * 60 * 1000;
  pub fn is_live(modified_ms: i64, now_ms: i64) -> bool;
  pub fn now_ms() -> i64;

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
  #[serde(rename_all = "snake_case")]
  pub enum LiveStatus { Watching, NoWatcher, Deleted }

  #[derive(Debug, Clone, PartialEq, Serialize)]
  #[serde(tag = "type", rename_all = "snake_case")]
  pub enum LiveMessage {
      Updated { project: String, session_id: String, view: SessionView, changed_trace_ids: Vec<String> },
      Status { project: String, session_id: String, status: LiveStatus },
  }

  pub struct LiveSession { .. }
  impl LiveSession {
      pub fn open(root: &Path, project: &str, session_id: &str) -> Result<Self, SessionError>;
      pub fn project(&self) -> &str;
      pub fn session_id(&self) -> &str;
      pub fn is(&self, project: &str, session_id: &str) -> bool;
      pub fn view(&self) -> SessionView;
      pub fn refresh(&mut self) -> Result<Option<LiveMessage>, SessionError>;
      pub fn node_detail(&self, trace_id: &str) -> Option<NodeDetail>;
  }
  ```
- Produces (in `sessions.rs`): `SessionSummary.live: bool`; `SessionView { diagram, skipped, live: bool, version: u64 }`; `pub fn modified_ms(meta: &std::fs::Metadata) -> i64`.

Behaviour of `LiveSession`:
- `open` validates names with `session_path` (keeps all M2 path checks), creates a `SessionFollower`, polls once with `ReadMode::Live`, applies events to a new `Trace` (a rejected event becomes a `SkippedLine` with reason `trace rejected the event: {err}`, exactly as `load_trace` does today), sets `version = 1`. `Missing` on open returns `SessionError::NotFound`. `Rewritten` on open cannot happen; treat it like an empty `Changed`.
- `view()` builds `SessionView { diagram: trace_view::build_session(&trace), skipped: clone, live: is_live(main file mtime, now_ms()), version }`. If the mtime cannot be read, `live` is false.
- `refresh()`:
  - `Changed` with no events and no skipped lines: return `Ok(None)` (but if `deleted` was set, clear it, since the file is back).
  - `Changed` with something: apply, increment `version`, collect the ids of applied events (in order, no duplicates) as `changed_trace_ids`, return `Updated`.
  - `Rewritten`: build a fresh follower and trace exactly like `open` (reuse a private helper), keep incrementing `version` (do not reset it), `changed_trace_ids` = every node id in the new trace, return `Updated`.
  - `Missing`: if not already reported, set `deleted = true` and return `Status { status: Deleted }`; otherwise `Ok(None)`.
  - Adapter I/O errors are returned as `Err(SessionError::Adapter(..))`; the caller logs and keeps the last view.
- `node_detail` calls `trace_view::node_detail(&self.trace, trace_id)`.

- [ ] **Step 1: Write failing tests** in a `#[cfg(test)] mod tests` at the bottom of `live.rs`. Use the fixture through a temp copy laid out as `<temp>/basic/<session>.jsonl` (a "projects root" containing one project `basic`).

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SESSION: &str = "00000000-0000-4000-8000-000000000002";

    fn fixture_main() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..").join("fixtures").join("claude-code").join("basic")
            .join(format!("{SESSION}.jsonl"));
        std::fs::read(path).expect("fixture")
    }

    /// A temp projects root with an empty `basic` project. Returns (root, main file).
    fn temp_root(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("snitchcraft-live-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("basic")).expect("mkdir");
        let main = root.join("basic").join(format!("{SESSION}.jsonl"));
        (root, main)
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("open");
        f.write_all(bytes).expect("write");
    }

    /// The fixture split into `parts` pieces at line boundaries.
    fn parts(parts: usize) -> Vec<Vec<u8>> {
        let text = String::from_utf8(fixture_main()).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        let size = lines.len().div_ceil(parts);
        lines.chunks(size).map(|c| c.iter().map(|l| format!("{l}\n")).collect::<String>().into_bytes()).collect()
    }

    #[test]
    fn live_window_is_ten_minutes() {
        assert!(is_live(1_000, 1_000 + LIVE_WINDOW_MS));
        assert!(!is_live(1_000, 1_001 + LIVE_WINDOW_MS));
        assert!(is_live(5_000, 1_000), "a time slightly in the future counts as live");
    }

    #[test]
    fn diagram_grows_as_lines_are_appended() {
        let (root, main) = temp_root("grows");
        let pieces = parts(4);
        append(&main, &pieces[0]);
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        let first = live.view();
        assert_eq!(first.version, 1);
        assert!(first.live, "just written");
        assert_eq!(live.refresh().expect("refresh"), None, "nothing new");

        let mut last_prompts = first.diagram.prompts.len();
        for piece in &pieces[1..] {
            append(&main, piece);
            let Some(LiveMessage::Updated { view, changed_trace_ids, .. }) = live.refresh().expect("refresh") else {
                panic!("expected an update");
            };
            assert!(!changed_trace_ids.is_empty());
            assert!(view.diagram.prompts.len() >= last_prompts);
            last_prompts = view.diagram.prompts.len();
        }
        let full = live.view();
        assert_eq!(full.version, 4);
        let one_shot = crate::sessions::load_trace(&root, "basic", SESSION).expect("load");
        assert_eq!(full.diagram, trace_view::build_session(&one_shot.trace));
    }

    #[test]
    fn rewritten_file_restarts_and_keeps_counting_versions() {
        let (root, main) = temp_root("rewrite");
        let pieces = parts(2);
        append(&main, &pieces[0]);
        append(&main, &pieces[1]);
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        std::fs::write(&main, &pieces[0]).expect("rewrite");
        let Some(LiveMessage::Updated { view, changed_trace_ids, .. }) = live.refresh().expect("refresh") else {
            panic!("expected an update");
        };
        assert_eq!(view.version, 2);
        assert!(changed_trace_ids.iter().any(|id| id.starts_with("run:")));
    }

    #[test]
    fn deleted_file_is_reported_once() {
        let (root, main) = temp_root("deleted");
        append(&main, &fixture_main());
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        std::fs::remove_file(&main).expect("delete");
        assert!(matches!(
            live.refresh().expect("refresh"),
            Some(LiveMessage::Status { status: LiveStatus::Deleted, .. })
        ));
        assert_eq!(live.refresh().expect("refresh"), None);
        assert!(live.node_detail(&format!("run:{SESSION}")).is_some(), "last trace is kept");
    }

    #[test]
    fn open_rejects_unsafe_names_and_missing_sessions() {
        let (root, _) = temp_root("names");
        assert!(matches!(LiveSession::open(&root, "..", SESSION), Err(SessionError::InvalidName { .. })));
        assert!(matches!(LiveSession::open(&root, "basic", "nope"), Err(SessionError::NotFound)));
    }

    #[test]
    fn messages_serialise_with_a_type_tag() {
        let msg = LiveMessage::Status { project: "p".into(), session_id: "s".into(), status: LiveStatus::NoWatcher };
        assert_eq!(
            serde_json::to_value(&msg).expect("json"),
            serde_json::json!({ "type": "status", "project": "p", "session_id": "s", "status": "no_watcher" })
        );
    }
}
```

`LiveMessage` derives `PartialEq` so `assert_eq!(.., None)` works (needs `SessionView: PartialEq`, which it already is).

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test -p snitchcraft live`
Expected: compile errors.

- [ ] **Step 3: Implement `live.rs`, the `sessions.rs` field changes, and the temporary `main.rs` wiring.**

In `sessions.rs`:
- Add `pub live: bool` (doc: "True if the file was written in the last 10 minutes, so the session may still be running.") to `SessionSummary`, set in `summarise` with `crate::live::is_live(modified_ms, crate::live::now_ms())`.
- Add `pub live: bool` and `pub version: u64` (doc: "Goes up by one with every update of the open session. The UI ignores a view with a lower version than the one it shows.") to `SessionView`.
- Extract `pub fn modified_ms(meta: &std::fs::Metadata) -> i64` from `summarise` (same expression, returning 0 on failure).
- Delete `SessionCache`, `CachedTrace`, `view_of`, `SessionError::CachePoisoned` stays (Task 4 uses it for its mutex). Keep `load_trace` (tests and `node_detail` fallback use it) and `LoadedTrace`.
- Update the existing `SessionCache` tests: `loads_fixture_session_as_diagram` and the `node_detail_*` tests become tests of `LiveSession::open(&fixture_root(), "basic", FIXTURE_SESSION)` + `view()` / `node_detail()`; delete `node_detail_uses_the_cached_trace` (no cache any more). Keep `errors_do_not_contain_paths`.

`now_ms()`:

```rust
/// The current time in milliseconds since the Unix epoch, or 0 if the clock
/// is before 1970.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// True if a file written at `modified_ms` counts as live at `now_ms`.
pub fn is_live(modified_ms: i64, now_ms: i64) -> bool {
    now_ms - modified_ms <= LIVE_WINDOW_MS
}
```

In `main.rs` for this task only: replace `AppCache` with `struct Open(Mutex<Option<LiveSession>>)`. `load_session` opens a `LiveSession`, stores it, returns `view()`. `node_detail` uses the stored session if `is(project, session_id)`, else opens one temporarily (do not store it) and asks it; always call `check_trace_id` first. A poisoned mutex maps to `SessionError::CachePoisoned`. No channels yet.

- [ ] **Step 4: Update the mock snapshot test and add the live-steps snapshot** in `sessions.rs` tests.

In `ui_mock_data_matches_the_commands`: build the view with `LiveSession::open(&root, "basic", FIXTURE_SESSION)`, then pin `view.live = false` (it depends on the checkout's file time) and keep `version` as `1`. Pin `s.live = false` for sessions as `modified_ms` is pinned. Details come from `live.node_detail(&id)`.

Add a second test writing `ui/src/mock/live-steps.json`, using the same `SNITCHCRAFT_UPDATE_SNAPSHOTS` switch and the same compare-or-write logic:

```rust
/// `ui/src/mock/live-steps.json` replays the fixture session growing in six
/// steps, as the backend reports it. The dev mock mode uses it to show live
/// updates in a browser.
#[test]
fn ui_live_steps_match_the_backend() {
    let root = temp_dir("live-steps");
    let project = root.join("basic");
    std::fs::create_dir_all(&project).expect("mkdir");
    let fixture = fixture_root().join("basic");
    // Subagent files are present from the start; they are linked once the
    // main transcript reaches the linking line.
    copy_dir(&fixture.join(FIXTURE_SESSION), &project.join(FIXTURE_SESSION));
    let text = std::fs::read_to_string(fixture.join(format!("{FIXTURE_SESSION}.jsonl"))).expect("read");
    let lines: Vec<&str> = text.lines().collect();
    let size = lines.len().div_ceil(6);
    let main = project.join(format!("{FIXTURE_SESSION}.jsonl"));
    let mut steps = Vec::new();
    let mut live: Option<LiveSession> = None;
    for chunk in lines.chunks(size) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&main).expect("open");
        for line in chunk {
            writeln!(f, "{line}").expect("write");
        }
        drop(f);
        let message = match live.as_mut() {
            None => {
                let opened = LiveSession::open(&root, "basic", FIXTURE_SESSION).expect("open");
                let view = opened.view();
                live = Some(opened);
                LiveMessage::Updated {
                    project: "basic".into(),
                    session_id: FIXTURE_SESSION.into(),
                    view,
                    changed_trace_ids: Vec::new(),
                }
            }
            Some(session) => session.refresh().expect("refresh").expect("an update"),
        };
        let mut value = serde_json::to_value(&message).expect("json");
        // Always live in the replay, whatever the temp file's time.
        value["view"]["live"] = serde_json::Value::Bool(true);
        steps.push(value);
    }
    // ...compare with or write ui/src/mock/live-steps.json, same as the other snapshot test.
}
```

Write a small `copy_dir(from: &Path, to: &Path)` test helper (recursive `std::fs::copy`). Factor the compare-or-write block shared by both snapshot tests into a helper `fn check_snapshot(path: &Path, expected: &serde_json::Value)` so the logic is not duplicated.

- [ ] **Step 5: Regenerate the mock data and run the tests**

```powershell
$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS
cargo test --workspace
```

Expected: all pass. Review `git diff --stat ui/src/mock/`: `fixture-data.json` should change only by the added `live` and `version` fields; `live-steps.json` is new with six steps whose `view.version` goes 1 to 6. Open `live-steps.json` and check it contains no machine paths or usernames (search it for `Users` and `\\`; expect no hits other than the fixture's invented `C:\\work\\example`).

- [ ] **Step 6: Add the new fields to `ui/src/types.ts`** (`live: boolean` on `SessionSummary`; `live: boolean` and `version: number` on `SessionView`, each with a one-line comment), then run the UI checks: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.

- [ ] **Step 7: Docs.**

Add to `docs/DECISIONS.md`:

```markdown
### 2026-10-05: The backend sends a full diagram on every update (chosen by the project owner)

While a session runs, the backend keeps its parsers open and reads only new lines, but after each change it rebuilds the whole diagram model and sends it to the UI. This keeps all diagram logic in Rust and the UI a plain renderer. Diagrams are a few kilobytes, so resending is cheap. Revisit this and send only the changes if diagrams grow large enough for updates to feel slow, or when the M5 harness streams events directly.

### 2026-10-05: A session is live if written in the last 10 minutes (chosen by the project owner)

Claude Code writes no "session ended" line and any session can be resumed, so the transcript's last write time is the only signal. 10 minutes covers normal pauses while the user reads or types. The flag only drives the live markers; the open session is watched for as long as it is selected.

### 2026-10-05: Views carry a version number

Each `SessionView` has a `version` that goes up with every update of the open session. A live update can reach the UI before the reply to `load_session`, so the UI keeps whichever view has the higher version.
```

In `CLAUDE.md`, change the snapshot command line to say it refreshes `ui/src/mock/fixture-data.json` and `ui/src/mock/live-steps.json`, and review the diff of both.

- [ ] **Step 8: Run all checks** (fmt, clippy, workspace tests, UI checks).

- [ ] **Step 9: Commit**

```powershell
git add src-tauri ui/src/types.ts ui/src/mock docs/DECISIONS.md CLAUDE.md
git commit -m "Keep the open session live in the backend and mark live sessions" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Watcher thread, channels and permissions

**Files:**
- Create: `src-tauri/src/watch.rs`
- Modify: `src-tauri/src/main.rs`, `src-tauri/Cargo.toml`, `Cargo.lock`, `src-tauri/build.rs`, `src-tauri/capabilities/default.json`
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `LiveSession`, `LiveMessage`, `LiveStatus` (Task 3); `SessionError::CachePoisoned`; `list_sessions` (existing).
- Produces (in `watch.rs`):
  ```rust
  pub trait Sink<T>: Send { fn send(&self, message: T) -> bool; }
  impl<T: tauri::ipc::IpcResponse> Sink<T> for tauri::ipc::Channel<T>;

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
  pub struct SessionsChanged {}

  pub struct Active { pub session: LiveSession, pub sink: Box<dyn Sink<LiveMessage>> }

  #[derive(Default)]
  pub struct Shared {
      pub active: Mutex<Option<Active>>,
      pub list_sink: Mutex<Option<Box<dyn Sink<SessionsChanged>>>>,
      pub watcher_ok: AtomicBool,
  }

  pub const BATCH: Duration = Duration::from_millis(250);
  pub const POLL: Duration = Duration::from_secs(1);
  pub const LIST_GAP: Duration = Duration::from_secs(2);

  pub struct Scheduler { .. }
  pub struct Due { pub refresh_session: bool, pub list_changed: bool }
  impl Scheduler {
      pub fn new(now: Instant) -> Self;
      pub fn file_changed(&mut self, touches_session: bool);
      pub fn due(&mut self, now: Instant) -> Due;
  }

  pub fn touches_session(root: &Path, path: &Path, project: &str, session_id: &str) -> bool;
  pub fn spawn(root: PathBuf, shared: Arc<Shared>) -> std::io::Result<std::thread::JoinHandle<()>>;
  ```
- Commands after this task:
  - `list_sessions() -> Vec<SessionSummary>` (unchanged)
  - `load_session(project, session_id, on_update: Channel<LiveMessage>) -> SessionView`
  - `node_detail(project, session_id, trace_id) -> Option<NodeDetail>`
  - `watch_sessions(on_change: Channel<SessionsChanged>) -> ()`

Worker behaviour:
- `spawn` creates `std::sync::mpsc::channel::<notify::Result<notify::Event>>()`, tries `notify::recommended_watcher(move |res| { let _ = tx.send(res); })` and `watcher.watch(&root, RecursiveMode::Recursive)`. On success sets `watcher_ok = true`; on failure logs `tracing::warn!` and leaves it false (the sender is dropped with the failed watcher). Then starts a named thread (`std::thread::Builder::new().name("snitchcraft-watch".into())`) that owns the watcher (keep it alive in a local) and runs the loop.
- Loop:
  1. `rx.recv_timeout(BATCH)`: an event is handled; `Timeout` does nothing; `Disconnected` means no watcher, so `std::thread::sleep(BATCH)`.
  2. Drain with `rx.try_recv()` until empty, handling each event.
  3. `let due = scheduler.due(Instant::now())`; if `refresh_session`, refresh the active session; if `list_changed`, send `SessionsChanged {}` to the list sink (drop the sink if `send` returns false).
- Handling an event: ignore `Err` (log at debug) and `EventKind::Access(_)`. For each path, `touches_session(root, path, active.project(), active.session_id())` (lock `active` briefly; if there is no active session it is false). Call `scheduler.file_changed(touches)` once per event with `touches` = any path touched.
- Refreshing: lock `active`; if `Some`, call `session.refresh()`. `Ok(Some(msg))` is sent to the sink; if `send` returns false, log at debug and leave the session in place (the UI may have reloaded; `node_detail` still works). `Err(e)` is logged with `tracing::warn!` and the last view kept. A poisoned lock is logged and the loop continues.
- `touches_session` strips `root` from `path` (`Path::strip_prefix`; return false on failure), takes the first two components as strings, and returns true if the first is `project` and the second is either `<session_id>.jsonl` or `<session_id>` (the folder holding subagents). Comparison is exact (case-sensitive), matching how the names came from `list_sessions`.
- `Scheduler`: `file_changed` sets `list_dirty = true`, and `session_dirty = true` if it touches the session. `due(now)`: `refresh_session = session_dirty || now - last_poll >= POLL`; when true, clear `session_dirty` and set `last_poll = now`. `list_changed = list_dirty && now - last_list >= LIST_GAP`; when true, clear `list_dirty` and set `last_list = now`. Use `now.saturating_duration_since(..)`.

Commands:
- `load_session`: inside `run_blocking`, open the `LiveSession`, take its `view()`, lock `shared.active` and store `Active { session, sink: Box::new(on_update.clone()) }`, then send `LiveMessage::Status { status: Watching or NoWatcher }` (from `watcher_ok`) through the channel. Return the view.
- `node_detail`: `check_trace_id` first; lock `active`; if it `is(project, session_id)`, answer from it; otherwise release the lock and open a temporary `LiveSession` to answer (not stored).
- `watch_sessions`: lock `list_sink`, store `Box::new(on_change)`. Returns `Ok(())`.
- Remove the temporary `Open` state from Task 3. Manage `Arc<Shared>` as Tauri state. In `setup`, after resolving `projects_root`, call `watch::spawn(root, Arc::clone(&shared))` when the root is known; log an error if `spawn` fails (the app still works).

- [ ] **Step 1: Add the dependency**

In `src-tauri/Cargo.toml` under `[dependencies]`: `notify = "8"`. Run `cargo build -p snitchcraft` to update `Cargo.lock`. (notify 8.2 is the current stable release; 9.0 is still a release candidate.)

- [ ] **Step 2: Write failing tests** in `#[cfg(test)] mod tests` in `watch.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    impl<T: Send> Sink<T> for mpsc::Sender<T> {
        fn send(&self, message: T) -> bool {
            mpsc::Sender::send(self, message).is_ok()
        }
    }

    #[test]
    fn touches_session_matches_relative_names() {
        let root = Path::new("projects");
        let s = "00000000-0000-4000-8000-000000000002";
        assert!(touches_session(root, &root.join("basic").join(format!("{s}.jsonl")), "basic", s));
        assert!(touches_session(
            root,
            &root.join("basic").join(s).join("subagents").join("agent-1.jsonl"),
            "basic",
            s
        ));
        assert!(!touches_session(root, &root.join("other").join(format!("{s}.jsonl")), "basic", s));
        assert!(!touches_session(root, &root.join("basic").join("another.jsonl"), "basic", s));
        assert!(!touches_session(root, Path::new("elsewhere").join("basic").as_path(), "basic", s));
    }

    #[test]
    fn scheduler_batches_and_polls() {
        let start = Instant::now();
        let mut s = Scheduler::new(start);
        let none = s.due(start);
        assert!(!none.refresh_session && !none.list_changed);

        s.file_changed(true);
        let due = s.due(start + Duration::from_millis(10));
        assert!(due.refresh_session, "a change to the session refreshes it at once");
        assert!(!due.list_changed, "list changes wait for the gap");

        let due = s.due(start + LIST_GAP);
        assert!(due.list_changed);
        assert!(due.refresh_session, "polled again after a second");

        let due = s.due(start + LIST_GAP + Duration::from_millis(10));
        assert!(!due.refresh_session && !due.list_changed);
    }

    #[test]
    fn other_files_change_the_list_but_do_not_force_a_refresh() {
        let start = Instant::now();
        let mut s = Scheduler::new(start);
        s.file_changed(false);
        let due = s.due(start + Duration::from_millis(10));
        assert!(!due.refresh_session);
        assert!(s.due(start + LIST_GAP).list_changed);
    }

    #[test]
    fn worker_sends_updates_when_the_open_file_grows() {
        let root = std::env::temp_dir().join(format!("snitchcraft-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("basic")).expect("mkdir");
        let session = "00000000-0000-4000-8000-000000000002";
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..").join("fixtures").join("claude-code").join("basic")
            .join(format!("{session}.jsonl"));
        let text = std::fs::read_to_string(fixture).expect("fixture");
        let lines: Vec<&str> = text.lines().collect();
        let main = root.join("basic").join(format!("{session}.jsonl"));
        let half = lines.len() / 2;
        std::fs::write(&main, lines[..half].iter().map(|l| format!("{l}\n")).collect::<String>()).expect("write");

        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel::<LiveMessage>();
        let (list_tx, list_rx) = mpsc::channel::<SessionsChanged>();
        {
            let live = LiveSession::open(&root, "basic", session).expect("open");
            *shared.active.lock().expect("lock") = Some(Active { session: live, sink: Box::new(tx) });
            *shared.list_sink.lock().expect("lock") = Some(Box::new(list_tx));
        }
        let _worker = spawn(root.clone(), Arc::clone(&shared)).expect("spawn");
        assert!(shared.watcher_ok.load(Ordering::SeqCst), "watcher starts on a temp folder");

        let mut f = std::fs::OpenOptions::new().append(true).open(&main).expect("open");
        for line in &lines[half..] {
            writeln!(f, "{line}").expect("write");
        }
        drop(f);

        let message = rx.recv_timeout(Duration::from_secs(5)).expect("an update within 5 seconds");
        assert!(matches!(message, LiveMessage::Updated { .. }));
        list_rx.recv_timeout(Duration::from_secs(5)).expect("a list change within 5 seconds");
    }
}
```

The worker test needs `use std::io::Write;` and `std::sync::atomic::Ordering` in scope. The worker thread is never stopped; that is fine for a test process. (If the test is flaky on CI because of timing, raise the timeout, do not remove the test.)

- [ ] **Step 3: Run to verify the tests fail**

Run: `cargo test -p snitchcraft watch`
Expected: compile errors.

- [ ] **Step 4: Implement `watch.rs` and the command changes** as described above. The `Channel` sink:

```rust
impl<T: tauri::ipc::IpcResponse> Sink<T> for tauri::ipc::Channel<T> {
    fn send(&self, message: T) -> bool {
        tauri::ipc::Channel::send(self, message).is_ok()
    }
}
```

Check the current Tauri docs (https://v2.tauri.app/develop/calling-frontend/#channels) for the exact `Channel` API before writing this, and adjust if `send`'s signature differs.

- [ ] **Step 5: Permissions**

`src-tauri/build.rs`: add `"watch_sessions"` to the command list.
`src-tauri/capabilities/default.json`: add `"allow-watch-sessions"` and update the description to: "The main window may list sessions, watch the list for changes, load one (with live updates over a channel), and read the details of its nodes. Nothing else: no file system, shell, network or other plugin access."

- [ ] **Step 6: Run the tests and checks**

Run: `cargo test -p snitchcraft` then all checks.
Expected: pass. Also run `cargo build -p snitchcraft` and confirm no warning about permissions.

- [ ] **Step 7: Docs.** Add `notify` to the dependency table in `docs/DECISIONS.md`:

`| \`notify\` 8 | snitchcraft | Watches the projects folder for transcript changes (M3). Named in CLAUDE.md. Version 8 is the latest stable; 9 is a release candidate. No debouncer crate: the worker batches events itself. |`

Add decisions:

```markdown
### 2026-10-05: Live updates use channels, not events

Tauri events would need the window to be granted the core event permissions to listen. A channel passed as a command argument needs none. `load_session` takes a channel for the open session's updates and `watch_sessions` takes one for session list changes, so the only new window permission is `allow-watch-sessions`.

### 2026-10-05: One watcher thread, plus polling as a safety net

A single recursive `notify` watcher on the projects folder covers new sessions, growing sessions and new subagent files. On Windows a change notice for a file another process keeps open can arrive late, so the open session is also polled once a second. Polling only reads the open session's new bytes and the list of its subagent files, so it costs almost nothing. If the watcher cannot start (for example the projects folder does not exist yet), polling still keeps the open session live and the UI says the session list needs a manual refresh. The worker is a plain thread rather than a tokio task, since it only waits on a channel with a timeout.

### 2026-10-05: The session list change signal is sent at most every 2 seconds

`list_sessions` reads every transcript to find titles, so the UI should not re-run it on every write. The signal carries no data; the UI calls `list_sessions` again.
```

- [ ] **Step 8: Commit**

```powershell
git add src-tauri Cargo.lock docs/DECISIONS.md
git commit -m "Watch the projects folder and push live updates over channels" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: UI types, API and live-update logic

**Files:**
- Modify: `ui/src/types.ts`, `ui/src/api.ts`, `ui/src/mock/mockApi.ts`
- Create: `ui/src/live.ts`, `ui/src/live.test.ts`

**Interfaces:**
- Consumes: the Rust shapes from Tasks 3 and 4 (field names exactly as serialised), `ui/src/mock/live-steps.json` (an array of `LiveMessage` of type `updated`).
- Produces:
  ```ts
  // types.ts
  export type LiveStatus = 'watching' | 'no_watcher' | 'deleted'
  export type LiveMessage =
    | { type: 'updated'; project: string; session_id: string; view: SessionView; changed_trace_ids: string[] }
    | { type: 'status'; project: string; session_id: string; status: LiveStatus }

  // api.ts
  export interface Api {
    listSessions(): Promise<SessionSummary[]>
    loadSession(project: string, sessionId: string, onMessage: (message: LiveMessage) => void): Promise<SessionView>
    nodeDetail(project: string, sessionId: string, traceId: string): Promise<NodeDetail | null>
    watchSessions(onChange: () => void): Promise<void>
  }

  // live.ts
  export function isNewer(current: SessionView | null, incoming: SessionView): boolean
  export function isForSession(message: LiveMessage, session: SessionSummary | null): boolean
  export function findNode(diagram: SessionDiagram, id: string): DiagramNode | null
  export function needsDetailRefetch(node: DiagramNode, changed: readonly string[]): boolean
  export function newPromptIndexes(previous: SessionDiagram | null, next: SessionDiagram): number[]
  export function statusMessage(status: LiveStatus | null): string | null
  ```

- [ ] **Step 1: Add the types** to `types.ts` (with a comment that they mirror `LiveMessage` and `LiveStatus` in `src-tauri/src/live.rs`).

- [ ] **Step 2: Write the failing tests** in `ui/src/live.test.ts`.

```ts
// Tests for the live-update logic. Run with `npm test`.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import {
  findNode,
  isForSession,
  isNewer,
  needsDetailRefetch,
  newPromptIndexes,
  statusMessage,
} from './live.ts'
import type { DiagramNode, LiveMessage, SessionSummary, SessionView } from './types.ts'

function steps(): LiveMessage[] {
  const url = new URL('./mock/live-steps.json', import.meta.url)
  return JSON.parse(readFileSync(url, 'utf8')) as LiveMessage[]
}

function viewAt(i: number): SessionView {
  const step = steps()[i]
  assert.ok(step && step.type === 'updated')
  return step.view
}

const session: SessionSummary = {
  project: 'basic',
  project_label: 'basic',
  session_id: '00000000-0000-4000-8000-000000000002',
  title: null,
  modified_ms: 0,
  size_bytes: 0,
  live: true,
}

test('isNewer_rejects_older_versions', () => {
  const v1 = viewAt(0)
  const v2 = viewAt(1)
  assert.equal(isNewer(null, v1), true)
  assert.equal(isNewer(v1, v2), true)
  assert.equal(isNewer(v2, v1), false)
  assert.equal(isNewer(v2, v2), false)
})

test('isForSession matches project and session id', () => {
  const msg: LiveMessage = { type: 'status', project: 'basic', session_id: session.session_id, status: 'watching' }
  assert.equal(isForSession(msg, session), true)
  assert.equal(isForSession({ ...msg, session_id: 'other' }, session), false)
  assert.equal(isForSession({ ...msg, project: 'other' }, session), false)
  assert.equal(isForSession(msg, null), false)
})

test('findNode finds prompt descendants and markers, and returns null when gone', () => {
  const last = viewAt(steps().length - 1).diagram
  const prompt = last.prompts[0]
  assert.ok(prompt)
  const deep = (function deepest(n: DiagramNode): DiagramNode {
    const child = n.children[n.children.length - 1]
    return child ? deepest(child) : n
  })(prompt.root)
  assert.equal(findNode(last, deep.id)?.id, deep.id)
  const marker = last.markers[0]
  if (marker) assert.equal(findNode(last, marker.id)?.id, marker.id)
  assert.equal(findNode(last, 'no-such-id'), null)
})

test('needsDetailRefetch only when one of the node trace ids changed', () => {
  const node = { trace_ids: ['tool:a', 'tool:b'] } as DiagramNode
  assert.equal(needsDetailRefetch(node, ['tool:b']), true)
  assert.equal(needsDetailRefetch(node, ['tool:c']), false)
  assert.equal(needsDetailRefetch(node, []), false)
})

test('newPromptIndexes lists prompts added since the previous diagram', () => {
  const all = steps().map((s) => (s.type === 'updated' ? s.view.diagram : null))
  const first = all[0]
  const last = all[all.length - 1]
  assert.ok(first && last)
  assert.deepEqual(newPromptIndexes(null, last), [])
  const added = newPromptIndexes(first, last)
  assert.deepEqual(
    added,
    last.prompts.slice(first.prompts.length).map((p) => p.index),
  )
  assert.ok(added.length > 0, 'the replay adds prompts')
})

test('statusMessage explains problems and is silent when watching', () => {
  assert.equal(statusMessage(null), null)
  assert.equal(statusMessage('watching'), null)
  assert.match(statusMessage('no_watcher') ?? '', /Refresh/)
  assert.match(statusMessage('deleted') ?? '', /no longer exists/)
})

test('live steps grow and keep increasing versions', () => {
  const versions = steps().map((s) => (s.type === 'updated' ? s.view.version : -1))
  assert.deepEqual(versions, [...versions].sort((a, b) => a - b))
  assert.equal(new Set(versions).size, versions.length)
})
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd ui; npm test`
Expected: fails to import `./live.ts`.

- [ ] **Step 4: Implement `live.ts`.**

```ts
// Pure logic for applying live updates from the backend. Kept free of React
// so it can be unit tested.

import type { DiagramNode, LiveMessage, LiveStatus, SessionDiagram, SessionSummary, SessionView } from './types.ts'

/** True if `incoming` should replace `current` (it has a higher version). */
export function isNewer(current: SessionView | null, incoming: SessionView): boolean {
  return current === null || incoming.version > current.version
}

/** True if a live message belongs to the session being shown. */
export function isForSession(message: LiveMessage, session: SessionSummary | null): boolean {
  return session !== null && message.project === session.project && message.session_id === session.session_id
}

/** The box with this diagram id in the new diagram, or null if it is gone. */
export function findNode(diagram: SessionDiagram, id: string): DiagramNode | null {
  const search = (nodes: readonly DiagramNode[]): DiagramNode | null => {
    for (const node of nodes) {
      if (node.id === id) return node
      const found = search(node.children)
      if (found) return found
    }
    return null
  }
  return search(diagram.markers) ?? search(diagram.prompts.map((p) => p.root))
}

/** True if any trace node behind this box changed in the update. */
export function needsDetailRefetch(node: DiagramNode, changed: readonly string[]): boolean {
  return node.trace_ids.some((id) => changed.includes(id))
}

/** Indexes of prompts that are in `next` but were not in `previous`. */
export function newPromptIndexes(previous: SessionDiagram | null, next: SessionDiagram): number[] {
  if (previous === null) return []
  const known = new Set(previous.prompts.map((p) => p.turn_id))
  return next.prompts.filter((p) => !known.has(p.turn_id)).map((p) => p.index)
}

/** A message for the header, or null when there is nothing to say. */
export function statusMessage(status: LiveStatus | null): string | null {
  switch (status) {
    case 'no_watcher':
      return 'File watcher unavailable: this session still updates, but the session list needs Refresh.'
    case 'deleted':
      return 'The transcript file no longer exists. Showing the last version read.'
    default:
      return null
  }
}
```

- [ ] **Step 5: Update `api.ts`.**

```ts
import { Channel, invoke } from '@tauri-apps/api/core'

import type { LiveMessage, NodeDetail, SessionSummary, SessionView } from './types.ts'

export interface Api {
  listSessions(): Promise<SessionSummary[]>
  /** Loads a session and keeps it live: later changes arrive through `onMessage`. */
  loadSession(project: string, sessionId: string, onMessage: (message: LiveMessage) => void): Promise<SessionView>
  nodeDetail(project: string, sessionId: string, traceId: string): Promise<NodeDetail | null>
  /** Calls `onChange` whenever the session list may have changed. */
  watchSessions(onChange: () => void): Promise<void>
}

const tauriApi: Api = {
  listSessions: () => invoke<SessionSummary[]>('list_sessions'),
  loadSession: (project, sessionId, onMessage) => {
    const channel = new Channel<LiveMessage>()
    channel.onmessage = onMessage
    return invoke<SessionView>('load_session', { project, session_id: sessionId, on_update: channel })
  },
  nodeDetail: (project, sessionId, traceId) =>
    invoke<NodeDetail | null>('node_detail', { project, session_id: sessionId, trace_id: traceId }),
  watchSessions: (onChange) => {
    const channel = new Channel<Record<string, never>>()
    channel.onmessage = () => onChange()
    return invoke<void>('watch_sessions', { on_change: channel })
  },
}
```

Update the file's top comment: "The backend commands. In the app they go through Tauri's `invoke`, and live updates arrive over Tauri channels."

- [ ] **Step 6: Update `mockApi.ts`** with a `live` URL flag.

Add to the header comment:
```
//   live         replay the session growing (ui/src/mock/live-steps.json),
//                one step every `step` ms (default 1500)
```

Implementation outline (write it fully):
- `import steps from './live-steps.json'` cast to `LiveMessage[]`.
- `const live = params.has('live')`, `const stepMs = Number(params.get('step') ?? '1500')`.
- Keep `let timer: ReturnType<typeof setInterval> | null = null` in the closure.
- `listSessions`: as today, plus `live: live && i === 0`.
- `loadSession(project, sessionId, onMessage)`: clear any existing `timer`. If not `live`, return the stored view as today (with `onMessage({ type: 'status', project, session_id: sessionId, status: 'watching' })` after the wait). If `live` and the key matches the fixture session: send status `watching`, return `steps[0].view`, and start `timer = setInterval(...)` that sends `steps[1]`, `steps[2]`, ... through `onMessage` and clears itself after the last step. If `fail=live` is set, after the replay send `{ type: 'status', status: 'deleted' }` so the deleted message can be seen.
- `nodeDetail`: unchanged (details of the full session cover every id in the steps).
- `watchSessions(onChange)`: resolve after `wait()`; in `live` mode also call `onChange()` once every 3 steps while the replay runs (use a counter inside the same interval).

- [ ] **Step 7: Update the existing caller** in `App.tsx` minimally so the app compiles: pass `() => {}` as `onMessage` to `api.loadSession` (Task 6 wires it properly). Run `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.
Expected: all pass, including the new `live.test.ts`.

- [ ] **Step 8: Commit**

```powershell
git add ui/src
git commit -m "Add the live update API and logic to the UI" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Show live updates in the UI

**Files:**
- Modify: `ui/src/App.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/components/DetailsPanel.tsx`, `ui/src/app.css`

**Interfaces:**
- Consumes: `Api.loadSession(.., onMessage)`, `Api.watchSessions`, everything in `live.ts` (Task 5).
- Produces: no new exports. `DetailsPanel` gains a prop `refreshKey: number`. `SessionPanel` gains props `liveStatus: LiveStatus | null` and `newPrompts: ReadonlySet<number>`. `SessionList` shows a live dot from `SessionSummary.live`.

Required behaviour (from the spec):
1. **Applying updates.** In `openSession`, pass an `onMessage` handler. It ignores the message unless `token === loadToken.current` and `isForSession(message, s)`. For `updated`: replace the view only if `isNewer(currentView, message.view)` (keep the current view in a ref, `viewRef`, so the handler reads the latest without re-creating callbacks). The `load_session` reply goes through the same `isNewer` check. Do not touch `openState` (it is keyed by diagram id, so expanded boxes carry over by themselves). Keep `promptIndex` unless it is now out of range.
2. **Selection.** After an update, `setSelected((sel) => sel ? findNode(view.diagram, sel.id) : null)`. If `sel` was not null and `findNode` returns null, the details panel closes.
3. **Details refetch.** Keep a `detailRefresh` counter in state. When an update arrives and the selected node (after step 2) passes `needsDetailRefetch(node, message.changed_trace_ids)`, increment it. Pass it to `DetailsPanel` as `refreshKey`. In `DetailsPanel`, change the effect's dependency list from `[api, project, sessionId, node, key]` to `[api, project, sessionId, node.id, traceIdsKey, refreshKey, key]` where `const traceIdsKey = node.trace_ids.join('|')`, and read `node.trace_ids` via that key, so a new node object with the same content does not refetch. Fix any lint complaint about missing deps by deriving the ids from `traceIdsKey.split('|')` inside the effect (guarding the empty string case).
4. **New prompt highlight.** Compute `newPromptIndexes(previousDiagram, nextDiagram)` on each update, add them to a `newPrompts` set state, and remove each index again after 4 seconds (`setTimeout`; clear timers on session change). `SessionPanel` adds class `item-new` to those prompt buttons. CSS: a short background fade using `--running-bg` (`@keyframes` from that background to transparent over 4s). Respect `prefers-reduced-motion: reduce` by using a static outline instead of the animation.
5. **Live status.** Keep `liveStatus: LiveStatus | null` in state, reset to null on `openSession`. A `status` message sets it. Show a "Live" badge in the session panel heading when `view.value.live` is true and `liveStatus !== 'deleted'` (class `live-badge`: small pill using `--ok` text on `--ok-bg`, with a dot). Show `statusMessage(liveStatus)` as a `notice notice-warning` under the session title when not null.
6. **Session list.** On startup, after `getApi()` resolves, call `chosen.watchSessions(() => listSessions(chosen))` and log failures with `console.warn` (the manual Refresh button still works). `listSessions` must not flip the list to `loading` on these background refreshes (it does not today; keep it that way). Also update `session` (the open `SessionSummary`) from the new list when its entry is present, so its `live` flag and title stay current; compare by project and session id.
7. **Live dot in the list.** In `SessionList`, when `s.live`, render `<span className="live-dot" title="Written in the last 10 minutes" aria-label="live" />` before the title. CSS: 8px circle in `--ok`.
8. Keep the `Diagram` `key` as it is (`project/session/turn_id`), so the view does not reset on updates.

- [ ] **Step 1: Implement the changes above.** Keep `App.tsx` readable: put the message handler in a `useCallback` named `handleLive` that takes `(token: number, s: SessionSummary, message: LiveMessage)`.

- [ ] **Step 2: Run the UI checks**

Run: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`
Expected: all pass.

- [ ] **Step 3: Check it in a browser with the mock.**

Run `cd ui; npm run dev`, open `http://localhost:5173/?mock&live`, and confirm by looking at the page (use the browser automation tools if available, otherwise report that this step was not done):
- the first session has a live dot;
- opening it shows a "Live" badge and the prompt list grows every 1.5 s, with new prompts briefly highlighted;
- expand a box, select a node, and confirm both survive the next steps;
- `?mock&live&fail=live` ends with the "no longer exists" notice.
Report exactly what was seen.

- [ ] **Step 4: Commit**

```powershell
git add ui/src
git commit -m "Show live sessions and update the open diagram as it grows" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: End-to-end check against a real session, and docs (done by the coordinator, not a subagent)

This task needs the real app window and a real Claude Code session, so the main session does it.

**Files:**
- Modify: `docs/HARNESS-NOTES.md`, `README.md`, `CLAUDE.md`

- [ ] **Step 1:** Run all checks from the Global Constraints on the finished branch.
- [ ] **Step 2:** Run `cargo tauri dev`. Confirm the window opens and the session list loads with live dots on recent sessions.
- [ ] **Step 3:** Start a short Claude Code session in another PowerShell window (for example in a temp folder, asking it to list files and read one). Open it in Snitchcraft and confirm the diagram grows while it runs, the sidebar picks the new session up without Refresh, and clicking a node during the run shows details. Note how quickly updates arrive.
- [ ] **Step 4:** Ask the session to start a subagent (Agent tool) and confirm the subagent box appears while it is still running (meta-file linking).
- [ ] **Step 5:** While watching, record what was learned about how Claude Code writes live in `docs/HARNESS-NOTES.md` (for example: whether lines arrive one write at a time, when `meta.json` appears relative to the subagent's first line, whether notices arrived late and polling mattered). Use only observations, and nothing private: no real paths, prompts or project names.
- [ ] **Step 6:** Update `README.md` (milestone status: M3 done; a short "Live updates" paragraph; the `?mock&live` dev flag) and `CLAUDE.md` (current milestone M3 complete, M4 next; commands list includes `?mock&live`).
- [ ] **Step 7:** Commit: `git commit -m "Document M3 live watching and mark it complete"` with the co-author line.
