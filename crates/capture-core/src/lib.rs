//! The capture format: one record per HTTP exchange between a harness and a
//! model API, plus helpers to rebuild a streamed response and compare two
//! requests. Pure logic: no I/O and nothing specific to one harness.

mod diff;
mod headers;
mod record;
mod request;
mod stream;

pub use diff::{MessageSummary, RequestDiff, SettingChange, diff, summarise_message};
pub use headers::{OMITTED, filter_headers, is_credential};
pub use record::{Body, CaptureRecord, CapturedRequest, CapturedResponse, Header};
pub use request::{MESSAGES_KEY, RequestSummary, SYSTEM_KEY, TOOLS_KEY, content_hash, summarise};
pub use stream::{Rebuilt, rebuild_message};
