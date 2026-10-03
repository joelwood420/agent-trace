//! Reads Claude Code JSONL transcripts and converts them into `trace-core`
//! events. Parsing is defensive: unknown fields and event types are skipped
//! and logged, never a panic.

mod parser;
mod time;

use std::path::{Path, PathBuf};

use trace_core::TraceEvent;

pub use parser::{HARNESS, Parser, Skipped, SubagentLink};

/// Errors from reading transcript files.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// A transcript file could not be read.
    #[error("could not read {path}: {source}")]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The path has no usable file name to take the session id from.
    #[error("not a transcript file: {0}")]
    NotATranscript(PathBuf),
}

/// The result of loading one session.
#[derive(Debug, Default)]
pub struct Session {
    /// Trace events in the order they were produced. Apply them in order.
    pub events: Vec<TraceEvent>,
    /// Lines that could not be used.
    pub skipped: Vec<Skipped>,
}

/// Loads a finished session: the main transcript at `path` plus every
/// subagent transcript it links to (`<session id>/subagents/agent-<id>.jsonl`).
pub fn load_session(path: &Path) -> Result<Session, AdapterError> {
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AdapterError::NotATranscript(path.to_path_buf()))?;
    let source = file_name(path);

    let mut session = Session::default();
    let mut parser = Parser::for_session(&source, session_id);
    feed_file(&mut parser, path, &mut session.events)?;
    session.events.extend(parser.finish());
    session.skipped.extend(parser.take_skipped());

    let subagent_dir = path.with_file_name(session_id).join("subagents");
    let mut pending: Vec<SubagentLink> = parser.subagents().to_vec();
    // Subagents can start further subagents, so work through a queue.
    while let Some(link) = pending.pop() {
        let file = subagent_dir.join(format!("agent-{}.jsonl", link.agent_id));
        if !file.is_file() {
            tracing::warn!(agent_id = %link.agent_id, "linked subagent transcript not found");
            continue;
        }
        let title = subagent_title(&subagent_dir, &link.agent_id);
        let source = format!("subagents/{}", file_name(&file));
        let mut sub = Parser::for_subagent(&source, &link, title);
        feed_file(&mut sub, &file, &mut session.events)?;
        session.events.extend(sub.finish());
        session.skipped.extend(sub.take_skipped());
        pending.extend(sub.subagents().iter().cloned());
    }
    Ok(session)
}

/// Feeds every complete line of a file to the parser.
fn feed_file(
    parser: &mut Parser,
    path: &Path,
    events: &mut Vec<TraceEvent>,
) -> Result<(), AdapterError> {
    // std on Windows opens files with read, write and delete sharing, so
    // this never blocks Claude Code from writing the transcript.
    let bytes = std::fs::read(path).map_err(|source| AdapterError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let text = String::from_utf8_lossy(&bytes);
    for (line_no, line) in complete_lines(&text) {
        events.extend(parser.push_line(line_no, line));
    }
    Ok(())
}

/// Splits text into numbered lines, leaving out a final line that is still
/// being written. A last line without a newline counts as complete only if
/// it is valid JSON; otherwise it is "not ready yet", not an error.
fn complete_lines(text: &str) -> Vec<(u64, &str)> {
    let mut lines: Vec<(u64, &str)> = text
        .split('\n')
        .enumerate()
        .map(|(i, l)| (i as u64 + 1, l.strip_suffix('\r').unwrap_or(l)))
        .collect();
    if let Some((line_no, last)) = lines.pop() {
        let finished =
            !last.trim().is_empty() && serde_json::from_str::<serde_json::Value>(last).is_ok();
        if finished {
            lines.push((line_no, last));
        } else if !last.trim().is_empty() {
            tracing::debug!(line = line_no, "last line incomplete, not ready yet");
        }
    }
    lines
}

/// The subagent's description from its `.meta.json` sidecar, if present.
fn subagent_title(dir: &Path, agent_id: &str) -> Option<String> {
    let meta = dir.join(format!("agent-{agent_id}.meta.json"));
    let text = std::fs::read_to_string(meta).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("description")
        .and_then(|d| d.as_str())
        .map(str::to_string)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::complete_lines;

    #[test]
    fn incomplete_last_line_is_held_back() {
        let lines = complete_lines("{\"a\":1}\n{\"b\":");
        assert_eq!(lines, [(1, "{\"a\":1}")]);
    }

    #[test]
    fn complete_last_line_without_newline_is_kept() {
        let lines = complete_lines("{\"a\":1}\r\n{\"b\":2}");
        assert_eq!(lines, [(1, "{\"a\":1}"), (2, "{\"b\":2}")]);
    }

    #[test]
    fn trailing_newline_gives_no_extra_line() {
        let lines = complete_lines("{\"a\":1}\n");
        assert_eq!(lines, [(1, "{\"a\":1}")]);
    }
}
