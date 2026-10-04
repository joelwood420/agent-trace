//! The record types: one [`CaptureRecord`] per HTTP exchange.

use serde::{Deserialize, Serialize};

/// One HTTP exchange between a harness and a model API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureRecord {
    /// Stable id of this record, unique within a capture store.
    pub id: String,
    /// When the request was received, milliseconds since 1970-01-01 UTC.
    pub started_at_ms: i64,
    /// When the first response byte arrived, if any did.
    pub first_byte_at_ms: Option<i64>,
    /// When the response finished, if it did.
    pub ended_at_ms: Option<i64>,
    /// The request as sent.
    pub request: CapturedRequest,
    /// The response, if the upstream answered.
    pub response: Option<CapturedResponse>,
    /// The API message id (`msg_...`) of the reply, when known.
    pub message_id: Option<String>,
    /// A failure description, for example an unreachable upstream or an error event.
    pub error: Option<String>,
}

/// The request half of a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedRequest {
    /// HTTP method, for example `POST`.
    pub method: String,
    /// Request path and query, for example `/v1/messages`.
    pub path: String,
    /// Filtered headers, see [`crate::filter_headers`].
    pub headers: Vec<Header>,
    /// The request body.
    pub body: Body,
}

/// The response half of a record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedResponse {
    /// HTTP status code.
    pub status: u16,
    /// Filtered headers, see [`crate::filter_headers`].
    pub headers: Vec<Header>,
    /// The raw event stream text when the response was streamed.
    pub stream: Option<String>,
    /// The complete message: the JSON body, or the message rebuilt from the stream.
    pub message: Option<serde_json::Value>,
}

/// One HTTP header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    /// Lowercased header name.
    pub name: String,
    /// Header value, or [`crate::OMITTED`] when the value is not kept.
    pub value: String,
}

/// A request body, kept as exactly as it can be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Body {
    /// No body.
    Empty,
    /// A body that parsed as JSON, with key order kept.
    Json(serde_json::Value),
    /// Any other body, as lossy UTF-8 text.
    Text(String),
}

impl Body {
    /// Classify raw bytes: empty, JSON, or text.
    pub fn from_bytes(bytes: &[u8]) -> Body {
        if bytes.is_empty() {
            return Body::Empty;
        }
        match serde_json::from_slice(bytes) {
            Ok(value) => Body::Json(value),
            Err(_) => Body::Text(String::from_utf8_lossy(bytes).into_owned()),
        }
    }

    /// The JSON value, if the body is JSON.
    pub fn json(&self) -> Option<&serde_json::Value> {
        match self {
            Body::Json(value) => Some(value),
            _ => None,
        }
    }
}

impl CaptureRecord {
    /// A request header value by name, ignoring case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.request
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_kinds() {
        assert_eq!(Body::from_bytes(b""), Body::Empty);
        assert_eq!(
            Body::from_bytes(b"{\"b\":1,\"a\":2}")
                .json()
                .map(|v| v.to_string()),
            Some("{\"b\":1,\"a\":2}".into()),
            "key order is kept"
        );
        assert_eq!(Body::from_bytes(b"not json"), Body::Text("not json".into()));
    }

    #[test]
    fn record_round_trips_through_json() {
        let record = CaptureRecord {
            id: "1".into(),
            started_at_ms: 1,
            first_byte_at_ms: Some(2),
            ended_at_ms: Some(3),
            request: CapturedRequest {
                method: "POST".into(),
                path: "/v1/messages".into(),
                headers: vec![Header {
                    name: "x-claude-code-session-id".into(),
                    value: "s1".into(),
                }],
                body: Body::Json(serde_json::json!({ "model": "m" })),
            },
            response: None,
            message_id: None,
            error: Some("upstream unreachable".into()),
        };
        let text = serde_json::to_string(&record).expect("json");
        assert_eq!(
            serde_json::from_str::<CaptureRecord>(&text).expect("parse"),
            record
        );
        assert_eq!(record.header("X-Claude-Code-Session-Id"), Some("s1"));
    }
}
