//! Finding and loading Claude Code session files. Everything here is
//! read-only and takes the projects folder as a parameter, so tests can point
//! it at a fixture folder instead of the real home directory.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
#[cfg(test)]
use trace_core::Trace;
use trace_view::SessionDiagram;

/// File extension of a session transcript.
const TRANSCRIPT_EXT: &str = "jsonl";

/// Errors returned to the UI. The UI shows the message as text.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The home directory could not be found at startup.
    #[error("could not find the home folder")]
    NoHome,
    /// A project or session name was not a single plain file name.
    #[error("invalid {what}: must be a plain name with no path separators")]
    InvalidName {
        /// Which input was rejected: "project" or "session id".
        what: &'static str,
    },
    /// The requested session file does not exist.
    #[error("session not found")]
    NotFound,
    /// The resolved file is not inside the projects folder.
    #[error("session is outside the projects folder")]
    OutsideRoot,
    /// Reading a folder or file failed.
    #[error("could not read {what}: {source}")]
    Io {
        /// What was being read, without any machine path.
        what: &'static str,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The adapter could not read the transcript.
    #[error("could not load session: {0}")]
    Adapter(#[from] adapter_claude_code::AdapterError),
    /// The capture store failed. Its message names no machine path.
    #[error("{0}")]
    Captures(#[from] capture::StoreError),
    /// The app's captures folder could not be found at startup, so capture
    /// is off.
    #[error("captures are unavailable: the app data folder could not be found")]
    CapturesOff,
    /// A capture id was empty or too long to be one.
    #[error("invalid capture id")]
    InvalidCaptureId,
    /// A trace node id was empty or too long to be one.
    #[error("invalid trace id")]
    InvalidTraceId,
}

/// One session transcript found under the projects folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionSummary {
    /// The project folder name, exactly as on disk. Claude Code encodes the
    /// project's path into this name. Pass it back to `load_session`.
    pub project: String,
    /// A short label for the project: the last part of the encoded name.
    pub project_label: String,
    /// The session id, which is the transcript file name without `.jsonl`.
    pub session_id: String,
    /// The latest title Claude Code wrote for the session, if any.
    pub title: Option<String>,
    /// When the file was last modified, in milliseconds since the Unix epoch.
    pub modified_ms: i64,
    /// File size in bytes.
    pub size_bytes: u64,
    /// True if the file was written in the last 10 minutes, so the session
    /// may still be running.
    pub live: bool,
}

/// A line the adapter could not use, in a form the UI can receive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedLine {
    /// Source file name, relative to the session (for example
    /// `subagents/agent-1.jsonl`).
    pub source: String,
    /// 1-based line number.
    pub line: u64,
    /// Why the line was skipped.
    pub reason: String,
}

/// A session, ready for the UI to draw.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    /// The diagram model built by `trace_view::build_session`. See
    /// `docs/VIEW-MODEL.md`.
    pub diagram: SessionDiagram,
    /// Lines that could not be used, so the UI can say so.
    pub skipped: Vec<SkippedLine>,
    /// True if the main transcript was written in the last 10 minutes, so
    /// the session may still be running.
    pub live: bool,
    /// Goes up by one with every update of the open session. The UI ignores
    /// a view with a lower version than the one it shows.
    pub version: u64,
    /// Model call trace ids that have a captured API call.
    pub captured_trace_ids: Vec<String>,
}

/// A session read from disk and turned into a trace. Only the tests use it
/// now, to compare live reading with loading the whole file at once.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct LoadedTrace {
    /// The trace built from the adapter's events.
    pub trace: Trace,
    /// Lines the adapter skipped, plus any event the trace rejected.
    pub skipped: Vec<SkippedLine>,
}

/// Longest trace id `node_detail` accepts. Real ids are far shorter; this
/// only stops the UI from sending something absurd.
const MAX_TRACE_ID_LEN: usize = 512;

