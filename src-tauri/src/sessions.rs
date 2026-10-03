//! Finding and loading Claude Code session files. Everything here is
//! read-only and takes the projects folder as a parameter, so tests can point
//! it at a fixture folder instead of the real home directory.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use trace_core::Trace;
use trace_view::{NodeDetail, SessionDiagram};

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
    /// A trace node id was empty or too long to be one.
    #[error("invalid trace id")]
    InvalidTraceId,
    /// Another thread panicked while holding the session cache.
    #[error("internal error: session cache is unavailable")]
    CachePoisoned,
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
}

/// A session read from disk and turned into a trace.
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

/// Lists every session transcript directly inside each project folder,
/// newest first. Subagent transcripts (in `<session id>/subagents/`) are not
/// listed, since they belong to their parent session. A missing projects
/// folder gives an empty list.
pub fn list_sessions(root: &Path) -> Result<Vec<SessionSummary>, SessionError> {
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
            if let Some(summary) = summarise(&project_name, &entry) {
                sessions.push(summary);
            }
        }
    }
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
fn summarise(project: &str, entry: &std::fs::DirEntry) -> Option<SessionSummary> {
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()) != Some(TRANSCRIPT_EXT) {
        return None;
    }
    let meta = entry.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let session_id = path.file_stem()?.to_str()?.to_string();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0);
    Some(SessionSummary {
        project: project.to_string(),
        project_label: project_label(project),
        session_id,
        title: read_title(&path),
        modified_ms,
        size_bytes: meta.len(),
    })
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
pub fn load_trace(
    root: &Path,
    project: &str,
    session_id: &str,
) -> Result<LoadedTrace, SessionError> {
    let path = session_path(root, project, session_id)?;
    let session = adapter_claude_code::load_session(&path)?;
    let mut skipped: Vec<SkippedLine> = session
        .skipped
        .into_iter()
        .map(|s| SkippedLine {
            source: s.source,
            line: s.line,
            reason: s.reason,
        })
        .collect();
    let mut trace = Trace::new();
    for event in session.events {
        let (source, line) = event
            .raw
            .first()
            .map(|r| (r.source.clone(), r.line))
            .unwrap_or_default();
        if let Err(err) = trace.apply(event) {
            tracing::warn!(error = %err, "trace rejected an event");
            skipped.push(SkippedLine {
                source,
                line,
                reason: format!("trace rejected the event: {err}"),
            });
        }
    }
    Ok(LoadedTrace { trace, skipped })
}

fn view_of(loaded: &LoadedTrace) -> SessionView {
    SessionView {
        diagram: trace_view::build_session(&loaded.trace),
        skipped: loaded.skipped.clone(),
    }
}

/// Checks a trace id sent by the UI. It is only used as a lookup key, never
/// as a path, so this is a sanity check rather than a security boundary.
pub fn check_trace_id(trace_id: &str) -> Result<(), SessionError> {
    if trace_id.is_empty() || trace_id.len() > MAX_TRACE_ID_LEN {
        return Err(SessionError::InvalidTraceId);
    }
    Ok(())
}

/// The most recently loaded session's trace, so that clicking boxes in the
/// diagram does not re-read the transcript for every click.
#[derive(Debug, Default)]
pub struct SessionCache {
    current: Mutex<Option<CachedTrace>>,
}

#[derive(Debug, Clone)]
struct CachedTrace {
    project: String,
    session_id: String,
    trace: Arc<Trace>,
}

