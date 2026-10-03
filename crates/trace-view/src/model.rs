//! The diagram model types. These are the contract the UI consumes.
//!
//! Every field is always present in JSON. Unknown values are `null`, never
//! left out, so the UI can rely on one fixed shape.

use serde::Serialize;

/// A whole session, ready to draw: run-level facts, markers that happened
/// outside any prompt, and one diagram per prompt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionDiagram {
    /// Trace id of the session's `Run`, or `None` if the trace has no run.
    pub run_id: Option<String>,
    /// The run's title, if the harness gave one.
    pub title: Option<String>,
    /// Name of the harness that produced the run.
    pub harness: Option<String>,
    /// Run start, milliseconds since the Unix epoch.
    pub started_at_ms: Option<i64>,
    /// Run end, milliseconds since the Unix epoch.
    pub ended_at_ms: Option<i64>,
    /// `ended_at_ms - started_at_ms`, when both are known.
    pub duration_ms: Option<i64>,
    /// Markers that are direct children of the run (things that happened
    /// outside any prompt, such as a hook at session start), in order.
    pub markers: Vec<DiagramNode>,
    /// One diagram per prompt (top-level turn), in order.
    pub prompts: Vec<PromptDiagram>,
}

/// The diagram for one prompt and everything the agent did in response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptDiagram {
    /// Position of this prompt in the session, starting at 0.
    pub index: usize,
    /// Trace id of the `Turn` this diagram was built from.
    pub turn_id: String,
    /// The prompt text on one line, shortened.
    pub prompt_preview: String,
    /// Turn start, milliseconds since the Unix epoch.
    pub started_at_ms: Option<i64>,
    /// Turn end, milliseconds since the Unix epoch.
    pub ended_at_ms: Option<i64>,
    /// `ended_at_ms - started_at_ms`, when both are known.
    pub duration_ms: Option<i64>,
    /// Summary numbers for the whole prompt.
    pub totals: PromptTotals,
    /// The diagram tree. Its root is a `prompt` node.
    pub root: DiagramNode,
}

/// Summary numbers for a prompt or a subagent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PromptTotals {
    /// Model calls, including those made by subagents.
    pub model_calls: usize,
    /// Tool calls, including those made by subagents.
    pub tool_calls: usize,
    /// Tool calls whose result is an error, including those in subagents.
    pub errors: usize,
    /// Largest context size of any model call made by this agent itself.
    /// Subagents are left out because they have their own context window.
    pub max_context_tokens: Option<u64>,
    /// Output tokens of all model calls, including subagents. `None` if no
    /// model call reported output tokens.
    pub output_tokens: Option<u64>,
}

/// One box in the diagram.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagramNode {
    /// Unique within the session diagram, and the same every time the same
    /// trace is built. Made of the kind and a trace id, for example
    /// `"tool_call:tc-1"` or `"parallel_group:mc-1"`.
    pub id: String,
    /// What this box represents.
    pub kind: NodeKind,
    /// Short main text.
    pub label: String,
    /// Optional second line, for example model name and stop reason.
    pub detail_label: Option<String>,
    /// Start time, milliseconds since the Unix epoch.
    pub started_at_ms: Option<i64>,
    /// Duration in milliseconds, when known. For groups and summaries this
    /// spans from the first start to the last end of everything inside.
    pub duration_ms: Option<i64>,
    /// Context size sent to the model, in tokens. For nodes that hold
    /// several model calls, the largest one.
    pub context_tokens: Option<u64>,
    /// Output tokens. For nodes that hold several model calls, the sum.
    pub output_tokens: Option<u64>,
    /// Outcome of this box. Container boxes show the worst status inside.
    pub status: Status,
    /// `true` if the UI should show this box with its children hidden
    /// until the user expands it.
    pub collapsed_by_default: bool,
    /// The trace node ids this box stands for. Pass one to `node_detail`
    /// to get full content for a details panel.
    pub trace_ids: Vec<String>,
    /// Child boxes, in order.
    pub children: Vec<DiagramNode>,
}

/// What a diagram box represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// The user's prompt. Root of each prompt diagram.
    Prompt,
    /// One request to the model.
    ModelCall,
    /// One tool use.
    ToolCall,
    /// Two or more tool calls requested together by one model call.
    ParallelGroup,
    /// A subagent run started by a tool call.
    Subagent,
    /// A collapsed run of similar consecutive model calls.
    Summary,
    /// Something notable that is not a call, such as a hook or compaction.
    Marker,
}

impl NodeKind {
    /// The kind name used in JSON, for example `"parallel_group"`.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Prompt => "prompt",
            NodeKind::ModelCall => "model_call",
            NodeKind::ToolCall => "tool_call",
            NodeKind::ParallelGroup => "parallel_group",
            NodeKind::Subagent => "subagent",
            NodeKind::Summary => "summary",
            NodeKind::Marker => "marker",
        }
    }
}

/// The outcome shown on a diagram box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Finished without error.
    Ok,
    /// A tool reported an error (here or, for containers, inside).
    Error,
    /// Not finished yet.
    Running,
    /// No status applies, for example a marker.
    None,
}

impl Status {
    /// The status to show for a box that contains boxes with these
    /// statuses: error beats running, running beats ok.
    pub fn worst<'a>(statuses: impl IntoIterator<Item = &'a Status>) -> Status {
        let mut worst = Status::None;
        for status in statuses {
            if status.rank() > worst.rank() {
                worst = *status;
            }
        }
        worst
    }

    fn rank(self) -> u8 {
        match self {
            Status::None => 0,
            Status::Ok => 1,
            Status::Running => 2,
            Status::Error => 3,
        }
    }
}
