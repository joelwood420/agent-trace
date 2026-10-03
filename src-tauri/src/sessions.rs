//! Finding and loading Claude Code session files. Everything here is
//! read-only and takes the projects folder as a parameter, so tests can point
//! it at a fixture folder instead of the real home directory.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use trace_core::TraceEvent;

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

/// The result of loading one session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LoadedSession {
    /// Trace events in order. Apply them in order.
    pub events: Vec<TraceEvent>,
    /// Lines the adapter skipped.
    pub skipped: Vec<SkippedLine>,
}

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

/// Loads one session with the Claude Code adapter.
pub fn load_session(
    root: &Path,
    project: &str,
    session_id: &str,
) -> Result<LoadedSession, SessionError> {
    let path = session_path(root, project, session_id)?;
    let session = adapter_claude_code::load_session(&path)?;
    let skipped = session
        .skipped
        .into_iter()
        .map(|s| SkippedLine {
            source: s.source,
            line: s.line,
            reason: s.reason,
        })
        .collect();
    Ok(LoadedSession {
        events: session.events,
        skipped,
    })
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
    fn loads_fixture_session() {
        let loaded = load_session(&fixture_root(), "basic", FIXTURE_SESSION).expect("load");
        assert!(!loaded.events.is_empty());
        assert_eq!(loaded.events[0].node.kind_name(), "run");
        // The response must serialise for the UI.
        let json = serde_json::to_value(&loaded).expect("serialise");
        assert!(json["events"].is_array());
        assert!(json["skipped"].is_array());
    }

    #[test]
    fn errors_do_not_contain_paths() {
        let err = load_session(&fixture_root(), "basic", "nope").expect_err("missing");
        assert_eq!(err.to_string(), "session not found");
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
