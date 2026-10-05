//! The capture store and the local proxy.

pub mod store;

pub use store::{CaptureStore, Loaded, SessionCaptures, StoreError, UNKNOWN_SESSION, is_valid_key};
