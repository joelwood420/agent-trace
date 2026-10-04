//! The capture format: one record per HTTP exchange between a harness and a
//! model API, plus helpers to rebuild a streamed response and compare two
//! requests. Pure logic: no I/O and nothing specific to one harness.

mod headers;
mod record;
mod stream;

pub use headers::{OMITTED, filter_headers, is_credential};
pub use record::{Body, CaptureRecord, CapturedRequest, CapturedResponse, Header};
pub use stream::{Rebuilt, rebuild_message};
