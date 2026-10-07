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
#[derive(Debug, Clone, PartialEq)]
pub struct LabelRule {
    /// Text that must appear in the section.
    pub needle: String,
    /// The name shown for the section.
    pub label: String,
    /// When true and the section starts with the section marker, the text
    /// after the marker up to the first " (" or line end is added as
    /// "<label>: <detail>".
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
