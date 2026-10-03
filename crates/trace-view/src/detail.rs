//! Full content of one trace node, for a details panel.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;
use trace_core::{Node, RawSource, Trace};

use crate::build::duration;

/// Everything known about one trace node. The diagram only carries short
/// labels; the UI asks for this when the user selects a box.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeDetail {
    /// The trace node's id.
    pub trace_id: String,
    /// Its parent's trace id, or `None` for a root run.
    pub parent_id: Option<String>,
    /// Trace node kind: `"run"`, `"turn"`, `"model_call"`, `"tool_call"`
    /// or `"marker"`.
    pub kind: String,
    /// Start, milliseconds since the Unix epoch.
    pub started_at_ms: Option<i64>,
    /// End, milliseconds since the Unix epoch.
    pub ended_at_ms: Option<i64>,
    /// `ended_at_ms - started_at_ms`, when both are known.
    pub duration_ms: Option<i64>,
    /// For a model call, its total context size (see `Usage::context_tokens`).
    pub context_tokens: Option<u64>,
    /// The node's full kind-specific data, exactly as in the trace schema
    /// (`docs/SCHEMA.md`, the `node` field): prompt, model output, stop
    /// reason and usage, tool input and result, or marker kind and summary.
    pub node: Node,
    /// Harness-specific extras.
    pub metadata: BTreeMap<String, Value>,
    /// The original source records, in order.
    pub raw: Vec<RawSource>,
}

/// Full content of the trace node with this id, or `None` if there is none.
pub fn node_detail(trace: &Trace, trace_id: &str) -> Option<NodeDetail> {
    let event = trace.get(trace_id)?;
    let context_tokens = match &event.node {
        Node::ModelCall(call) => call.usage.and_then(|u| u.context_tokens()),
        _ => None,
    };
    Some(NodeDetail {
        trace_id: event.id.clone(),
        parent_id: event.parent_id.clone(),
        kind: event.node.kind_name().to_string(),
        started_at_ms: event.started_at_ms,
        ended_at_ms: event.ended_at_ms,
        duration_ms: duration(event),
        context_tokens,
        node: event.node.clone(),
        metadata: event.metadata.clone(),
        raw: event.raw.clone(),
    })
}
