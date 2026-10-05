//! Keeps the open session's trace up to date as its transcripts grow.
//! Everything here is read-only: the follower only opens files for reading.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use adapter_claude_code::{PollOutcome, ReadMode, Session, SessionFollower, session_key};
use capture_core::CaptureRecord;
use serde::Serialize;
use trace_core::Trace;
use trace_view::NodeDetail;

use crate::captures::CaptureIndex;
use crate::sessions::{SessionError, SessionView, SkippedLine, session_path};

/// A session counts as live if its transcript was written this recently.
pub const LIVE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// True if a file written at `modified_ms` counts as live at `now_ms`.
pub fn is_live(modified_ms: i64, now_ms: i64) -> bool {
    now_ms - modified_ms <= LIVE_WINDOW_MS
}

/// The current time in milliseconds since the Unix epoch, or 0 if the clock
/// is before 1970.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The state of watching the open session, for the UI header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveStatus {
    /// File changes are being watched.
    Watching,
    /// The file watcher could not start. The open session is still polled,
    /// but the session list only changes on Refresh.
    NoWatcher,
    /// The session's main transcript was deleted. The last view is kept.
    Deleted,
}

/// A message about the open session, sent from the backend to the UI.
// `Updated` is much larger than `Status`, but messages are built one at a
// time and sent straight away, so boxing the view would gain nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveMessage {
    /// The session changed. `view` is the whole new view.
    Updated {
        /// The project folder name.
        project: String,
        /// The session id.
        session_id: String,
        /// The full view after the change.
        view: SessionView,
        /// Ids of the trace nodes added or changed by this update, in the
        /// order they were applied, without duplicates.
        changed_trace_ids: Vec<String>,
    },
    /// The watching state changed.
    Status {
        /// The project folder name.
        project: String,
        /// The session id.
        session_id: String,
        /// The new state.
        status: LiveStatus,
    },
}

/// One open session: a follower that reads only new lines, and the trace
/// built from everything read so far.
pub struct LiveSession {
    project: String,
    session_id: String,
    follower: SessionFollower,
    trace: Trace,
    skipped: Vec<SkippedLine>,
    version: u64,
    /// True once a missing main transcript has been reported.
    deleted: bool,
    /// The `live` flag of the last view handed out, so a change in it alone
    /// (the session going quiet) can be sent as an update.
    sent_live: bool,
    /// Which model calls have a captured API call.
    captures: CaptureIndex,
    /// The session's captured records the index was built from, kept so the
    /// index can be rebuilt when new trace nodes arrive. Shared with the
    /// record cache, so holding it costs no copy.
    capture_records: Arc<Vec<CaptureRecord>>,
}

impl LiveSession {
    /// Validates the names (the same checks as `session_path`), reads the
    /// session as it is now, and returns it with version 1.
    pub fn open(root: &Path, project: &str, session_id: &str) -> Result<Self, SessionError> {
        let path = session_path(root, project, session_id)?;
        let (follower, trace, skipped) = start(&path)?;
        Ok(Self {
            project: project.to_string(),
            session_id: session_id.to_string(),
            follower,
            trace,
            skipped,
            version: 1,
            deleted: false,
            sent_live: false,
            captures: CaptureIndex::default(),
            capture_records: Arc::new(Vec::new()),
        })
    }

    /// The project folder name.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// True if this is the session with these names.
    pub fn is(&self, project: &str, session_id: &str) -> bool {
        self.project == project && self.session_id == session_id
    }

    /// True if the session was reported deleted and has not come back.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// The current view. `live` comes from the main transcript's modified
    /// time and is false if that cannot be read. Remembers the `live` flag
    /// it hands out, so `refresh` can tell when it changes.
    pub fn view(&mut self) -> SessionView {
        let live = self.current_live();
        self.sent_live = live;
        SessionView {
            diagram: trace_view::build_session(&self.trace),
            skipped: self.skipped.clone(),
            live,
            version: self.version,
            captured_trace_ids: self.captures.captured_trace_ids(),
        }
    }

