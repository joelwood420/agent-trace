//! How captured API traffic maps onto Claude Code sessions and model calls.
//! These helpers take plain strings so this crate needs no capture types.

/// The request header Claude Code sets to the session id (the transcript file name).
pub const SESSION_HEADER: &str = "x-claude-code-session-id";

/// The capture store key for a Claude Code session id header value, if it is a
/// plain name (1 to 128 characters of `A-Za-z0-9_-`).
pub fn session_key(header_value: Option<&str>) -> Option<String> {
    let value = header_value?;
    let plain = (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    plain.then(|| value.to_string())
}

/// The trace id of the model call a response message id belongs to.
pub fn model_call_id(message_id: &str) -> String {
    format!("model:{message_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_accepts_plain_ids_only() {
        assert_eq!(
            session_key(Some("00000000-0000-4000-8000-000000000002")).as_deref(),
            Some("00000000-0000-4000-8000-000000000002")
        );
        assert_eq!(session_key(Some("../x")), None);
        assert_eq!(session_key(Some("")), None);
        assert_eq!(session_key(None), None);
        assert_eq!(session_key(Some(&"a".repeat(129))), None);
    }

    #[test]
    fn model_call_ids_match_the_parser() {
        assert_eq!(model_call_id("msg_1"), "model:msg_1");
    }
}
