//! What fills a model call's context: measure a request or a trace, scale to
//! tokens, explain the biggest costs. Nothing here knows which harness
//! produced the data; harness rules come in as `ContextRules`. Also flags
//! waste and problems in a trace (`session_flags`), with rules as `FlagRules`, and compares two runs (`compare`).

pub mod advice;
pub mod breakdown;
pub mod compare;
pub mod flags;
pub mod measure;
mod request;
pub mod rules;
mod transcript;

pub use advice::{Advice, AdviceLevel};
pub use breakdown::{
    BarSlice, CHARS_PER_TOKEN, ContextBar, ContextBreakdown, Item, Slice, breakdown,
};
pub use compare::{
    CallPoint, FLAG_KINDS, KindCount, Named, RunSummary, Scope, Total, first_main_call,
    prompt_in_main_run, run_summary,
};
pub use flags::{Flag, FlagKind, LARGE_RESULT_TOKENS, LOOP_WARN, REPEAT_READ_WARN, session_flags};
pub use measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, MeasuredItem, SliceKind, json_chars, text_chars,
};
pub use request::{measure_request, tool_group};
pub use rules::{ContextRules, FileEditRule, FileReadRule, FlagRules, LabelRule};
pub use transcript::{measure_transcript, measure_transcript_all};