    /// Keeps the session's captured records and rebuilds the capture index
    /// from them.
    pub fn set_captures(&mut self, records: Arc<Vec<CaptureRecord>>) {
        self.captures = CaptureIndex::build(&self.trace, &records);
        self.capture_records = records;
    }

    /// Replaces the session's captured records (as `set_captures`). If any
    /// model call gained or lost a capture, bumps the version and returns an
    /// `Updated` message listing those calls in trace order; otherwise
    /// returns `None` (the diagram looks the same).
    pub fn refresh_captures(&mut self, records: Arc<Vec<CaptureRecord>>) -> Option<LiveMessage> {
        let before: HashSet<String> = self.captures.captured_trace_ids().into_iter().collect();
        self.set_captures(records);
        let after: HashSet<String> = self.captures.captured_trace_ids().into_iter().collect();
        let changed: Vec<String> = all_trace_ids(&self.trace)
            .into_iter()
            .filter(|id| before.contains(id) != after.contains(id))
            .collect();
        if changed.is_empty() {
            return None;
        }
        Some(self.updated(changed))
    }

    /// The capture store key of this session, if its id is a usable one.
    pub fn capture_key(&self) -> Option<String> {
        session_key(Some(&self.session_id))
    }

    /// The trace built so far.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// Rebuilds the capture index after the trace changed, since a capture
    /// may have been saved before its model call reached the transcript.
    /// Adds the model calls that became captured to `changed`.
    fn reindex_captures(&mut self, changed: &mut Vec<String>) {
        let before: HashSet<String> = self.captures.captured_trace_ids().into_iter().collect();
        self.captures = CaptureIndex::build(&self.trace, &self.capture_records);
        for id in self.captures.captured_trace_ids() {
            if !before.contains(&id) && !changed.contains(&id) {
                changed.push(id);
            }
        }
    }

    /// Reads whatever was written since the last call. Returns `None` if
    /// nothing changed, `Updated` with the whole new view after new lines, a
    /// rewritten or recreated file, or a change in the `live` flag alone
    /// (then with no changed ids), and `Status { Deleted }` once when the
    /// main transcript disappears. On an error the last trace is kept.
    pub fn refresh(&mut self) -> Result<Option<LiveMessage>, SessionError> {
        if self.deleted {
            // A file that comes back is a new file: reading on from the old
            // offset would be wrong, so start over.
            return self.restart("transcript is back, reading it again");
        }
        match self.follower.poll(ReadMode::Live)? {
            PollOutcome::Changed(session) => {
                if session.events.is_empty() && session.skipped.is_empty() {
                    if self.current_live() != self.sent_live {
                        return Ok(Some(self.updated(Vec::new())));
                    }
                    return Ok(None);
                }
                let mut changed = apply(&mut self.trace, &mut self.skipped, session);
                self.reindex_captures(&mut changed);
                Ok(Some(self.updated(changed)))
            }
            PollOutcome::Rewritten => self.restart("transcript was rewritten, reading it again"),
            PollOutcome::Missing => Ok(self.missing()),
        }
    }

    /// Reads the main transcript again from the start with a new follower,
    /// and returns an update listing every node. Reports the file as
    /// missing if it is not there.
    fn restart(&mut self, why: &str) -> Result<Option<LiveMessage>, SessionError> {
        let path = self.follower.main_path().to_path_buf();
        let (follower, trace, skipped) = match start(&path) {
            Ok(started) => started,
            Err(SessionError::NotFound) => return Ok(self.missing()),
            Err(err) => return Err(err),
        };
        tracing::info!("{why}");
        self.follower = follower;
        self.trace = trace;
        self.skipped = skipped;
        self.deleted = false;
        let mut changed = all_trace_ids(&self.trace);
        self.reindex_captures(&mut changed);
        Ok(Some(self.updated(changed)))
    }

