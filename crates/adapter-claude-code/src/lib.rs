//! Reads Claude Code JSONL transcripts and converts them into `trace-core`
//! events. Parsing is defensive: unknown fields and event types are skipped
//! and logged, never a panic.

mod capture;
mod context;
mod context_parts;
mod follow;
mod parser;
mod tail;
mod time;

use std::path::{Path, PathBuf};

use trace_core::TraceEvent;

pub use capture::{SESSION_HEADER, model_call_id, session_key};
pub use context::context_rules;
pub use follow::{PollOutcome, ReadMode, SessionFollower};
pub use parser::{HARNESS, Parser, Skipped, SubagentLink};
pub use tail::{TailRead, TranscriptTail};

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
