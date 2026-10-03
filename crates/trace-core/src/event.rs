//! The trace event types. See `docs/SCHEMA.md` for the readable description.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One node in the trace tree, as sent by a harness or an adapter.
///
/// A trace is a stream of these events. Each event describes one node. If an
/// event arrives with an `id` that was already seen, it replaces the earlier
/// version of that node (for example, a tool call re-sent with its result).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Stable identifier for this node. Unique within one trace.
    pub id: String,
    /// The `id` of the parent node, or `None` for a root `Run`.
    /// The parent must be sent before its children.
    pub parent_id: Option<String>,
    /// What kind of node this is, with its kind-specific data.
    pub node: Node,
    /// When the node started, in milliseconds since the Unix epoch (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    /// When the node finished, in milliseconds since the Unix epoch (UTC).
    /// `None` while the node is still running or if the end is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    /// The original source records this node was built from, in order.
    /// A harness that emits events directly may leave this empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<RawSource>,
    /// Harness-specific extra data that has no place in the core types.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// The kind of a trace node and its kind-specific data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    /// A whole agent session, or a subagent run nested under a `ToolCall`.
    Run(Run),
    /// One user prompt and everything the agent did in response.
    Turn(Turn),
    /// One request to a model and its response.
    ModelCall(ModelCall),
    /// One tool use requested by a model call, with its result once known.
    ToolCall(ToolCall),
    /// Something notable that is not a model or tool call, such as context
    /// compaction, a hook running, an interrupt, or an API error.
    Marker(Marker),
}

impl Node {
    /// The kind name used in JSON, for example `"model_call"`.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Node::Run(_) => "run",
            Node::Turn(_) => "turn",
            Node::ModelCall(_) => "model_call",
            Node::ToolCall(_) => "tool_call",
            Node::Marker(_) => "marker",
        }
    }
}

/// A whole agent session or a subagent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Name of the harness that produced the run, for example `"claude-code"`.
    pub harness: String,
    /// Optional human-readable title for the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One user prompt and everything the agent did in response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// The prompt that started this turn.
    pub prompt: Vec<ContentBlock>,
}

/// One request to a model and its response.
///
/// The tool calls this response asked for are its children. All tool calls
/// under the same model call were requested together and may run in parallel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCall {
    /// Model identifier as reported by the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Text and thinking produced by the model, in order. Tool uses are not
    /// included here; they are child `ToolCall` nodes.
    #[serde(default)]
    pub output: Vec<ContentBlock>,
    /// Why the model stopped generating, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Token counts for this call, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// One tool use requested by a model call.
///
/// If the tool started a subagent, the subagent's `Run` is a child of this node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name, for example `"Read"` or `"Bash"`.
    pub name: String,
    /// The input the model passed to the tool, as JSON.
    pub input: Value,
    /// The tool's result. `None` until the tool has finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResult>,
}

/// The result of a tool call, as returned to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The result content returned to the model.
    pub content: Vec<ContentBlock>,
    /// `true` if the tool reported an error.
    #[serde(default)]
    pub is_error: bool,
}

/// Something notable that is not a model call or tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Short machine-friendly label, for example `"compaction"`, `"hook"`,
    /// `"interrupt"` or `"api_error"`. Harnesses may define their own.
    pub kind: String,
    /// One-line human-readable description.
    pub summary: String,
}

/// A piece of content in a prompt, model output, or tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// The model's visible reasoning.
    Thinking {
        /// The reasoning text. May be empty if the provider redacted it.
        text: String,
    },
    /// An image. The image data itself is not kept in the trace.
    Image {
        /// Media type, for example `"image/png"`, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// Content of a kind this schema does not model. See the node's `raw`.
    Other {
        /// The source's name for this content kind.
        kind: String,
    },
}

/// Why a model stopped generating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its reply.
    EndTurn,
    /// The model stopped to have tools run.
    ToolUse,
    /// The output hit the token limit.
    MaxTokens,
    /// The output hit a stop sequence.
    StopSequence,
    /// Any other reason, with the provider's own name for it.
    Other(String),
}

/// Token counts for one model call. Any count may be unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens that were neither read from nor written to a cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Tokens generated by the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl Usage {
    /// Total input context sent to the model: uncached input plus cache reads
    /// plus cache writes. `None` if none of these counts is known.
    pub fn context_tokens(&self) -> Option<u64> {
        let parts = [
            self.input_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ];
        if parts.iter().all(Option::is_none) {
            return None;
        }
        Some(parts.iter().flatten().sum())
    }
}

/// One original record a node was built from, kept so the UI can always show
/// the source data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSource {
    /// Where the record came from, for example a transcript file name.
    pub source: String,
    /// 1-based line number within the source.
    pub line: u64,
    /// The record exactly as read.
    pub text: String,
}