    /// Whether the main transcript was written within the live window.
    /// False if its modified time cannot be read.
    fn current_live(&self) -> bool {
        std::fs::metadata(self.follower.main_path())
            .map(|meta| is_live(crate::sessions::modified_ms(&meta), now_ms()))
            .unwrap_or(false)
    }

    /// Full content of one trace node, or `None` if there is no such node.
    pub fn node_detail(&self, trace_id: &str) -> Option<NodeDetail> {
        trace_view::node_detail(&self.trace, trace_id)
    }

    /// Bumps the version and builds an `Updated` message.
    fn updated(&mut self, changed_trace_ids: Vec<String>) -> LiveMessage {
        self.version += 1;
        LiveMessage::Updated {
            project: self.project.clone(),
            session_id: self.session_id.clone(),
            view: self.view(),
            changed_trace_ids,
        }
    }

    /// A `Deleted` status the first time the file is missing, then `None`.
    fn missing(&mut self) -> Option<LiveMessage> {
        if self.deleted {
            return None;
        }
        self.deleted = true;
        tracing::info!("the open session's transcript was deleted");
        Some(LiveMessage::Status {
            project: self.project.clone(),
            session_id: self.session_id.clone(),
            status: LiveStatus::Deleted,
        })
    }
}

/// A new follower and the trace built from its first read.
fn start(path: &Path) -> Result<(SessionFollower, Trace, Vec<SkippedLine>), SessionError> {
    let mut follower = SessionFollower::new(path)?;
    let session = match follower.poll(ReadMode::Live)? {
        PollOutcome::Changed(session) => session,
        PollOutcome::Missing => return Err(SessionError::NotFound),
        // A first read starts at offset 0, so it cannot see a shorter file.
        PollOutcome::Rewritten => Session::default(),
    };
    let mut trace = Trace::new();
    let mut skipped = Vec::new();
    apply(&mut trace, &mut skipped, session);
    Ok((follower, trace, skipped))
}

/// Applies the adapter's events and skipped lines. An event the trace
/// rejects (an adapter bug) becomes a skipped line. Returns the ids of the
/// applied events, in order, without duplicates.
pub fn apply(trace: &mut Trace, skipped: &mut Vec<SkippedLine>, session: Session) -> Vec<String> {
    skipped.extend(session.skipped.into_iter().map(|s| SkippedLine {
        source: s.source,
        line: s.line,
        reason: s.reason,
    }));
    let mut changed: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for event in session.events {
        let (source, line) = event
            .raw
            .first()
            .map(|r| (r.source.clone(), r.line))
            .unwrap_or_default();
        let id = event.id.clone();
        match trace.apply(event) {
            Ok(()) => {
                if seen.insert(id.clone()) {
                    changed.push(id);
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "trace rejected an event");
                skipped.push(SkippedLine {
                    source,
                    line,
                    reason: format!("trace rejected the event: {err}"),
                });
            }
        }
    }
    changed
}

