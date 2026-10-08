//! Harness-agnostic trace schema: Run > Turn > ModelCall > ToolCall.
//!
//! This crate holds only the schema. It has no I/O and nothing specific to
//! any one agent harness. See `docs/SCHEMA.md` for the readable description.

mod event;
mod trace;

pub use event::{
    ContentBlock, ContextPart, ContextPartKind, ContextUpdate, Marker, ModelCall, Node, RawSource,
    Run, StopReason, ToolCall, ToolResult, TraceEvent, Turn, Usage,
};
pub use trace::{Trace, TraceError};
