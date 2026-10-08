//! Character measures of a call's context, slice by slice.

use std::collections::BTreeMap;

use serde::Serialize;

/// The kinds of context a model call's input is split into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceKind {
    /// The system prompt.
    SystemPrompt,
    /// Tool definitions sent with the request.
    ToolDefinitions,
    /// Injected instructions and reminders.
    Instructions,
    /// Content of files the agent read.
    FilesRead,
    /// Results of tools other than file reads.
    ToolResults,
    /// Prompts and assistant messages.
    Conversation,
    /// Context that could not be seen.
    NotCaptured,
}

impl SliceKind {
    /// Every kind, in display order.
    pub const ALL: [SliceKind; 7] = [
        SliceKind::SystemPrompt,
        SliceKind::ToolDefinitions,
        SliceKind::Instructions,
        SliceKind::FilesRead,
        SliceKind::ToolResults,
        SliceKind::Conversation,
        SliceKind::NotCaptured,
    ];

    /// The name shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            SliceKind::SystemPrompt => "System prompt",
            SliceKind::ToolDefinitions => "Tool definitions",
            SliceKind::Instructions => "Instructions and reminders",
            SliceKind::FilesRead => "Files read",
            SliceKind::ToolResults => "Other tool results",
            SliceKind::Conversation => "Conversation",
            SliceKind::NotCaptured => "Not captured",
        }
    }
}

/// Where a measure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// Measured from a captured API request.
    Captured,
    /// Estimated from the transcript.
    Transcript,
}

/// One named part of a slice, in characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredItem {
    /// Name of the item.
    pub label: String,
    /// Total characters over all occurrences.
    pub chars: u64,
    /// Number of occurrences.
    pub count: u32,
    /// Characters of the biggest single occurrence.
    pub largest_chars: u64,
    /// True when the item comes from a context part the transcript recorded.
    pub from_transcript: bool,
}

/// A call's context measured in characters, slice by slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMeasure {
    /// Where the measure came from.
    pub source: ContextSource,
    slices: BTreeMap<SliceKind, Vec<MeasuredItem>>,
}

impl ContextMeasure {
    /// An empty measure.
    pub fn new(source: ContextSource) -> Self {
        Self {
            source,
            slices: BTreeMap::new(),
        }
    }

    /// Adds `chars` to the item `label` of slice `kind`, creating it if
    /// needed. Each call is one occurrence: `count` goes up by one and
    /// `largest_chars` keeps the biggest single occurrence.
    pub fn add(&mut self, kind: SliceKind, label: &str, chars: u64) {
        self.add_many(kind, label, chars, 1);
    }

    /// Like `add` but adds `count` occurrences at once (for tool sets:
    /// one item per MCP server whose count is its number of tools).
    pub fn add_many(&mut self, kind: SliceKind, label: &str, chars: u64, count: u32) {
        self.add_item(kind, label, chars, count, false);
    }

    /// Adds one occurrence of a context part the transcript recorded. The
    /// item is marked `from_transcript`.
    pub fn add_part(&mut self, kind: SliceKind, label: &str, chars: u64) {
        self.add_item(kind, label, chars, 1, true);
    }

    /// True when any item came from a transcript context part.
    pub fn has_parts(&self) -> bool {
        self.slices.values().flatten().any(|i| i.from_transcript)
    }

    fn add_item(&mut self, kind: SliceKind, label: &str, chars: u64, count: u32, part: bool) {
        let items = self.slices.entry(kind).or_default();
        match items.iter_mut().find(|i| i.label == label) {
            Some(item) => {
                item.chars += chars;
                item.count += count;
                item.largest_chars = item.largest_chars.max(chars);
                item.from_transcript |= part;
            }
            None => items.push(MeasuredItem {
                label: label.to_string(),
                chars,
                count,
                largest_chars: chars,
                from_transcript: part,
            }),
        }
    }

    /// Clears everything collected (used at a compaction marker).
    pub fn clear(&mut self) {
        self.slices.clear();
    }

    /// The items of one slice, in insertion order.
    pub fn items(&self, kind: SliceKind) -> &[MeasuredItem] {
        self.slices.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Total characters of one slice.
    pub fn slice_chars(&self, kind: SliceKind) -> u64 {
        self.items(kind).iter().map(|i| i.chars).sum()
    }

    /// Total characters of every slice.
    pub fn total_chars(&self) -> u64 {
        SliceKind::ALL.iter().map(|k| self.slice_chars(*k)).sum()
    }
}

/// Character weight of one image, so images show up without decoding them.
pub const IMAGE_CHARS: u64 = 6_000;

/// Length of `value` as compact JSON text.
pub fn json_chars(value: &serde_json::Value) -> u64 {
    serde_json::to_string(value)
        .map(|s| s.chars().count() as u64)
        .unwrap_or(0)
}

/// Length of `text` in characters.
pub fn text_chars(text: &str) -> u64 {
    text.chars().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_merges_items_by_label_and_keeps_the_largest() {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        m.add(SliceKind::FilesRead, "a.rs", 100);
        m.add(SliceKind::FilesRead, "a.rs", 300);
        m.add(SliceKind::FilesRead, "b.rs", 50);
        let items = m.items(SliceKind::FilesRead);
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            MeasuredItem {
                label: "a.rs".into(),
                chars: 400,
                count: 2,
                largest_chars: 300,
                from_transcript: false
            }
        );
        assert_eq!(m.slice_chars(SliceKind::FilesRead), 450);
        assert_eq!(m.total_chars(), 450);
    }

    #[test]
    fn add_many_counts_several_occurrences() {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        m.add_many(SliceKind::ToolDefinitions, "MCP: docs", 900, 3);
        assert_eq!(m.items(SliceKind::ToolDefinitions)[0].count, 3);
    }

    #[test]
    fn add_part_marks_items_and_has_parts() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "your prompts", 10);
        assert!(!m.has_parts());
        m.add_part(SliceKind::SystemPrompt, "system prompt", 30);
        assert!(m.has_parts());
        assert!(m.items(SliceKind::SystemPrompt)[0].from_transcript);
        assert!(!m.items(SliceKind::Conversation)[0].from_transcript);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "your prompts", 10);
        m.clear();
        assert_eq!(m.total_chars(), 0);
        assert!(m.items(SliceKind::Conversation).is_empty());
    }

    #[test]
    fn json_and_text_lengths() {
        assert_eq!(json_chars(&json!({"a": 1})), 7);
        assert_eq!(text_chars("héllo"), 5);
    }

    #[test]
    fn kinds_have_labels_in_display_order() {
        assert_eq!(SliceKind::ALL[0], SliceKind::SystemPrompt);
        assert_eq!(SliceKind::ALL[6], SliceKind::NotCaptured);
        assert_eq!(
            SliceKind::Instructions.label(),
            "Instructions and reminders"
        );
    }
}
