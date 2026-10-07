//! What fills a model call's context: measure a request or a trace, scale to
//! tokens, explain the biggest costs. Nothing here knows which harness
//! produced the data; harness rules come in as `ContextRules`.

pub mod measure;
pub mod rules;

pub use measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, MeasuredItem, SliceKind, json_chars, text_chars,
};
pub use rules::{ContextRules, FileReadRule, LabelRule};
