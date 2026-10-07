//! Harness rules, supplied as plain data by an adapter.

/// What a harness's injected reminders and file reads look like. Plain data
/// supplied by the adapter, so this crate stays harness neutral.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextRules {
    /// Text that opens an injected reminder, for example a tag.
    pub reminder_open: String,
    /// Text that closes an injected reminder.
    pub reminder_close: String,
    /// Text that starts a new item inside a reminder, if the harness has one.
    pub section_marker: Option<String>,
    /// Names for reminder sections; the first rule whose needle appears wins.
    pub labels: Vec<LabelRule>,
    /// Tools whose result is the content of a file named in their input.
    pub file_reads: Vec<FileReadRule>,
}

/// A rule that names a reminder section.
///
/// For a section that starts with the section marker, only its header line
/// (the text after the marker up to the first line end) is searched for the
/// needle, and a marker section that no rule matches is labelled
/// "file: <detail>". Any other section is searched as a whole and falls back
/// to "other reminders".
#[derive(Debug, Clone, PartialEq)]
pub struct LabelRule {
    /// Text that must appear in the section (or in its header line, for a
    /// section that starts with the section marker).
    pub needle: String,
    /// The name shown for the section.
    pub label: String,
    /// When true and the section starts with the section marker, the label
    /// is "<label>: <detail>", where the detail is the header line up to its
    /// last " (" (or the whole line), trimmed and without a trailing ":".
    pub with_detail: bool,
}

/// A rule that recognises a tool whose result is a file's content.
#[derive(Debug, Clone, PartialEq)]
pub struct FileReadRule {
    /// Tool name, matched exactly.
    pub tool: String,
    /// Input field holding the file path.
    pub path_field: String,
}
