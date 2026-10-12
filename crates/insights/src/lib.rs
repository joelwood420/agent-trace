//! What fills a model call's context: measure a request or a trace, scale to
//! tokens, explain the biggest costs. Nothing here knows which harness
//! produced the data; harness rules come in as `ContextRules`. Also flags
//! waste and problems in a trace (`session_flags`), with rules as `FlagRules`, compares two runs (`compare`) and lines two runs up step by step (`align`).

pub mod advice;
pub mod align;
pub mod breakdown;
pub mod compare;
pub mod flags;
pub mod measure;
mod request;
pub mod rules;
mod transcript;

pub use advice::{Advice, AdviceLevel};
pub use align::{
    AlignRow, Alignment, DiffPoint, MAX_CELLS, PromptPair, PromptSide, PromptSteps, RowKind,
    RunSteps, Step, align_steps, run_steps,
};
pub use breakdown::{
    BarSlice, CHARS_PER_TOKEN, ContextBar, ContextBreakdown, Item, Slice, breakdown,
};
pub use compare::{
    CallPoint, Comparison, FLAG_KINDS, Higher, KindCount, Named, Presence, Row, RunSummary, Scope,
    Section, Total, Unit, compare, first_main_call, prompt_in_main_run, run_summary,
};
pub use flags::{Flag, FlagKind, LARGE_RESULT_TOKENS, LOOP_WARN, REPEAT_READ_WARN, session_flags};
pub use measure::{
    ContextMeasure, ContextSource, IMAGE_CHARS, MeasuredItem, SliceKind, json_chars, text_chars,
};
pub use request::{measure_request, tool_group};
pub use rules::{
    AlignRules, ContextRules, FileEditRule, FileReadRule, FlagRules, LabelRule, TargetRule,
};
pub use transcript::{measure_transcript, measure_transcript_all};
