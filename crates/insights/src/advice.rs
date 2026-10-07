//! Plain-language advice about what fills a call's context.

use serde::Serialize;

use crate::measure::SliceKind;

/// How much attention a piece of advice deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdviceLevel {
    /// Worth acting on.
    Warn,
    /// Good to know.
    Info,
}

/// One piece of advice, tied to the slice it is about.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advice {
    /// How much attention it deserves.
    pub level: AdviceLevel,
    /// The slice it is about.
    pub slice: SliceKind,
    /// The advice in plain words.
    pub text: String,
}
