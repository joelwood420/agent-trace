//! Header filtering. Credentials are dropped, a short allowlist keeps its
//! values, and everything else is kept by name only.

use crate::record::Header;

/// Value stored for a header whose name is kept but whose value is not.
pub const OMITTED: &str = "<omitted>";

const KEEP_EXACT: [&str; 6] = [
    "user-agent",
    "content-type",
    "x-app",
    "x-claude-code-session-id",
    "request-id",
    "retry-after",
];

/// True for a header that can carry a credential: `authorization`,
/// `proxy-authorization`, `x-api-key`, `cookie`, `set-cookie`, or any name
/// containing `token`, `secret`, `auth`, `key`, `password` or `credential`.
/// Case-insensitive. Names starting `anthropic-ratelimit-` are exempt from the
/// substring rules, since they carry only numbers and timestamps (for example
/// `anthropic-ratelimit-tokens-remaining`). This is checked before the allowlist, so
/// `anthropic-api-key` is a credential.
pub fn is_credential(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name.starts_with("anthropic-ratelimit-") {
        return false;
    }
    matches!(
        name.as_str(),
        "authorization" | "proxy-authorization" | "x-api-key" | "cookie" | "set-cookie"
    ) || ["token", "secret", "auth", "key", "password", "credential"]
        .iter()
        .any(|word| name.contains(word))
}

/// Filter headers for storage. Names are lowercased and input order is kept.
/// Credential headers are dropped entirely. Names starting `anthropic-` or
/// `x-stainless-` and a few others keep their value. All other headers are
/// kept with the value [`OMITTED`].
pub fn filter_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Header> {
    headers
        .into_iter()
        .filter(|(name, _)| !is_credential(name))
        .map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            let keep = name.starts_with("anthropic-")
                || name.starts_with("x-stainless-")
                || KEEP_EXACT.contains(&name.as_str());
            Header {
                value: if keep {
                    value.to_string()
                } else {
                    OMITTED.to_string()
                },
                name,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_headers_are_dropped() {
        let kept = filter_headers([
            ("Authorization", "Bearer secret-1"),
            ("x-api-key", "secret-2"),
            ("Cookie", "a=secret-3"),
            ("x-session-token", "secret-4"),
            ("x-client-secret", "secret-5"),
            ("anthropic-version", "2023-06-01"),
        ]);
        let text = format!("{kept:?}");
        for secret in ["secret-1", "secret-2", "secret-3", "secret-4", "secret-5"] {
            assert!(!text.contains(secret), "{secret} leaked");
        }
        assert_eq!(
            kept,
            [Header {
                name: "anthropic-version".into(),
                value: "2023-06-01".into()
            }]
        );
    }

    #[test]
    fn odd_casings_and_names_are_credentials() {
        let kept = filter_headers([
            ("X-API-KEY", "secret-a"),
            ("PROXY-AUTHORIZATION", "secret-b"),
            ("anthropic-api-key", "secret-c"),
            ("x-goog-api-key", "secret-d"),
            ("x-password", "secret-e"),
            ("x-credentials", "secret-f"),
            ("content-type", "application/json"),
        ]);
        assert_eq!(
            kept,
            [Header {
                name: "content-type".into(),
                value: "application/json".into()
            }]
        );
        assert!(is_credential("X-Api-Key"));
        assert!(!is_credential("anthropic-version"));
    }

    #[test]
    fn rate_limit_headers_keep_their_values() {
        let kept = filter_headers([
            ("anthropic-ratelimit-tokens-remaining", "1000"),
            ("anthropic-ratelimit-input-tokens-limit", "5000"),
            ("anthropic-api-key", "secret-z"),
        ]);
        assert_eq!(
            kept,
            [
                Header {
                    name: "anthropic-ratelimit-tokens-remaining".into(),
                    value: "1000".into()
                },
                Header {
                    name: "anthropic-ratelimit-input-tokens-limit".into(),
                    value: "5000".into()
                }
            ]
        );
    }

    #[test]
    fn allowlisted_values_are_kept_and_others_omitted() {
        let kept = filter_headers([
            ("User-Agent", "claude-cli/0.0.0"),
            ("x-stainless-os", "Windows"),
            (
                "X-Claude-Code-Session-Id",
                "00000000-0000-4000-8000-000000000002",
            ),
            ("x-forwarded-for", "10.0.0.1"),
        ]);
        assert_eq!(
            kept[0],
            Header {
                name: "user-agent".into(),
                value: "claude-cli/0.0.0".into()
            }
        );
        assert_eq!(kept[1].value, "Windows");
        assert_eq!(kept[2].value, "00000000-0000-4000-8000-000000000002");
        assert_eq!(
            kept[3],
            Header {
                name: "x-forwarded-for".into(),
                value: OMITTED.into()
            }
        );
    }
}
