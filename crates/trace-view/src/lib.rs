//! Turns a `trace_core::Trace` into a diagram model that a UI can draw as is.
//!
//! All decisions about what the diagram shows live here: which nodes are
//! grouped as parallel, which runs of similar calls are summarised, which
//! nodes start collapsed, and what each node's label says. The UI only lays
//! out and draws the result. See `docs/VIEW-MODEL.md` for the JSON shape.
//!
//! This crate is harness-agnostic. It only knows the `trace-core` schema.
//! It does no I/O.

mod build;
mod detail;
mod model;
mod text;

pub use build::{SUMMARY_MIN_CALLS, build_session};
pub use detail::{NodeDetail, node_detail};
pub use model::{DiagramNode, NodeKind, PromptDiagram, PromptTotals, SessionDiagram, Status};