/// The Claude Code projects folder inside a home directory.
pub fn projects_root(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

/// Session titles from earlier listings, so a listing only rereads the
/// transcripts that changed. A transcript is read again when its size or
/// modified time differs from when its title was read.
#[derive(Default)]
pub struct TitleCache {
    entries: Mutex<HashMap<PathBuf, CachedTitle>>,
    /// How many transcripts were read for their title, for tests.
    reads: AtomicUsize,
}

/// One cached title and the file state it was read from.
struct CachedTitle {
    size: u64,
    modified: Option<SystemTime>,
    title: Option<String>,
}

impl TitleCache {
    /// The title of the transcript at `path`, read from the file only if
    /// its size or modified time changed since the last read.
    fn title(&self, path: &Path, meta: &std::fs::Metadata) -> Option<String> {
        let size = meta.len();
        let modified = meta.modified().ok();
        if let Some(cached) = self.lock().get(path) {
            if cached.size == size && cached.modified == modified {
                return cached.title.clone();
            }
        }
        // Read without holding the lock.
        self.reads.fetch_add(1, Ordering::Relaxed);
        let title = read_title(path);
        self.lock().insert(
            path.to_path_buf(),
            CachedTitle {
                size,
                modified,
                title: title.clone(),
            },
        );
        title
    }

    /// Forgets transcripts that were not in the latest listing.
    fn keep_only(&self, seen: &HashSet<PathBuf>) {
        self.lock().retain(|path, _| seen.contains(path));
    }

    /// The map, recovered if a thread panicked while holding it: at worst
    /// a title is read again.
    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, CachedTitle>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many transcripts were read for their title so far.
    #[cfg(test)]
    fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }
}

/// Lists sessions like `list_sessions_cached`, reading every title afresh.
#[cfg(test)]
pub fn list_sessions(root: &Path) -> Result<Vec<SessionSummary>, SessionError> {
    list_sessions_cached(root, &TitleCache::default())
}

/// Lists every session transcript directly inside each project folder,
/// newest first. Subagent transcripts (in `<session id>/subagents/`) are not
/// listed, since they belong to their parent session. A missing projects
/// folder gives an empty list. Titles come from `cache` when the transcript
/// has not changed since it was last read.
pub fn list_sessions_cached(
    root: &Path,
    cache: &TitleCache,
) -> Result<Vec<SessionSummary>, SessionError> {
    let projects = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("projects folder does not exist yet");
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(SessionError::Io {
                what: "the projects folder",
                source,
            });
        }
    };

    let mut sessions = Vec::new();
    let mut seen = HashSet::new();
    for project in projects.flatten() {
        let is_dir = project.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let Some(project_name) = project.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_dir {
            continue;
        }
        let entries = match std::fs::read_dir(project.path()) {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!(error = %err, "could not read a project folder");
                continue;
            }
        };
        for entry in entries.flatten() {
            if let Some(summary) = summarise(&project_name, &entry, cache) {
                seen.insert(entry.path());
                sessions.push(summary);
            }
        }
    }
    cache.keep_only(&seen);
    sessions.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.project.cmp(&b.project))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    Ok(sessions)
}

/// Builds a summary for one directory entry, or `None` if it is not a
/// session transcript.
fn summarise(
    project: &str,
    entry: &std::fs::DirEntry,
    cache: &TitleCache,
) -> Option<SessionSummary> {
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()) != Some(TRANSCRIPT_EXT) {
        return None;
    }
    let meta = entry.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let session_id = path.file_stem()?.to_str()?.to_string();
    let modified_ms = modified_ms(&meta);
    Some(SessionSummary {
        project: project.to_string(),
        project_label: project_label(project),
        session_id,
        title: cache.title(&path, &meta),
        modified_ms,
        size_bytes: meta.len(),
        live: crate::live::is_live(modified_ms, crate::live::now_ms()),
    })
}

/// When a file was last modified, in milliseconds since the Unix epoch, or 0
/// if that cannot be read.
pub fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// A short label for an encoded project folder name: the text after the last
/// `-`. Claude Code turns path separators and some other characters into
/// `-`, so the original name cannot be recovered exactly. For `C--work-demo`
/// the label is `demo`; for `C--work-my-app` it is only `app`. Names without
/// a `-` are returned unchanged.
pub fn project_label(project: &str) -> String {
    match project.rsplit('-').next() {
        Some(last) if !last.is_empty() => last.to_string(),
        _ => project.to_string(),
    }
}

