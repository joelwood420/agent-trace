//! What fills a model call's context: measure a request or a trace, scale to
//! tokens, explain the biggest costs. Nothing here knows which harness
//! produced the data; harness rules come in as `ContextRules`.

pub mod measure;
mod request;
pub mod rules;

pub use measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, MeasuredItem, SliceKind, json_chars, text_chars,
};
pub use request::measure_request;
pub use rules::{ContextRules, FileReadRule, LabelRule};