impl SessionCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the session from disk (always, so the view is fresh), keeps its
    /// trace for later `node_detail` calls, and returns its diagram model.
    pub fn load_session(
        &self,
        root: &Path,
        project: &str,
        session_id: &str,
    ) -> Result<SessionView, SessionError> {
        let loaded = load_trace(root, project, session_id)?;
        let view = view_of(&loaded);
        self.store(project, session_id, Arc::new(loaded.trace))?;
        Ok(view)
    }

    /// Full content of one trace node. Uses the cached trace if it belongs to
    /// this session, otherwise loads the session first and caches it.
    /// Returns `None` if the session has no node with that id.
    pub fn node_detail(
        &self,
        root: &Path,
        project: &str,
        session_id: &str,
        trace_id: &str,
    ) -> Result<Option<NodeDetail>, SessionError> {
        check_trace_id(trace_id)?;
        // Validate the names even when the cache would answer, so the
        // command behaves the same either way.
        session_path(root, project, session_id)?;
        let trace = match self.cached(project, session_id)? {
            Some(trace) => trace,
            None => {
                let loaded = load_trace(root, project, session_id)?;
                let trace = Arc::new(loaded.trace);
                self.store(project, session_id, Arc::clone(&trace))?;
                trace
            }
        };
        Ok(trace_view::node_detail(&trace, trace_id))
    }

    fn cached(&self, project: &str, session_id: &str) -> Result<Option<Arc<Trace>>, SessionError> {
        let guard = self
            .current
            .lock()
            .map_err(|_| SessionError::CachePoisoned)?;
        Ok(guard
            .as_ref()
            .filter(|c| c.project == project && c.session_id == session_id)
            .map(|c| Arc::clone(&c.trace)))
    }

    fn store(
        &self,
        project: &str,
        session_id: &str,
        trace: Arc<Trace>,
    ) -> Result<(), SessionError> {
        let mut guard = self
            .current
            .lock()
            .map_err(|_| SessionError::CachePoisoned)?;
        *guard = Some(CachedTrace {
            project: project.to_string(),
            session_id: session_id.to_string(),
            trace,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let view = SessionCache::new()
            .load_session(&fixture_root(), "basic", FIXTURE_SESSION)
            .expect("load");
        let diagram = &view.diagram;
        assert_eq!(diagram.harness.as_deref(), Some("claude-code"));
        assert_eq!(diagram.prompts.len(), 5);
        assert_eq!(diagram.prompts[0].root.kind, trace_view::NodeKind::Prompt);
        assert!(view.skipped.is_empty(), "{:?}", view.skipped);
        // The response must serialise for the UI with the documented shape.
        let json = serde_json::to_value(&view).expect("serialise");
        assert!(json["diagram"]["prompts"].is_array());
        assert!(json["diagram"]["markers"].is_array());
        assert!(json["skipped"].is_array());
        assert!(json.get("events").is_none(), "raw events must not be sent");
    }

    #[test]
    fn node_detail_without_prior_load_reads_the_session() {
        let cache = SessionCache::new();
        let view = SessionCache::new()
            .load_session(&fixture_root(), "basic", FIXTURE_SESSION)
            .expect("load");
        let turn_id = view.diagram.prompts[0].turn_id.clone();
        let detail = cache
            .node_detail(&fixture_root(), "basic", FIXTURE_SESSION, &turn_id)
            .expect("detail")
            .expect("known id");
        assert_eq!(detail.trace_id, turn_id);
        assert_eq!(detail.kind, "turn");
        assert!(!detail.raw.is_empty(), "raw source lines are kept");
    }

    #[test]
    fn node_detail_reports_unknown_id_as_none() {
        let cache = SessionCache::new();
        let detail = cache
            .node_detail(&fixture_root(), "basic", FIXTURE_SESSION, "no-such-node")
            .expect("detail");
        assert_eq!(detail, None);
    }

    #[test]
    fn node_detail_validates_inputs() {
        let cache = SessionCache::new();
        let root = fixture_root();
        let err = cache
            .node_detail(&root, "..", FIXTURE_SESSION, "x")
            .expect_err("bad project");
        assert!(matches!(err, SessionError::InvalidName { .. }), "{err}");
        let err = cache
            .node_detail(&root, "basic", "..\\basic\\x", "x")
            .expect_err("bad session id");
        assert!(matches!(err, SessionError::InvalidName { .. }), "{err}");
        let err = cache
            .node_detail(&root, "basic", FIXTURE_SESSION, "")
            .expect_err("empty id");
        assert!(matches!(err, SessionError::InvalidTraceId), "{err}");
        let long = "x".repeat(MAX_TRACE_ID_LEN + 1);
        let err = cache
            .node_detail(&root, "basic", FIXTURE_SESSION, &long)
            .expect_err("long id");
        assert!(matches!(err, SessionError::InvalidTraceId), "{err}");
        let err = cache
            .node_detail(&root, "basic", "nope", "x")
            .expect_err("missing");
        assert!(matches!(err, SessionError::NotFound), "{err}");
    }

    #[test]
    fn node_detail_uses_the_cached_trace() {
        // Copy the fixture session into a temp projects folder, load it,
        // then empty the file. Details must still come from the cache.
        let root = temp_dir("cache");
        let project = root.join("demo");
        std::fs::create_dir_all(&project).expect("mkdir");
        let file = project.join(format!("{FIXTURE_SESSION}.jsonl"));
        let source = fixture_root()
            .join("basic")
            .join(format!("{FIXTURE_SESSION}.jsonl"));
        std::fs::copy(&source, &file).expect("copy");

        let cache = SessionCache::new();
        let view = cache
            .load_session(&root, "demo", FIXTURE_SESSION)
            .expect("load");
        let turn_id = view.diagram.prompts[1].turn_id.clone();
        std::fs::write(&file, "").expect("truncate");

        let detail = cache
            .node_detail(&root, "demo", FIXTURE_SESSION, &turn_id)
            .expect("detail");
        assert!(detail.is_some(), "served from the cache");

        // Loading again re-reads the now empty file and replaces the cache.
        let view = cache
            .load_session(&root, "demo", FIXTURE_SESSION)
            .expect("reload");
        assert!(view.diagram.prompts.is_empty());
        let detail = cache
            .node_detail(&root, "demo", FIXTURE_SESSION, &turn_id)
            .expect("detail");
        assert_eq!(detail, None);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn errors_do_not_contain_paths() {
        let err = SessionCache::new()
            .load_session(&fixture_root(), "basic", "nope")
            .expect_err("missing");
        assert_eq!(err.to_string(), "session not found");
    }

    /// Every trace node id, depth first.
    fn all_trace_ids(trace: &Trace) -> Vec<String> {
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

    /// The UI's dev-only mock mode (`?mock` in the browser) replays the
    /// command responses for the sanitised fixture from a checked-in JSON
    /// file. This test keeps that file identical to what the commands return.
    /// Set `LOOPTRACE_UPDATE_SNAPSHOTS=1` to rewrite it after a change.
    #[test]
    fn ui_mock_data_matches_the_commands() {
        let root = fixture_root();
        let mut sessions = list_sessions(&root).expect("list");
        // The file's modified time depends on the checkout, so pin it.
        for s in &mut sessions {
            s.modified_ms = 1_767_225_600_000;
        }
        let cache = SessionCache::new();
        let view = cache
            .load_session(&root, "basic", FIXTURE_SESSION)
            .expect("load");
        let loaded = load_trace(&root, "basic", FIXTURE_SESSION).expect("trace");
        let mut details = serde_json::Map::new();
        for id in all_trace_ids(&loaded.trace) {
            let detail = cache
                .node_detail(&root, "basic", FIXTURE_SESSION, &id)
                .expect("detail")
                .expect("known id");
            details.insert(id, serde_json::to_value(detail).expect("serialise"));
        }
        let expected = serde_json::json!({
            "sessions": sessions,
            "views": { format!("basic/{FIXTURE_SESSION}"): view },
            "details": details,
        });

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("ui")
            .join("src")
            .join("mock")
            .join("fixture-data.json");
        let mut text = serde_json::to_string_pretty(&expected).expect("serialise");
        text.push('\n');
        if std::env::var_os("LOOPTRACE_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &text).expect("write mock data");
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        let actual: serde_json::Value = serde_json::from_str(&actual).unwrap_or_default();
        assert!(
            actual == expected,
            "ui/src/mock/fixture-data.json is out of date. Run the tests with \
             LOOPTRACE_UPDATE_SNAPSHOTS=1 set and review the diff."
        );
    }

    /// A fresh empty folder under the system temp folder.
    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("looptrace-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }
}