/// The `aiTitle` of the last `ai-title` line in a transcript, if any.
/// Only lines that mention `ai-title` are parsed as JSON. Bad lines, an
/// unfinished last line, and read errors are ignored.
fn read_title(path: &Path) -> Option<String> {
    // std on Windows opens files with read, write and delete sharing, so
    // this never blocks Claude Code from writing the transcript.
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut title = None;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                tracing::debug!(error = %err, "stopped reading a transcript for its title");
                break;
            }
        }
        if !contains(&buf, b"\"ai-title\"") {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&buf) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("ai-title") {
            continue;
        }
        if let Some(t) = value.get("aiTitle").and_then(|t| t.as_str()) {
            title = Some(t.to_string());
        }
    }
    title
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// True if `name` is one plain path component: not empty, not `.` or `..`,
/// and with no path separators, drive letters or other special parts.
pub fn is_plain_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    // Reject both separators on every platform, plus the drive and stream
    // separator `:` and NUL, before asking the OS path parser.
    if name.contains(['/', '\\', ':', '\0']) {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(c)), None) if c == name
    )
}

/// Resolves a session file from untrusted names and checks that it is a
/// file inside `root`.
pub fn session_path(root: &Path, project: &str, session_id: &str) -> Result<PathBuf, SessionError> {
    if !is_plain_name(project) {
        return Err(SessionError::InvalidName { what: "project" });
    }
    if !is_plain_name(session_id) {
        return Err(SessionError::InvalidName { what: "session id" });
    }
    let candidate = root
        .join(project)
        .join(format!("{session_id}.{TRANSCRIPT_EXT}"));
    let resolved = match candidate.canonicalize() {
        Ok(p) => p,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SessionError::NotFound);
        }
        Err(source) => {
            return Err(SessionError::Io {
                what: "the session file",
                source,
            });
        }
    };
    let root = root.canonicalize().map_err(|source| SessionError::Io {
        what: "the projects folder",
        source,
    })?;
    // Catches links or junctions that point outside the projects folder.
    if !resolved.starts_with(&root) {
        return Err(SessionError::OutsideRoot);
    }
    if !resolved.is_file() {
        return Err(SessionError::NotFound);
    }
    Ok(resolved)
}

/// Loads one session with the Claude Code adapter and builds its trace.
/// Events the trace rejects (which would be an adapter bug) are reported as
/// skipped lines instead of failing the whole session.
#[cfg(test)]
pub fn load_trace(
    root: &Path,
    project: &str,
    session_id: &str,
) -> Result<LoadedTrace, SessionError> {
    let path = session_path(root, project, session_id)?;
    let session = adapter_claude_code::load_session(&path)?;
    let mut trace = Trace::new();
    let mut skipped = Vec::new();
    crate::live::apply(&mut trace, &mut skipped, session);
    Ok(LoadedTrace { trace, skipped })
}

