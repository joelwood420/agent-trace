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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("snitchcraft-tail-{}-{name}", std::process::id()));
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
        assert_eq!(
            lines(tail.read_new().expect("read")),
            [(1, "{\"a\":1}".to_string())]
        );
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
        assert_eq!(
            lines(tail.read_new().expect("read")),
            [(1, "{\"a\":1}".to_string())]
        );
        assert_eq!(tail.take_final_line(), Some((2, "{\"b\":2}".to_string())));

        let path = temp_file("final-partial");
        append(&path, b"{\"a\":1}\n{\"b\":");
        let mut tail = TranscriptTail::new(&path);
        lines(tail.read_new().expect("read"));
        assert_eq!(tail.take_final_line(), None);
    }
}
