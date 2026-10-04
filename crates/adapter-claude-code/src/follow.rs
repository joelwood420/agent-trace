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
            let file = self
                .subagent_dir
                .join(format!("agent-{}.jsonl", link.agent_id));
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
            let Some(tool_use_id) = read_meta(&entry.path()).and_then(|m| {
                m.get("toolUseId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }) else {
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
        session
            .events
            .extend(source.parser.push_line(*line_no, text));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_found_twice_is_started_once() {
        let dir =
            std::env::temp_dir().join(format!("snitchcraft-follow-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("s").join("subagents")).expect("mkdir");
        std::fs::write(dir.join("s").join("subagents").join("agent-x.jsonl"), b"").expect("write");
        std::fs::write(dir.join("s.jsonl"), b"").expect("write");
        let mut follower = SessionFollower::new(&dir.join("s.jsonl")).expect("follower");
        let link = SubagentLink {
            agent_id: "x".into(),
            tool_node_id: "tool:t".into(),
        };
        for _ in 0..2 {
            if follower.linked.insert(link.agent_id.clone()) {
                follower.waiting.push(link.clone());
            }
            follower.link_subagents(ReadMode::Live);
        }
        assert_eq!(follower.subagents.len(), 1);
    }
}