/// Checks a trace id sent by the UI. It is only used as a lookup key, never
/// as a path, so this is a sanity check rather than a security boundary.
pub fn check_trace_id(trace_id: &str) -> Result<(), SessionError> {
    if trace_id.is_empty() || trace_id.len() > MAX_TRACE_ID_LEN {
        return Err(SessionError::InvalidTraceId);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::{LiveMessage, LiveSession, all_trace_ids};
    use std::io::Write;

    /// The sanitised fixture folder, laid out like a projects folder with
    /// one project called `basic`.
    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("fixtures")
            .join("claude-code")
    }

    const FIXTURE_SESSION: &str = "00000000-0000-4000-8000-000000000002";

    #[test]
    fn projects_root_is_inside_dot_claude() {
        let root = projects_root(Path::new("home"));
        assert_eq!(root, Path::new("home").join(".claude").join("projects"));
    }

    #[test]
    fn lists_fixture_session_and_skips_subagents() {
        let sessions = list_sessions(&fixture_root()).expect("list");
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        let s = &sessions[0];
        assert_eq!(s.project, "basic");
        assert_eq!(s.project_label, "basic");
        assert_eq!(s.session_id, FIXTURE_SESSION);
        assert!(s.size_bytes > 0);
        assert!(s.modified_ms > 0);
        // The fixture has several ai-title lines; the last one wins.
        let title = s.title.as_deref().expect("title");
        assert!(title.starts_with("Example text"), "{title}");
    }

    #[test]
    fn missing_root_gives_empty_list() {
        let root = fixture_root().join("does-not-exist");
        assert_eq!(list_sessions(&root).expect("list"), Vec::new());
    }

    #[test]
    fn last_title_wins_and_bad_lines_are_ignored() {
        let dir = temp_dir("titles");
        let file = dir.join("s.jsonl");
        std::fs::write(
            &file,
            concat!(
                "{\"type\":\"ai-title\",\"aiTitle\":\"First\"}\n",
                "not json at all \"ai-title\"\n",
                "{\"type\":\"user\",\"text\":\"mentions \\\"ai-title\\\" only\"}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Second\"}\n",
                "{\"type\":\"ai-title\",\"aiTi",
            ),
        )
        .expect("write");
        assert_eq!(read_title(&file).as_deref(), Some("Second"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn file_without_title_has_none() {
        let dir = temp_dir("no-title");
        let file = dir.join("s.jsonl");
        std::fs::write(&file, "{\"type\":\"user\"}\n").expect("write");
        assert_eq!(read_title(&file), None);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn listing_rereads_a_title_only_when_the_file_changes() {
        let root = temp_dir("title-cache");
        let project = root.join("C--work-demo");
        std::fs::create_dir_all(&project).expect("mkdir");
        let file = project.join("s.jsonl");
        let line = |t: &str| format!("{{\"type\":\"ai-title\",\"aiTitle\":\"{t}\"}}\n");
        std::fs::write(&file, line("Aa")).expect("write");
        let cache = TitleCache::default();
        let title = |cache: &TitleCache| {
            list_sessions_cached(&root, cache).expect("list")[0]
                .title
                .clone()
        };

        assert_eq!(title(&cache).as_deref(), Some("Aa"));
        assert_eq!(cache.reads(), 1);
        assert_eq!(title(&cache).as_deref(), Some("Aa"));
        assert_eq!(cache.reads(), 1, "an unchanged file is not read again");

        // Same size and modified time: the cached title is kept.
        let modified = std::fs::metadata(&file)
            .expect("meta")
            .modified()
            .expect("mtime");
        std::fs::write(&file, line("Bb")).expect("write");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .expect("open")
            .set_modified(modified)
            .expect("set mtime");
        assert_eq!(title(&cache).as_deref(), Some("Aa"));
        assert_eq!(cache.reads(), 1);

        // The file grows: it is read again.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .expect("open");
        std::io::Write::write_all(&mut f, line("Cc").as_bytes()).expect("write");
        drop(f);
        assert_eq!(title(&cache).as_deref(), Some("Cc"));
        assert_eq!(cache.reads(), 2);

        // A deleted file is forgotten.
        std::fs::remove_file(&file).expect("delete");
        assert!(
            list_sessions_cached(&root, &cache)
                .expect("list")
                .is_empty()
        );
        assert!(cache.lock().is_empty());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn lists_newest_first_and_ignores_other_files() {
        let root = temp_dir("listing");
        let project = root.join("C--work-demo");
        std::fs::create_dir_all(project.join("old").join("subagents")).expect("mkdir");
        std::fs::write(project.join("old.jsonl"), "{}\n").expect("write");
        std::fs::write(
            project.join("old").join("subagents").join("agent-1.jsonl"),
            "{}\n",
        )
        .expect("write");
        std::fs::write(project.join("notes.txt"), "x").expect("write");
        std::fs::write(root.join("stray.jsonl"), "{}\n").expect("write");
        // Make sure the second file has a later modified time.
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(project.join("new.jsonl"), "{}\n").expect("write");

        let sessions = list_sessions(&root).expect("list");
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["new", "old"]);
        assert_eq!(sessions[0].project_label, "demo");
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn plain_names_are_accepted() {
        for name in ["basic", FIXTURE_SESSION, "C--work-demo", "a.b", "..a"] {
            assert!(is_plain_name(name), "{name}");
        }
    }

    #[test]
    fn unsafe_names_are_rejected() {
        for name in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "../x",
            "..\\x",
            "/abs",
            "\\abs",
            "C:",
            "C:x",
            "C:\\x",
            "\\\\server\\share",
            "a\0b",
            "file.jsonl:stream",
        ] {
            assert!(!is_plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn session_path_rejects_unsafe_inputs() {
        let root = fixture_root();
        let cases = [
            ("..", FIXTURE_SESSION),
            ("basic", "..\\basic\\x"),
            ("basic/..", FIXTURE_SESSION),
            ("", FIXTURE_SESSION),
            ("basic", ""),
            ("C:\\", FIXTURE_SESSION),
        ];
        for (project, id) in cases {
            let err = session_path(&root, project, id).expect_err("must reject");
            assert!(
                matches!(err, SessionError::InvalidName { .. }),
                "{project:?} {id:?}: {err}"
            );
        }
    }

    #[test]
    fn session_path_reports_missing_session() {
        let err = session_path(&fixture_root(), "basic", "nope").expect_err("missing");
        assert!(matches!(err, SessionError::NotFound), "{err}");
    }

    #[test]
    fn loads_fixture_session_as_diagram() {
        let view = LiveSession::open(&fixture_root(), "basic", FIXTURE_SESSION)
            .expect("load")
            .view();
        let diagram = &view.diagram;
        assert_eq!(diagram.harness.as_deref(), Some("claude-code"));
        assert_eq!(diagram.prompts.len(), 5);
        assert_eq!(diagram.prompts[0].root.kind, trace_view::NodeKind::Prompt);
        assert!(view.skipped.is_empty(), "{:?}", view.skipped);
        assert_eq!(view.version, 1);
        // The response must serialise for the UI with the documented shape.
        let json = serde_json::to_value(&view).expect("serialise");
        assert!(json["diagram"]["prompts"].is_array());
        assert!(json["diagram"]["markers"].is_array());
        assert!(json["skipped"].is_array());
        assert!(json["live"].is_boolean());
        assert_eq!(json["version"], 1);
        assert!(json.get("events").is_none(), "raw events must not be sent");
    }

    #[test]
    fn node_detail_comes_from_the_open_session() {
        let mut live = LiveSession::open(&fixture_root(), "basic", FIXTURE_SESSION).expect("open");
        let turn_id = live.view().diagram.prompts[0].turn_id.clone();
        let detail = live.node_detail(&turn_id).expect("known id");
        assert_eq!(detail.trace_id, turn_id);
        assert_eq!(detail.kind, "turn");
        assert!(!detail.raw.is_empty(), "raw source lines are kept");
    }

    #[test]
    fn node_detail_reports_unknown_id_as_none() {
        let live = LiveSession::open(&fixture_root(), "basic", FIXTURE_SESSION).expect("open");
        assert_eq!(live.node_detail("no-such-node"), None);
    }

    #[test]
    fn trace_ids_are_checked() {
        assert!(check_trace_id("turn:1").is_ok());
        let err = check_trace_id("").expect_err("empty id");
        assert!(matches!(err, SessionError::InvalidTraceId), "{err}");
        let long = "x".repeat(MAX_TRACE_ID_LEN + 1);
        let err = check_trace_id(&long).expect_err("long id");
        assert!(matches!(err, SessionError::InvalidTraceId), "{err}");
    }

    #[test]
    fn open_validates_inputs() {
        let root = fixture_root();
        let err = LiveSession::open(&root, "..", FIXTURE_SESSION)
            .err()
            .expect("bad project");
        assert!(matches!(err, SessionError::InvalidName { .. }), "{err}");
        let err = LiveSession::open(&root, "basic", "..\\basic\\x")
            .err()
            .expect("bad session id");
        assert!(matches!(err, SessionError::InvalidName { .. }), "{err}");
    }

    #[test]
    fn errors_do_not_contain_paths() {
        let err = LiveSession::open(&fixture_root(), "basic", "nope")
            .err()
            .expect("missing");
        assert_eq!(err.to_string(), "session not found");
    }

    /// Compares a checked-in UI mock file with what the backend produces, or
    /// rewrites it when `SNITCHCRAFT_UPDATE_SNAPSHOTS` is set.
    fn check_snapshot(path: &Path, expected: &serde_json::Value) {
        let mut text = serde_json::to_string_pretty(expected).expect("serialise");
        text.push('\n');
        if std::env::var_os("SNITCHCRAFT_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(path, &text).expect("write mock data");
            return;
        }
        let actual = std::fs::read_to_string(path).unwrap_or_default();
        let actual: serde_json::Value = serde_json::from_str(&actual).unwrap_or_default();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        assert!(
            &actual == expected,
            "ui/src/mock/{name} is out of date. Run the tests with \
             SNITCHCRAFT_UPDATE_SNAPSHOTS=1 set and review the diff."
        );
    }

    /// A file in `ui/src/mock`.
    fn mock_file(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("ui")
            .join("src")
            .join("mock")
            .join(name)
    }

    /// The UI's dev-only mock mode (`?mock` in the browser) replays the
    /// command responses for the sanitised fixture from a checked-in JSON
    /// file. This test keeps that file identical to what the commands return.
    /// Set `SNITCHCRAFT_UPDATE_SNAPSHOTS=1` to rewrite it after a change.
    #[test]
    fn ui_mock_data_matches_the_commands() {
        let root = fixture_root();
        let mut sessions = list_sessions(&root).expect("list");
        // The file's modified time depends on the checkout, so pin it and
        // the live flag that comes from it.
        for s in &mut sessions {
            s.modified_ms = 1_767_225_600_000;
            s.live = false;
        }
        let mut live = LiveSession::open(&root, "basic", FIXTURE_SESSION).expect("open");
        let mut view = live.view();
        view.live = false;
        let loaded = load_trace(&root, "basic", FIXTURE_SESSION).expect("trace");
        let mut details = serde_json::Map::new();
        for id in all_trace_ids(&loaded.trace) {
            let detail = live.node_detail(&id).expect("known id");
            details.insert(id, serde_json::to_value(detail).expect("serialise"));
        }
        let expected = serde_json::json!({
            "sessions": sessions,
            "views": { format!("basic/{FIXTURE_SESSION}"): view },
            "details": details,
        });
        check_snapshot(&mock_file("fixture-data.json"), &expected);
    }

    /// `ui/src/mock/capture-data.json` holds the capture overview and every
    /// capture detail for the invented capture fixture, so the dev mock mode
    /// can show the capture views. The stored size is pinned, since the
    /// fixture is not read through the store here.
    #[test]
    fn ui_capture_data_matches_the_backend() {
        let root = fixture_root();
        let live = LiveSession::open(&root, "basic", FIXTURE_SESSION).expect("open");
        let loaded = load_trace(&root, "basic", FIXTURE_SESSION).expect("trace");
        let records = crate::captures::fixture_records();
        let captures = capture::Loaded {
            records: records.clone(),
            skipped: Vec::new(),
        };
        let key = live.capture_key();
        let overview = crate::captures::overview(&loaded.trace, &captures, key.as_deref(), 123_456);
        let mut details = serde_json::Map::new();
        for record in &records {
            let detail =
                crate::captures::detail(&loaded.trace, &records, &record.id).expect("known id");
            details.insert(
                record.id.clone(),
                serde_json::to_value(detail).expect("serialise"),
            );
        }
        let expected = serde_json::json!({ "overview": overview, "details": details });
        check_snapshot(&mock_file("capture-data.json"), &expected);
    }

    /// Copies a folder and everything in it.
    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read dir") {
            let entry = entry.expect("entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("type").is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy");
            }
        }
    }

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
        copy_dir(
            &fixture.join(FIXTURE_SESSION),
            &project.join(FIXTURE_SESSION),
        );
        let text = std::fs::read_to_string(fixture.join(format!("{FIXTURE_SESSION}.jsonl")))
            .expect("read");
        let lines: Vec<&str> = text.lines().collect();
        let size = lines.len().div_ceil(6);
        let main = project.join(format!("{FIXTURE_SESSION}.jsonl"));
        let mut steps = Vec::new();
        let mut live: Option<LiveSession> = None;
        for chunk in lines.chunks(size) {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&main)
                .expect("open");
            for line in chunk {
                writeln!(f, "{line}").expect("write");
            }
            drop(f);
            let message = match live.as_mut() {
                None => {
                    let mut opened =
                        LiveSession::open(&root, "basic", FIXTURE_SESSION).expect("open");
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
        drop(live);
        assert_eq!(steps.len(), 6);
        std::fs::remove_dir_all(&root).expect("cleanup");
        check_snapshot(
            &mock_file("live-steps.json"),
            &serde_json::Value::Array(steps),
        );
    }

    /// A fresh empty folder under the system temp folder.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("snitchcraft-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }
}
