//! The capture store and the local proxy.

pub mod proxy;
pub mod store;

pub use proxy::{CaptureSink, PROXY_PORT, Proxy, ProxyError, UPSTREAM};
pub use store::{CaptureStore, Loaded, SessionCaptures, StoreError, UNKNOWN_SESSION, is_valid_key};