/// Every node id in the trace, depth first.
pub fn all_trace_ids(trace: &Trace) -> Vec<String> {
    fn walk(trace: &Trace, id: &str, out: &mut Vec<String>) {
        out.push(id.to_string());
        for child in trace.children(id) {
            walk(trace, &child.id, out);
        }
    }
    let mut out = Vec::new();
    for root in trace.roots() {
        walk(trace, &root.id, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    const SESSION: &str = "00000000-0000-4000-8000-000000000002";

    fn fixture_main() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
            .join("basic")
            .join(format!("{SESSION}.jsonl"));
        std::fs::read(path).expect("fixture")
    }

    /// A temp projects root with an empty `basic` project. Returns (root, main file).
    fn temp_root(name: &str) -> (PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("snitchcraft-live-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("basic")).expect("mkdir");
        let main = root.join("basic").join(format!("{SESSION}.jsonl"));
        (root, main)
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open");
        f.write_all(bytes).expect("write");
    }

    /// The fixture split into `parts` pieces at line boundaries.
    fn parts(parts: usize) -> Vec<Vec<u8>> {
        let text = String::from_utf8(fixture_main()).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        let size = lines.len().div_ceil(parts);
        lines
            .chunks(size)
            .map(|c| {
                c.iter()
                    .map(|l| format!("{l}\n"))
                    .collect::<String>()
                    .into_bytes()
            })
            .collect()
    }

    #[test]
    fn live_window_is_ten_minutes() {
        assert!(is_live(1_000, 1_000 + LIVE_WINDOW_MS));
        assert!(!is_live(1_000, 1_001 + LIVE_WINDOW_MS));
        assert!(
            is_live(5_000, 1_000),
            "a time slightly in the future counts as live"
        );
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
            let Some(LiveMessage::Updated {
                view,
                changed_trace_ids,
                ..
            }) = live.refresh().expect("refresh")
            else {
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
        assert_eq!(full.skipped, one_shot.skipped);
    }

    #[test]
    fn rewritten_file_restarts_and_keeps_counting_versions() {
        let (root, main) = temp_root("rewrite");
        let pieces = parts(2);
        append(&main, &pieces[0]);
        append(&main, &pieces[1]);
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        std::fs::write(&main, &pieces[0]).expect("rewrite");
        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh().expect("refresh")
        else {
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
            Some(LiveMessage::Status {
                status: LiveStatus::Deleted,
                ..
            })
        ));
        assert_eq!(live.refresh().expect("refresh"), None);
        assert!(
            live.node_detail(&format!("run:{SESSION}")).is_some(),
            "last trace is kept"
        );
    }

    #[test]
    fn recreated_file_is_read_from_the_start() {
        let (root, main) = temp_root("recreated");
        let pieces = parts(2);
        append(&main, &pieces[0]);
        append(&main, &pieces[1]);
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        std::fs::remove_file(&main).expect("delete");
        assert!(matches!(
            live.refresh().expect("refresh"),
            Some(LiveMessage::Status {
                status: LiveStatus::Deleted,
                ..
            })
        ));
        assert!(live.is_deleted());
        assert_eq!(live.refresh().expect("refresh"), None, "still gone");

        // Recreated with different (shorter) content.
        append(&main, &pieces[0]);
        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh().expect("refresh")
        else {
            panic!("expected an update for the recreated file");
        };
        assert!(!live.is_deleted());
        assert!(changed_trace_ids.iter().any(|id| id.starts_with("run:")));
        let expected = {
            let (other_root, other_main) = temp_root("recreated-expected");
            append(&other_main, &pieces[0]);
            LiveSession::open(&other_root, "basic", SESSION)
                .expect("open")
                .view()
        };
        assert_eq!(view.diagram, expected.diagram);
        assert_eq!(view.skipped, expected.skipped);
        assert_eq!(live.refresh().expect("refresh"), None);
    }

    /// Sets the file's modified time to `age` ago.
    fn age_file(path: &Path, age: std::time::Duration) {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open");
        f.set_modified(std::time::SystemTime::now() - age)
            .expect("set mtime");
    }

    #[test]
    fn live_flag_changes_are_sent_without_new_lines() {
        let (root, main) = temp_root("live-flag");
        append(&main, &fixture_main());
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        assert!(live.view().live, "fresh file");
        assert_eq!(live.refresh().expect("refresh"), None);

        age_file(&main, std::time::Duration::from_secs(11 * 60));
        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh().expect("refresh")
        else {
            panic!("expected an update when the session goes quiet");
        };
        assert!(!view.live);
        assert!(changed_trace_ids.is_empty());
        assert_eq!(view.version, 2);
        assert_eq!(live.refresh().expect("refresh"), None, "sent once");

        age_file(&main, std::time::Duration::ZERO);
        let Some(LiveMessage::Updated { view, .. }) = live.refresh().expect("refresh") else {
            panic!("expected an update when the file is touched again");
        };
        assert!(view.live);
        assert_eq!(view.version, 3);
    }

    #[test]
    fn an_old_session_stays_quiet() {
        let (root, main) = temp_root("old");
        append(&main, &fixture_main());
        age_file(&main, std::time::Duration::from_secs(60 * 60));
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        assert!(!live.view().live);
        assert_eq!(live.refresh().expect("refresh"), None);
    }

    #[test]
    fn open_rejects_unsafe_names_and_missing_sessions() {
        let (root, _) = temp_root("names");
        assert!(matches!(
            LiveSession::open(&root, "..", SESSION),
            Err(SessionError::InvalidName { .. })
        ));
        assert!(matches!(
            LiveSession::open(&root, "basic", "nope"),
            Err(SessionError::NotFound)
        ));
    }

    #[test]
    fn captures_mark_model_calls_in_the_view() {
        let (root, main) = temp_root("captures");
        append(&main, &fixture_main());
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        assert!(live.view().captured_trace_ids.is_empty(), "none set yet");
        assert_eq!(live.capture_key().as_deref(), Some(SESSION));

        live.set_captures(Arc::new(crate::captures::fixture_records()));
        let mut ids = live.view().captured_trace_ids;
        ids.sort();
        // Only the main transcript is in this temp folder, so the subagent
        // captures and the title call match no model call.
        let mut calls: Vec<String> = trace_view::model_calls_by_run(&live.trace)
            .into_iter()
            .flat_map(|(_, c)| c)
            .collect();
        calls.sort();
        assert!(!calls.is_empty());
        assert_eq!(ids, calls);
    }

    #[test]
    fn a_capture_saved_before_its_call_is_marked_when_the_call_arrives() {
        let (root, main) = temp_root("captures-early");
        let pieces = parts(4);
        append(&main, &pieces[0]);
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        live.set_captures(Arc::new(crate::captures::fixture_records()));
        let early = live.view().captured_trace_ids;

        for piece in &pieces[1..] {
            append(&main, piece);
        }
        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh().expect("refresh")
        else {
            panic!("expected an update");
        };
        let late: Vec<&String> = view
            .captured_trace_ids
            .iter()
            .filter(|id| !early.contains(id))
            .collect();
        assert!(!late.is_empty(), "some calls only arrive with later lines");
        for id in late {
            assert!(changed_trace_ids.contains(id), "{id} is listed as changed");
        }
        let mut unique = changed_trace_ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), changed_trace_ids.len(), "no duplicates");
    }

    #[test]
    fn refresh_captures_lists_calls_that_gained_or_lost_a_capture() {
        let (root, main) = temp_root("captures-refresh");
        append(&main, &fixture_main());
        let mut live = LiveSession::open(&root, "basic", SESSION).expect("open");
        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh_captures(Arc::new(crate::captures::fixture_records()))
        else {
            panic!("expected an update");
        };
        assert_eq!(view.version, 2);
        let mut marked = view.captured_trace_ids.clone();
        marked.sort();
        let mut changed = changed_trace_ids.clone();
        changed.sort();
        assert!(!marked.is_empty());
        assert_eq!(changed, marked, "every newly captured call");
        assert_eq!(
            live.refresh_captures(Arc::new(crate::captures::fixture_records())),
            None,
            "nothing changed, nothing sent"
        );

        let Some(LiveMessage::Updated {
            view,
            changed_trace_ids,
            ..
        }) = live.refresh_captures(Arc::new(Vec::new()))
        else {
            panic!("expected an update");
        };
        assert_eq!(view.version, 3);
        assert!(view.captured_trace_ids.is_empty());
        let mut changed = changed_trace_ids;
        changed.sort();
        assert_eq!(changed, marked, "every call that lost its capture");
    }

    #[test]
    fn messages_serialise_with_a_type_tag() {
        let msg = LiveMessage::Status {
            project: "p".into(),
            session_id: "s".into(),
            status: LiveStatus::NoWatcher,
        };
        assert_eq!(
            serde_json::to_value(&msg).expect("json"),
            serde_json::json!({ "type": "status", "project": "p", "session_id": "s", "status": "no_watcher" })
        );
    }
}
