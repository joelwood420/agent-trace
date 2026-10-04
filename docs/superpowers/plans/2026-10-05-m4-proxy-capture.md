# M4 Proxy Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture every API request and response of an opt-in Claude Code session through a local proxy, store them, join them to model calls, and show the full request, the changes since the previous call, and a session overview.

**Architecture:** A new pure crate `capture-core` defines the capture record, header filtering, rebuilding a message from a response stream, request analysis and the diff. A new crate `capture` holds the on-disk store and the proxy (hyper server on `127.0.0.1:47821`, reqwest client to the fixed upstream `https://api.anthropic.com`). The app starts the proxy, saves each record, tells the open live session, and serves overview and detail commands. The UI shows the three views.

**Tech Stack:** Rust stable, tokio, hyper 1 + hyper-util + http-body-util (server), reqwest 0.13 with rustls on the `ring` provider (client), flate2 (gzip), sha2 (hashing), Tauri 2 commands and channels, React + TypeScript.

**Spec:** `docs/superpowers/specs/2026-10-05-m4-proxy-capture-design.md`. Read it before starting any task.

## Global Constraints

- Every command must work in PowerShell on native Windows. No bash-only syntax in docs or scripts.
- Never write to, move, or delete anything under the user's `.claude` folder. The only folder the app writes to is its own captures folder. Tests use temp folders and `fixtures/` only.
- No real request, response, prompt, token, path or username is ever committed. Capture fixtures are invented.
- Credential headers (`authorization`, `x-api-key`, `cookie`, `set-cookie`, and any header name containing `token`, `secret` or `auth`) are never stored, logged, returned by a command, or shown. They are still forwarded upstream unchanged.
- The proxy listens on `127.0.0.1` only, port `47821`, and forwards only to `https://api.anthropic.com`. Only unit tests inside the `capture` crate may use another upstream.
- A failure to save a capture never affects forwarding.
- No `unwrap()` or `expect()` outside tests (integration test files start with `#![allow(clippy::expect_used)]`).
- No em dashes anywhere: code, comments, docs, commit messages.
- `trace-core` gets no changes. If a task seems to need one, stop and report back.
- Parse defensively: unknown data is kept as raw text and shown raw, never a panic.
- Doc comments on all public items of `capture-core` (the format is a contract) and on all new public items elsewhere.
- Every new dependency is added to the dependency table in `docs/DECISIONS.md` with the reason.
- Before committing, all of these must pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - in `ui`: `npm run lint; npm run typecheck; npm test; npm run build` (for tasks touching `ui/` or the generated mock data)
- Commit with a clear message for a public reader, ending with the Co-Authored-By trailer your session's attribution instructions give. Never push.

## Decisions taken while planning (not in the spec)

Each task that implements one records it in `docs/DECISIONS.md`.

1. **Captures folder:** Tauri's app data folder for the app identifier `dev.snitchcraft.app`, so `%APPDATA%\dev.snitchcraft.app\captures\` on Windows (the spec's `%APPDATA%\snitchcraft\` was illustrative). Task 8 updates the spec line.
2. **serde_json keeps key order.** The workspace enables serde_json's `preserve_order` feature, so a captured request body keeps its exact key order (tool input schemas are shown to the model in that order). This changes the order of keys in JSON the app already produces (metadata maps) but not their content; existing snapshot tests compare JSON values, which stay equal.
3. **TLS backend:** reqwest with `rustls-no-provider` plus rustls's `ring` provider, installed once at proxy start. The default `aws-lc` provider can need CMake and NASM on Windows, which this machine does not have.
4. **Store layout:** `captures/<session key>/calls.jsonl.gz` is a gzip file with one gzip member per record (appending a member is valid gzip and needs no rewrite). `captures/<session key>/blobs/<sha256>.json.gz` holds each distinct system prompt and tool set. In a stored record those two body fields are replaced in place by `{"snitchcraft_blob": "<sha256>"}`, and the record lists which fields were replaced, so they are restored at the same position.
5. **Diff ignores `cache_control`.** Claude Code moves the cache marker to the newest message on every call, so message equality for the diff ignores every `cache_control` key. The full request view still shows the markers.
6. **Previous call pairing** lives in `trace-view` (`previous_model_call`): the previous model call within the same run, skipping nested subagent runs. It is harness-agnostic trace logic.
7. **Records are read on demand.** The open session keeps only a small index of its captures in memory; details are read from the store, with the last session's decoded records cached until a new record arrives.
8. **Confirming Delete uses an in-page confirmation**, not a browser dialog.

## Review Focus

- **A secret slipping into storage or a command response:** an `Authorization` or `x-api-key` header, or a header name containing `token`, must not appear anywhere written or returned. Pinned by Task 1 (`credential_headers_are_dropped`) and Task 4 (`stored_bytes_never_contain_the_credential`).
- **The proxy changing what Claude Code receives or slowing it:** bytes must arrive unchanged and chunk by chunk, before the upstream finishes. Pinned by Task 4 (`streamed_response_arrives_in_pieces_and_unchanged`).
- **Moving cache markers making every call look fully changed:** consecutive normal calls must show a long shared prefix. Pinned by Task 2 (`cache_marker_moves_do_not_count_as_changes`).
- **Subagent calls compared with the main agent's calls:** the previous call must be the same agent's. Pinned by Task 5 (`previous_model_call_skips_subagent_runs`).
- **A damaged capture file breaking the session view:** a truncated last record must be skipped and reported, earlier records kept. Pinned by Task 3 (`truncated_last_record_is_skipped`).

---

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `Cargo.toml` (workspace) | 1, 3, 4 | new members, workspace deps, serde_json `preserve_order` |
| `crates/capture-core/src/{lib,record,headers,stream}.rs` | 1 | record types, header filter, stream rebuild |
| `crates/capture-core/src/{request,diff}.rs` | 2 | request analysis, hashing, diff |
| `docs/CAPTURE-FORMAT.md` | 1, 2 | readable description of the format |
| `crates/capture/src/{lib,store}.rs` | 3 | on-disk store |
| `crates/capture/src/proxy.rs` | 4 | the proxy |
| `crates/trace-view/src/order.rs` | 5 | `previous_model_call` |
| `crates/adapter-claude-code/src/capture.rs` | 5 | Claude Code session key and model call id mapping |
| `crates/capture/examples/make_fixture.rs`, `fixtures/captures/basic/calls.jsonl` | 6 | invented capture fixture |
| `src-tauri/src/captures.rs` | 7 | overview and detail assembly, capture index |
| `src-tauri/src/{live,sessions,main,watch}.rs`, `build.rs`, capabilities | 7, 8 | integration, commands, proxy start |
| `ui/src/mock/capture-data.json` (generated) | 7 | mock data for the views |
| `ui/src/{types,api,captures}.ts`, `ui/src/captures.test.ts`, `ui/src/mock/mockApi.ts` | 9 | UI types, API, pure helpers, mock |
| `ui/src/components/*`, `ui/src/app.css`, `ui/src/App.tsx` | 10 | the three views, markers, start command, Delete |
| docs, README, CLAUDE.md | 11 | coordinator |

---

### Task 1: `capture-core` record format, header filter and stream rebuild

**Files:**
- Create: `crates/capture-core/Cargo.toml`, `crates/capture-core/src/lib.rs`, `src/record.rs`, `src/headers.rs`, `src/stream.rs`
- Create: `docs/CAPTURE-FORMAT.md`
- Modify: `Cargo.toml` (workspace members; workspace dep `capture-core = { path = "crates/capture-core" }`; serde_json gets `features = ["preserve_order"]`)
- Modify: `docs/DECISIONS.md`

**Interfaces:**
- Produces:
  ```rust
  // record.rs
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct CaptureRecord {
      pub id: String,
      pub started_at_ms: i64,
      pub first_byte_at_ms: Option<i64>,
      pub ended_at_ms: Option<i64>,
      pub request: CapturedRequest,
      pub response: Option<CapturedResponse>,
      pub message_id: Option<String>,
      pub error: Option<String>,
  }
  pub struct CapturedRequest { pub method: String, pub path: String, pub headers: Vec<Header>, pub body: Body }
  pub struct CapturedResponse { pub status: u16, pub headers: Vec<Header>, pub stream: Option<String>, pub message: Option<serde_json::Value> }
  pub struct Header { pub name: String, pub value: String }
  #[serde(tag = "kind", content = "value", rename_all = "snake_case")]
  pub enum Body { Empty, Json(serde_json::Value), Text(String) }
  impl Body { pub fn from_bytes(bytes: &[u8]) -> Body; pub fn json(&self) -> Option<&serde_json::Value>; }
  impl CaptureRecord { pub fn header(&self, name: &str) -> Option<&str>; }  // request header, case-insensitive

  // headers.rs
  pub const OMITTED: &str = "<omitted>";
  pub fn filter_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Header>;
  pub fn is_credential(name: &str) -> bool;

  // stream.rs
  #[derive(Debug, Clone, PartialEq, Default)]
  pub struct Rebuilt { pub message: Option<serde_json::Value>, pub message_id: Option<String>, pub error: Option<String>, pub unknown_events: Vec<String> }
  pub fn rebuild_message(content_type: Option<&str>, body: &str) -> Rebuilt;
  ```
  All types derive `Debug, Clone, PartialEq, Serialize, Deserialize`. Field names serialise as written (snake_case).

Rules:
- `filter_headers`: names are lowercased. Credential headers (`is_credential`: name is `authorization`, `proxy-authorization`, `x-api-key`, `cookie` or `set-cookie`, or contains `token`, `secret` or `auth`) are dropped entirely. Kept with value: names starting `anthropic-` or `x-stainless-`, and exactly `user-agent`, `content-type`, `x-app`, `x-claude-code-session-id`, `request-id`, `retry-after`. Rate-limit headers (`anthropic-ratelimit-*` already covered). Every other header is kept with value `<omitted>`. Order of input is kept.
- `Body::from_bytes`: empty slice gives `Empty`; valid JSON gives `Json`; otherwise `Text` (lossy UTF-8).
- `rebuild_message`:
  - If content type starts with `text/event-stream`: parse events separated by blank lines; each event has an `event:` line and one or more `data:` lines (join data lines with `\n`). Handle CRLF.
    - `message_start`: message = `data.message`.
    - `content_block_start`: set `content[index]` = `data.content_block` (extend the array as needed).
    - `content_block_delta`: by `delta.type`: `text_delta` appends `delta.text` to the block's `text`; `thinking_delta` appends `delta.thinking` to `thinking`; `signature_delta` sets `signature`; `input_json_delta` appends `delta.partial_json` to a side buffer for that index; `citations_delta` pushes `delta.citation` onto the block's `citations` array; any other delta type is pushed to `unknown_events` as `content_block_delta:<type>`.
    - `content_block_stop`: if the side buffer for that index is non-empty, parse it as JSON into the block's `input` (on parse failure set `input` to the raw string and add `content_block_stop:bad_json` to `unknown_events`).
    - `message_delta`: copy every key of `data.delta` onto the message (for example `stop_reason`, `stop_sequence`); merge every key of `data.usage` into `message.usage`.
    - `message_stop`, `ping`: nothing.
    - `error`: `error` = the `data.error.message` string, or the raw data.
    - any other event name: push it to `unknown_events`.
    - `message_id` = `message.id` when it is a string.
  - Otherwise, if the body is JSON: `message` = the JSON; `message_id` = its `id` if it is a string starting with `msg_`; if it has `type: "error"`, `error` = `error.message`.
  - Otherwise: everything `None`.

- [ ] **Step 1: Create the crate skeleton** with `Cargo.toml`:

```toml
[package]
name = "capture-core"
description = "Harness-agnostic format for captured model API calls, with stream rebuild and request diff."
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true

[lints]
workspace = true
```

Add it to the workspace `members`, add the workspace dependency, and enable `preserve_order` on the workspace `serde_json` dependency (`serde_json = { version = "1.0.151", features = ["preserve_order"] }`).

- [ ] **Step 2: Run the existing workspace tests** (`cargo test --workspace`) to see whether `preserve_order` changes anything. If a test fails only because of key order, fix the test to compare values, not text. If a generated mock file's text order changes, regenerate it with PowerShell (`$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS`) and check the diff only reorders keys.

- [ ] **Step 3: Write failing tests** (unit tests in each module).

`headers.rs`:
```rust
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
        assert_eq!(kept, [Header { name: "anthropic-version".into(), value: "2023-06-01".into() }]);
    }

    #[test]
    fn allowlisted_values_are_kept_and_others_omitted() {
        let kept = filter_headers([
            ("User-Agent", "claude-cli/0.0.0"),
            ("x-stainless-os", "Windows"),
            ("X-Claude-Code-Session-Id", "00000000-0000-4000-8000-000000000002"),
            ("x-forwarded-for", "10.0.0.1"),
        ]);
        assert_eq!(kept[0], Header { name: "user-agent".into(), value: "claude-cli/0.0.0".into() });
        assert_eq!(kept[1].value, "Windows");
        assert_eq!(kept[2].value, "00000000-0000-4000-8000-000000000002");
        assert_eq!(kept[3], Header { name: "x-forwarded-for".into(), value: OMITTED.into() });
    }
}
```

`stream.rs` (invented streams):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SSE: &str = "event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"test-model\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\r\n\r\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me \"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"look.\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{}}}\n\n\
event: ping\ndata: {\"type\":\"ping\"}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.txt\\\"}\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n\
event: future_event\ndata: {\"type\":\"future_event\"}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":42}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    #[test]
    fn rebuilds_a_streamed_message() {
        let r = rebuild_message(Some("text/event-stream; charset=utf-8"), SSE);
        assert_eq!(r.message_id.as_deref(), Some("msg_test1"));
        assert_eq!(r.unknown_events, ["future_event"]);
        let m = r.message.expect("message");
        assert_eq!(m["stop_reason"], json!("tool_use"));
        assert_eq!(m["usage"], json!({ "input_tokens": 10, "output_tokens": 42 }));
        assert_eq!(m["content"][0], json!({ "type": "thinking", "thinking": "Let me look.", "signature": "sig" }));
        assert_eq!(m["content"][1]["input"], json!({ "path": "a.txt" }));
    }

    #[test]
    fn json_error_body_is_reported() {
        let r = rebuild_message(Some("application/json"), r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        assert_eq!(r.error.as_deref(), Some("Overloaded"));
        assert_eq!(r.message_id, None);
    }

    #[test]
    fn stream_error_event_is_reported() {
        let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"Boom\"}}\n\n";
        assert_eq!(rebuild_message(Some("text/event-stream"), body).error.as_deref(), Some("Boom"));
    }

    #[test]
    fn non_json_body_gives_nothing() {
        assert_eq!(rebuild_message(Some("text/plain"), "hello"), Rebuilt::default());
    }
}
```

`record.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_kinds() {
        assert_eq!(Body::from_bytes(b""), Body::Empty);
        assert_eq!(Body::from_bytes(b"{\"b\":1,\"a\":2}").json().map(|v| v.to_string()), Some("{\"b\":1,\"a\":2}".into()), "key order is kept");
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
                headers: vec![Header { name: "x-claude-code-session-id".into(), value: "s1".into() }],
                body: Body::Json(serde_json::json!({ "model": "m" })),
            },
            response: None,
            message_id: None,
            error: Some("upstream unreachable".into()),
        };
        let text = serde_json::to_string(&record).expect("json");
        assert_eq!(serde_json::from_str::<CaptureRecord>(&text).expect("parse"), record);
        assert_eq!(record.header("X-Claude-Code-Session-Id"), Some("s1"));
    }
}
```

- [ ] **Step 4: Run to see them fail:** `cargo test -p capture-core` (compile errors expected).

- [ ] **Step 5: Implement** `record.rs`, `headers.rs`, `stream.rs` per the rules above, with doc comments on every public item, and `lib.rs`:

```rust
//! The capture format: one record per HTTP exchange between a harness and a
//! model API, plus helpers to rebuild a streamed response and compare two
//! requests. Pure logic: no I/O and nothing specific to one harness.

mod headers;
mod record;
mod stream;

pub use headers::{OMITTED, filter_headers, is_credential};
pub use record::{Body, CaptureRecord, CapturedRequest, CapturedResponse, Header};
pub use stream::{Rebuilt, rebuild_message};
```

- [ ] **Step 6: Run the tests:** `cargo test -p capture-core`, then all checks.

- [ ] **Step 7: Docs.** Create `docs/CAPTURE-FORMAT.md` describing the record (one section per type, a field table each, the header rules, the stream rebuild rules) in the style of `docs/SCHEMA.md`, with a short invented JSON example of one record. Add to `docs/DECISIONS.md` the decisions "Captures live outside the trace schema (chosen by the project owner)", "serde_json keeps key order" (planning decision 2), and "Header allowlist for captures" (the rules above and why credentials are dropped rather than masked).

- [ ] **Step 8: Commit**

```powershell
git add Cargo.toml Cargo.lock crates/capture-core docs/CAPTURE-FORMAT.md docs/DECISIONS.md
git commit -m "Add the capture-core record format, header filter and stream rebuild"
```
(plus any test or snapshot files changed in Step 2; end the message with the trailer)

---

### Task 2: Request analysis, hashing and the diff in `capture-core`

**Files:**
- Create: `crates/capture-core/src/request.rs`, `crates/capture-core/src/diff.rs`
- Modify: `crates/capture-core/src/lib.rs`, `crates/capture-core/Cargo.toml` (add `sha2.workspace = true`), `Cargo.toml` (workspace dep `sha2 = "0.10"`), `docs/CAPTURE-FORMAT.md`, `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `CaptureRecord`, `Body` (Task 1).
- Produces:
  ```rust
  // request.rs
  pub const SYSTEM_KEY: &str = "system";
  pub const TOOLS_KEY: &str = "tools";
  pub const MESSAGES_KEY: &str = "messages";
  /// sha256 hex of the compact JSON text of the value (key order as stored).
  pub fn content_hash(value: &serde_json::Value) -> String;
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct RequestSummary {
      pub model: Option<String>,
      pub system_hash: Option<String>,
      pub tools_hash: Option<String>,
      pub tool_names: Vec<String>,
      pub system_chars: usize,
      pub message_count: usize,
      /// Every top-level body key except system, tools and messages, in body order.
      pub settings: Vec<(String, serde_json::Value)>,
      /// Comma-separated values of the anthropic-beta request header, trimmed.
      pub betas: Vec<String>,
  }
  pub fn summarise(record: &CaptureRecord) -> Option<RequestSummary>;   // None if the body is not a JSON object

  // diff.rs
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct MessageSummary { pub index: usize, pub role: String, pub block_types: Vec<String>, pub chars: usize, pub preview: String }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct SettingChange { pub key: String, pub before: Option<serde_json::Value>, pub after: Option<serde_json::Value> }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct RequestDiff {
      pub shared_prefix: usize,
      pub removed: Vec<MessageSummary>,
      pub added: Vec<MessageSummary>,
      pub system_changed: bool,
      pub tools_changed: bool,
      pub tools_added: Vec<String>,
      pub tools_removed: Vec<String>,
      pub settings_changed: Vec<SettingChange>,
      pub betas_added: Vec<String>,
      pub betas_removed: Vec<String>,
  }
  pub fn diff(previous: &CaptureRecord, next: &CaptureRecord) -> Option<RequestDiff>;  // None if either body is not a JSON object
  pub fn summarise_message(index: usize, message: &serde_json::Value) -> MessageSummary;
  ```

Rules:
- `content_hash`: `serde_json::to_vec(value)` then sha256, lowercase hex.
- `summarise`: `system_chars` = total length of all `text` fields in system blocks, or the string length if `system` is a string. `tool_names` = each tool's `name` (string), in order. `model` from body `model`.
- Message equality for the diff: compare the two messages after removing every `cache_control` key at any depth (planning decision 5). `shared_prefix` = number of leading messages equal under that rule. `removed` = previous messages from `shared_prefix` on; `added` = next messages from `shared_prefix` on. Indexes are positions in their own request.
- `summarise_message`: `role` (or `"?"`); `block_types` = each content block's `type` (a string content gives `["text"]`); `chars` = total length of the JSON text of the content; `preview` = the first text found (string content, or the first `text` block, or for `tool_result` the first text inside it), cut to 160 characters (on a character boundary), with newlines replaced by spaces; empty if none.
- `system_changed` / `tools_changed` by `content_hash` inequality (an absent field counts as a distinct value). `tools_added` / `tools_removed` by name sets, in the order they appear.
- `settings_changed`: union of setting keys in previous order then new keys in next order; include a key if its value differs (absent on one side gives `None`).

- [ ] **Step 1: Write failing tests** in `diff.rs` (build records with a helper):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Body, CaptureRecord, CapturedRequest, Header};
    use serde_json::{Value, json};

    fn record(body: Value, betas: &str) -> CaptureRecord {
        CaptureRecord {
            id: "x".into(),
            started_at_ms: 0,
            first_byte_at_ms: None,
            ended_at_ms: None,
            request: CapturedRequest {
                method: "POST".into(),
                path: "/v1/messages".into(),
                headers: vec![Header { name: "anthropic-beta".into(), value: betas.into() }],
                body: Body::Json(body),
            },
            response: None,
            message_id: None,
            error: None,
        }
    }

    fn user(text: &str) -> Value { json!({ "role": "user", "content": [{ "type": "text", "text": text }] }) }
    fn assistant(text: &str) -> Value { json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] }) }
    fn with_cache(mut m: Value) -> Value { m["content"][0]["cache_control"] = json!({ "type": "ephemeral" }); m }

    fn body(messages: Vec<Value>, tools: &[&str], max_tokens: u64) -> Value {
        json!({
            "model": "test-model",
            "max_tokens": max_tokens,
            "system": [{ "type": "text", "text": "You are a test agent." }],
            "tools": tools.iter().map(|n| json!({ "name": n, "description": "d", "input_schema": { "type": "object" } })).collect::<Vec<_>>(),
            "messages": messages,
        })
    }

    #[test]
    fn plain_growth_adds_messages() {
        let a = record(body(vec![user("one")], &["Read"], 100), "b1");
        let b = record(body(vec![user("one"), assistant("two"), user("three")], &["Read"], 100), "b1");
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 1);
        assert!(d.removed.is_empty());
        assert_eq!(d.added.iter().map(|m| m.preview.as_str()).collect::<Vec<_>>(), ["two", "three"]);
        assert!(!d.system_changed && !d.tools_changed && d.settings_changed.is_empty());
    }

    #[test]
    fn cache_marker_moves_do_not_count_as_changes() {
        let a = record(body(vec![user("one"), with_cache(assistant("two"))], &["Read"], 100), "b1");
        let b = record(body(vec![user("one"), assistant("two"), with_cache(user("three"))], &["Read"], 100), "b1");
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 2);
        assert_eq!(d.added.len(), 1);
    }

    #[test]
    fn compaction_replaces_history_with_a_summary() {
        let a = record(body(vec![user("one"), assistant("two"), user("three")], &["Read"], 100), "b1");
        let b = record(body(vec![user("Summary of the conversation so far"), user("four")], &["Read"], 100), "b1");
        let d = diff(&a, &b).expect("diff");
        assert_eq!(d.shared_prefix, 0);
        assert_eq!(d.removed.len(), 3);
        assert_eq!(d.added[0].preview, "Summary of the conversation so far");
    }

    #[test]
    fn tools_settings_and_betas_changes_are_listed() {
        let a = record(body(vec![user("one")], &["Read", "Grep"], 100), "b1, b2");
        let b = record(body(vec![user("one")], &["Read", "Write"], 200), "b2,b3");
        let d = diff(&a, &b).expect("diff");
        assert!(d.tools_changed);
        assert_eq!(d.tools_added, ["Write"]);
        assert_eq!(d.tools_removed, ["Grep"]);
        assert_eq!(d.settings_changed, [SettingChange { key: "max_tokens".into(), before: Some(json!(100)), after: Some(json!(200)) }]);
        assert_eq!(d.betas_added, ["b3"]);
        assert_eq!(d.betas_removed, ["b1"]);
    }

    #[test]
    fn non_json_bodies_give_no_diff() {
        let mut a = record(json!({}), "");
        a.request.body = Body::Text("x".into());
        let b = record(body(vec![], &[], 1), "");
        assert_eq!(diff(&a, &b), None);
    }
}
```

In `request.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_depends_on_content_and_order() {
        assert_eq!(content_hash(&json!({ "a": 1 })), content_hash(&json!({ "a": 1 })));
        assert_ne!(content_hash(&json!({ "a": 1, "b": 2 })), content_hash(&json!({ "b": 2, "a": 1 })));
        assert_eq!(content_hash(&json!(null)).len(), 64);
    }
}
```
plus a `summarise` test on a record built like the diff helper: model, tool names in order, `system_chars` = 21, settings = `[("model", ..), ("max_tokens", ..)]` in body order, betas trimmed.

- [ ] **Step 2: Run to see them fail:** `cargo test -p capture-core`.
- [ ] **Step 3: Implement** both modules; export from `lib.rs` (`pub use diff::{MessageSummary, RequestDiff, SettingChange, diff, summarise_message}; pub use request::{MESSAGES_KEY, RequestSummary, SYSTEM_KEY, TOOLS_KEY, content_hash, summarise};`).
- [ ] **Step 4: Run tests and all checks.**
- [ ] **Step 5: Docs.** Add a "Comparing requests" section to `docs/CAPTURE-FORMAT.md` (the rules above). Add `sha2` to the DECISIONS dependency table ("content hashes for deduplicating system prompts and tool sets; already in the build through Tauri"). Add decision "The diff ignores cache markers" (planning decision 5).
- [ ] **Step 6: Commit** (`git commit -m "Add request summaries, content hashes and the request diff"` with the trailer).

---

### Task 3: The capture store

**Files:**
- Create: `crates/capture/Cargo.toml`, `crates/capture/src/lib.rs`, `crates/capture/src/store.rs`
- Modify: `Cargo.toml` (member; workspace deps `capture = { path = "crates/capture" }`, `flate2 = "1"`), `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `CaptureRecord`, `Body`, `content_hash`, `SYSTEM_KEY`, `TOOLS_KEY` (Tasks 1-2).
- Produces:
  ```rust
  #[derive(Debug, thiserror::Error)]
  pub enum StoreError { #[error("invalid session key")] InvalidKey, #[error("could not {what}: {source}")] Io { what: &'static str, source: std::io::Error }, #[error("could not encode a capture: {0}")] Encode(serde_json::Error) }
  pub const UNKNOWN_SESSION: &str = "unknown";
  pub fn is_valid_key(key: &str) -> bool;          // 1..=128 chars of [A-Za-z0-9_-]
  #[derive(Debug, Clone, PartialEq)]
  pub struct Loaded { pub records: Vec<CaptureRecord>, pub skipped: Vec<String> }   // skipped: reasons
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct SessionCaptures { pub key: String, pub bytes: u64, pub modified_ms: i64 }
  pub struct CaptureStore { .. }
  impl CaptureStore {
      pub fn new(root: PathBuf) -> Self;            // creates nothing yet
      pub fn root(&self) -> &Path;
      pub fn append(&self, key: &str, record: &CaptureRecord) -> Result<(), StoreError>;
      pub fn load(&self, key: &str) -> Result<Loaded, StoreError>;   // missing folder gives empty Loaded
      pub fn size(&self, key: &str) -> Result<u64, StoreError>;      // total bytes in the folder, 0 if missing
      pub fn delete(&self, key: &str) -> Result<(), StoreError>;     // missing folder is Ok
  }
  ```
  `crates/capture/Cargo.toml` deps: `capture-core`, `serde`, `serde_json`, `thiserror`, `tracing`, `flate2` (workspace). Dev-deps none needed beyond std.

Rules:
- Every method first checks `is_valid_key`; invalid gives `InvalidKey`. Paths are `root.join(key)`; nothing outside `root` is ever touched.
- `append`: create the folder (and `blobs/`) if needed. For each of `system` and `tools` present in a JSON object body: compute `content_hash`, write `blobs/<hash>.json.gz` if it does not exist (write to a temp name in the same folder then rename, so a crash never leaves a half blob), and replace the field value in place with `{"snitchcraft_blob": "<hash>"}`. Wrap as `{"blobs": {"system": "<hash>", ...}, "record": <record>}` (the stored line), serialise as one JSON line plus `\n`, compress it as its own gzip member, and append to `calls.jsonl.gz` (open with append; one write call). Concurrent appends are serialised with a `Mutex<()>` inside the store.
- `load`: read `calls.jsonl.gz` with `flate2::read::MultiGzDecoder`, reading line by line. A line that fails to parse is skipped with a reason. A decode error (for example a cut-off last member) stops reading and adds a reason; records already read are kept. Restore blobs in place for the fields listed in `blobs` (a missing or unreadable blob: leave the placeholder, add a reason).
- `size`: sum of file sizes under the folder (recursive).
- `delete`: `std::fs::remove_dir_all(root.join(key))`.

- [ ] **Step 1: Write failing tests** in `store.rs` (temp folders under `std::env::temp_dir()` with the process id and test name; build records with a helper similar to Task 2's, with a large invented system prompt):

```rust
#[test]
fn round_trip_restores_system_and_tools_in_place() { /* append 2 records, load, assert equal to the originals including key order (compare serde_json::to_string of bodies) */ }

#[test]
fn identical_system_prompts_are_stored_once() { /* append 3 records with the same system and tools, 2 different user messages: blobs dir has exactly 2 files; size() is less than 3 x the uncompressed system text */ }

#[test]
fn truncated_last_record_is_skipped() { /* append 3, then truncate calls.jsonl.gz by 10 bytes; load gives the first 2 records and one skipped reason */ }

#[test]
fn invalid_keys_are_rejected_and_nothing_is_written() { for key in ["", "..", "a/b", "a\\b", "C:", &"x".repeat(129)] { assert!(matches!(store.append(key, &r), Err(StoreError::InvalidKey))); } /* root folder still has no entries */ }

#[test]
fn delete_removes_only_that_session() { /* append to "s1" and "s2", delete "s1": s1 folder gone, s2 loads */ }

#[test]
fn missing_session_loads_empty() { /* load("nope") gives 0 records, size 0 */ }
```
Write each test fully (the comments describe what to assert).

- [ ] **Step 2: Run to see them fail:** `cargo test -p capture`.
- [ ] **Step 3: Implement** `store.rs` and `lib.rs` (`//! The capture store and the local proxy.`, `pub mod store; pub use store::{...};`).
- [ ] **Step 4: Run tests and all checks.**
- [ ] **Step 5: Docs.** DECISIONS: `flate2` in the dependency table ("gzip for stored captures; already in the build through Tauri"); decision "Capture store layout" (planning decision 4, with the reason: appends stay cheap and crash-safe, repeated system prompts and tool sets are stored once).
- [ ] **Step 6: Commit** (`git commit -m "Add the capture store"` with the trailer).

---

### Task 4: The proxy

**Files:**
- Create: `crates/capture/src/proxy.rs`
- Modify: `crates/capture/Cargo.toml`, `crates/capture/src/lib.rs`, `Cargo.toml` (workspace deps), `docs/DECISIONS.md`

**Interfaces:**
- Consumes: `CaptureRecord`, `CapturedRequest`, `CapturedResponse`, `Body`, `filter_headers`, `rebuild_message` (Task 1).
- Produces:
  ```rust
  pub const PROXY_PORT: u16 = 47821;
  pub const UPSTREAM: &str = "https://api.anthropic.com";
  pub trait CaptureSink: Send + Sync + 'static { fn record(&self, record: CaptureRecord); }
  #[derive(Debug, thiserror::Error)]
  pub enum ProxyError { #[error("could not listen on 127.0.0.1:{port}: {source}")] Bind { port: u16, source: std::io::Error }, #[error("could not set up the HTTPS client: {0}")] Client(String) }
  pub struct Proxy { .. }
  impl Proxy {
      /// Binds 127.0.0.1:PROXY_PORT and prepares the client for UPSTREAM.
      pub async fn bind(sink: Arc<dyn CaptureSink>) -> Result<Proxy, ProxyError>;
      pub fn local_addr(&self) -> SocketAddr;
      /// Serves until the task is dropped. Accept errors are logged and the loop continues.
      pub async fn serve(self);
  }
  ```
  Unit tests (inside `proxy.rs`, `#[cfg(test)]`) use a private constructor `Proxy::bind_with(addr: SocketAddr, upstream: String, sink)` that production code never calls with anything but `127.0.0.1:47821` and `UPSTREAM`. `bind` calls `bind_with`.

Dependencies (workspace, then `capture/Cargo.toml`):
```toml
tokio = { version = "1", features = ["rt", "net", "sync", "macros", "time"] }
hyper = { version = "1", features = ["server", "http1"] }
hyper-util = { version = "0.1", features = ["tokio"] }
http-body-util = { version = "0.1", features = ["channel"] }
bytes = "1"
reqwest = { version = "0.13", default-features = false, features = ["rustls-no-provider", "http2"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
```
Before writing code, check docs.rs for the exact current APIs: hyper 1 `server::conn::http1::Builder::serve_connection` with `hyper_util::rt::TokioIo` and `service_fn`; `http_body_util::channel::Channel` (or `StreamBody` if `Channel` is not available in 0.1.5); reqwest 0.13 with `rustls-no-provider` (install `rustls::crypto::ring::default_provider().install_default()` once, ignoring the error when already installed; confirm reqwest then uses it and loads the platform roots). Adjust names to the real APIs and say so in the report. If the `ring` route does not build on this machine, stop and report BLOCKED with the error (do not switch to a provider that needs CMake or NASM).

Behaviour per request:
1. Record `started_at_ms`. Read the whole request body (requests are bounded).
2. Build the upstream request: same method; URL = upstream + path and query; all request headers except `host`, `connection`, `content-length`, `transfer-encoding`, `keep-alive`, `upgrade`, `proxy-connection`, `te`, `trailer`, and `accept-encoding`; then set `accept-encoding: identity`. Body unchanged (none for `GET`/`HEAD`).
3. Send. On error: answer `502` with body `snitchcraft proxy: upstream unreachable`, and record with `error` set and no response.
4. On a response: answer with the same status and headers except the hop-by-hop ones above, `content-length` kept only for `HEAD` or when the upstream sent it. Stream the body: a spawned task reads upstream chunks (`Response::chunk()`), sends each to the client body channel at once, and appends it to a buffer; record `first_byte_at_ms` at the first chunk. If the client goes away, keep reading to the end so the record is complete. At the end, build the record: `response.stream` = buffer as lossy UTF-8 (for `HEAD` or empty body: `None`); `rebuild_message(content type, text)` gives `message`, `message_id`, and an `error` if any; `ended_at_ms`.
5. Request headers are stored through `filter_headers`; the forwarded request keeps the originals.
6. Call `sink.record(record)` from a blocking-safe context (`tokio::task::spawn_blocking`), so a slow disk never stalls the proxy.
7. `id` = `format!("{started_at_ms}-{counter}")` with a process-wide `AtomicU64` counter.

- [ ] **Step 1: Add the dependencies** and run `cargo build -p capture` to confirm the TLS route builds on Windows without CMake or NASM.

- [ ] **Step 2: Write failing tests** in `proxy.rs`. A test helper starts a fake upstream: a hyper server on `127.0.0.1:0` whose handler (a) records the request it received (method, path, headers, body) into a shared `Mutex<Vec<..>>`, and (b) answers `/stream` with `text/event-stream` and a body sent in 3 chunks 150 ms apart (use the invented SSE text from Task 1 split in 3), answers `HEAD /api/hello` with 200 and no body, and answers `/error` with 529 and a JSON error body. A collecting sink stores records in a `Mutex<Vec<CaptureRecord>>` and signals a `tokio::sync::Notify`. The proxy is bound with `bind_with("127.0.0.1:0", fake_upstream_url, sink)`. Requests are made with `reqwest` (plain HTTP to the proxy).

```rust
#[tokio::test]
async fn request_is_forwarded_unchanged_except_hop_headers() { /* POST /stream?beta=true with headers authorization: Bearer test-secret, x-claude-code-session-id: s1, anthropic-beta: b1, accept-encoding: gzip, body {"model":"m","messages":[]}: upstream saw same path and query, same body bytes, authorization present, accept-encoding = identity */ }

#[tokio::test]
async fn streamed_response_arrives_in_pieces_and_unchanged() { /* read the proxied response with chunk(): the first chunk arrives before 300 ms after sending (i.e. before the upstream finished), and the concatenation equals the upstream body exactly */ }

#[tokio::test]
async fn record_has_rebuilt_message_and_timing() { /* after the response, wait on Notify: one record, message_id Some("msg_test1"), response.status 200, first_byte_at_ms and ended_at_ms set and ordered */ }

#[tokio::test]
async fn stored_bytes_never_contain_the_credential() { /* serialise the record to JSON text: does not contain "test-secret" nor "authorization" */ }

#[tokio::test]
async fn head_request_passes_through() { /* HEAD /api/hello gives 200 through the proxy, and a record with stream None */ }

#[tokio::test]
async fn upstream_error_status_is_passed_back_and_recorded() { /* /error: client sees 529 and the same body; record.error = the message */ }

#[tokio::test]
async fn unreachable_upstream_gives_502_and_a_recorded_error() { /* bind_with an upstream of http://127.0.0.1:1 : client sees 502; record.error is Some, response None */ }
```
Write each test fully.

- [ ] **Step 3: Run to see them fail:** `cargo test -p capture proxy`.
- [ ] **Step 4: Implement** `proxy.rs`; export from `lib.rs` (`pub mod proxy; pub use proxy::{CaptureSink, PROXY_PORT, Proxy, ProxyError, UPSTREAM};`).
- [ ] **Step 5: Run tests (several times, to check they are not timing-flaky) and all checks.**
- [ ] **Step 6: Docs.** DECISIONS dependency table: `tokio`, `hyper`, `hyper-util`, `http-body-util`, `bytes`, `reqwest`, `rustls` with reasons (all but rustls's ring provider are already in the build through Tauri; `ring` chosen because `aws-lc` can need CMake and NASM on Windows). Decision "The proxy" (fixed port and upstream, plain HTTP on localhost so no certificate tricks are needed, forwarding first and recording second, uncompressed upstream responses so the stream can be recorded). Decision "First network access (M4)": outbound HTTPS to `api.anthropic.com` only, listening on `127.0.0.1` only, the web page gets none.
- [ ] **Step 7: Commit** (`git commit -m "Add the local capture proxy"` with the trailer).

---

### Task 5: Previous-call pairing and Claude Code capture mapping

**Files:**
- Create: `crates/trace-view/src/order.rs`, `crates/adapter-claude-code/src/capture.rs`
- Modify: `crates/trace-view/src/lib.rs`, `crates/adapter-claude-code/src/lib.rs`, `docs/HARNESS-NOTES.md`

**Interfaces:**
- Produces:
  ```rust
  // trace_view
  /// Model calls of the run that contains `model_call_id`, in order, not
  /// descending into nested runs (subagents); returns the one before it.
  pub fn previous_model_call(trace: &Trace, model_call_id: &str) -> Option<String>;
  /// All model call ids of a trace, each run's calls in order, runs in tree order.
  pub fn model_calls_by_run(trace: &Trace) -> Vec<(String, Vec<String>)>;   // (run id, model call ids)

  // adapter_claude_code::capture (re-exported at crate root)
  pub const SESSION_HEADER: &str = "x-claude-code-session-id";
  /// The capture store key for a Claude Code session id header value, if it is a plain name.
  pub fn session_key(header_value: Option<&str>) -> Option<String>;   // 1..=128 chars of [A-Za-z0-9_-]
  /// The trace id of the model call a response message id belongs to.
  pub fn model_call_id(message_id: &str) -> String;                    // "model:<message id>"
  ```
  `adapter-claude-code` must not depend on `capture-core`; these take plain strings.

Rules for `previous_model_call`: find the nearest ancestor `Run` of the node; collect model calls under that run in depth-first child order, descending through turns and markers but not into tool calls' nested runs (a `Run` child stops the descent); return the element before the given id.

- [ ] **Step 1: Write failing tests.**

In `crates/trace-view/src/order.rs`, build a small trace by hand (`Trace::apply` of events): run R with turn T1 containing model calls M1 (tool call X with a nested run S containing model calls S1, S2) and M2, and turn T2 with M3.
```rust
#[test]
fn previous_model_call_skips_subagent_runs() {
    assert_eq!(previous_model_call(&trace, "M2").as_deref(), Some("M1"));
    assert_eq!(previous_model_call(&trace, "M3").as_deref(), Some("M2"));
    assert_eq!(previous_model_call(&trace, "S2").as_deref(), Some("S1"));
    assert_eq!(previous_model_call(&trace, "M1"), None);
    assert_eq!(previous_model_call(&trace, "S1"), None);
    assert_eq!(previous_model_call(&trace, "nope"), None);
}
#[test]
fn model_calls_are_grouped_by_run() {
    assert_eq!(model_calls_by_run(&trace), [("R".to_string(), vec!["M1".to_string(), "M2".into(), "M3".into()]), ("S".to_string(), vec!["S1".to_string(), "S2".into()])]);
}
```
Also one test against the Claude Code fixture (dev-dependency already present): every model call except the first of each run has a previous call.

In `crates/adapter-claude-code/src/capture.rs`:
```rust
#[test]
fn session_key_accepts_plain_ids_only() {
    assert_eq!(session_key(Some("00000000-0000-4000-8000-000000000002")).as_deref(), Some("00000000-0000-4000-8000-000000000002"));
    assert_eq!(session_key(Some("../x")), None);
    assert_eq!(session_key(Some("")), None);
    assert_eq!(session_key(None), None);
}
#[test]
fn model_call_ids_match_the_parser() { assert_eq!(model_call_id("msg_1"), "model:msg_1"); }
```
And an integration check in `tests/basic_session.rs` style (a new test file is fine): every model call id in the fixture trace starts with `model:`, so `model_call_id` agrees with the parser.

- [ ] **Step 2: Run to see them fail; implement; run tests and all checks.**
- [ ] **Step 3: Docs.** `docs/HARNESS-NOTES.md`: a section "Raw API requests (seen during M4)" with the facts in the spec's Feasibility section (no system prompt text, only structure and sizes).
- [ ] **Step 4: Commit** (`git commit -m "Pair model calls by agent and map captures to Claude Code sessions"` with the trailer).

---

### Task 6: Invented capture fixture

**Files:**
- Create: `crates/capture/examples/make_fixture.rs`, `fixtures/captures/basic/calls.jsonl`
- Modify: `crates/capture/Cargo.toml` (dev-dependencies: `adapter-claude-code`, `trace-core`, `capture-core`), `README.md` only if it lists fixtures

**Interfaces:**
- Consumes: `load_session` (adapter), `CaptureRecord` and friends (Task 1), `model_call_id` / `SESSION_HEADER` (Task 5).
- Produces: `fixtures/captures/basic/calls.jsonl`: one `CaptureRecord` per line, full (system and tools inline, no blob placeholders), for the session `00000000-0000-4000-8000-000000000002` in `fixtures/claude-code/basic`. Regenerate with `cargo run -p capture --example make_fixture`.

What the generator writes (all invented text; nothing read from the real `.claude` folder):
- For every model call in the fixture trace, in time order (main agent and the subagent), one record whose `message_id` is the model call's message id (strip the `model:` prefix), `request.headers` = `x-claude-code-session-id: 00000000-0000-4000-8000-000000000002`, `anthropic-beta: test-beta-1,test-beta-2`, `user-agent: test-harness/0.0.0`, `content-type: application/json`; `request.body` = `{ model (from the trace), max_tokens: 32000, system: [3 invented text blocks, the last two with cache_control], tools: [invented Read, Grep, PowerShell, Skill], messages: [...] }`.
- `messages` grows like a real conversation: for a model call, all prompts of the earlier turns in that run, the earlier model calls' text outputs as assistant messages and their tool results as user `tool_result` messages, using the fixture's own (already sanitised) text; the newest message gets `cache_control`. A subagent's records use a different invented system prompt and start their own message list from the subagent's first prompt.
- From the fourth main-agent call on, the tool list gains an invented `WebFetch` (so the overview shows two tool set versions and a diff shows a tool added).
- `response`: status 200, headers `content-type: text/event-stream; charset=utf-8` and `request-id: req_test_<n>`, a short invented SSE `stream` that rebuilds (via `rebuild_message`) into a message with that `message_id`, and `message` set from `rebuild_message`.
- Timing: `started_at_ms` = the model call's start, `first_byte_at_ms` = start + 800, `ended_at_ms` = the model call's end (or start + 2000).
- One extra record with no matching model call: an invented title-generation call (`model: test-small-model`, a one-message request, a JSON non-stream response with `id: msg_title_test`), timed after the first prompt.
- The generator asserts (with `expect`, it is a dev tool) that every record rebuilds to its own `message_id` and that the output contains none of `Users`, `AppData`, `Bearer`.

- [ ] **Step 1: Write the generator**, run it, and read the output once to check it is invented and well formed.
- [ ] **Step 2: Add a test** `crates/capture/tests/fixture.rs`: the fixture parses; record count = fixture model call count + 1; every record's `message_id` except the title call maps (via `model_call_id`) to a model call in the fixture trace; consecutive main-agent records diff with `shared_prefix > 0`; the store round-trips every fixture record (append all to a temp store, load, compare).
- [ ] **Step 3: Run tests and all checks.**
- [ ] **Step 4: Commit** (`git commit -m "Add an invented capture fixture for the sample session"` with the trailer).

---

### Task 7: Capture views in the app backend

**Files:**
- Create: `src-tauri/src/captures.rs`
- Modify: `src-tauri/Cargo.toml` (deps `capture-core`, `capture`), `src-tauri/src/live.rs`, `src-tauri/src/sessions.rs`, `src-tauri/src/main.rs` (module only), `ui/src/types.ts` (new SessionView field only)
- Create (generated): `ui/src/mock/capture-data.json`

**Interfaces:**
- Consumes: `CaptureStore`, `Loaded` (Task 3); `summarise`, `diff`, `content_hash`, `RequestSummary`, `RequestDiff` (Task 2); `previous_model_call` (Task 5); `session_key`, `model_call_id` (Task 5).
- Produces (in `captures.rs`, all `Serialize`):
  ```rust
  pub struct CaptureIndexEntry { pub capture_id: String, pub trace_id: Option<String>, pub started_at_ms: i64, pub system_hash: Option<String>, pub tools_hash: Option<String> }
  pub struct CaptureIndex { entries: Vec<CaptureIndexEntry> }
  impl CaptureIndex {
      pub fn build(trace: &Trace, records: &[CaptureRecord]) -> Self;   // trace_id = model_call_id(message_id) if that node exists in the trace
      pub fn captured_trace_ids(&self) -> Vec<String>;
      pub fn capture_for(&self, trace_id: &str) -> Option<&CaptureIndexEntry>;
      pub fn entries(&self) -> &[CaptureIndexEntry];
  }
  pub struct CallSummary { pub capture_id: String, pub trace_id: Option<String>, pub started_at_ms: i64, pub duration_ms: Option<i64>, pub model: Option<String>, pub status: Option<u16>, pub error: Option<String>, pub system_version: Option<usize>, pub tools_version: Option<usize>, pub message_count: usize }
  pub struct VersionSummary { pub version: usize, pub hash: String, pub first_capture_id: String, pub call_count: usize, pub system_chars: Option<usize>, pub tool_names: Option<Vec<String>> }
  pub struct CaptureOverview { pub session_key: Option<String>, pub total_bytes: u64, pub calls: Vec<CallSummary>, pub system_versions: Vec<VersionSummary>, pub tool_versions: Vec<VersionSummary>, pub other_call_ids: Vec<String>, pub skipped: Vec<String> }
  pub struct CaptureDetail { pub record: CaptureRecord, pub summary: Option<RequestSummary>, pub previous_capture_id: Option<String>, pub diff: Option<RequestDiff> }
  pub fn overview(trace: &Trace, loaded: &Loaded, key: Option<&str>, total_bytes: u64) -> CaptureOverview;
  pub fn detail(trace: &Trace, records: &[CaptureRecord], capture_id: &str) -> Option<CaptureDetail>;
  ```
- `SessionView` gains `pub captured_trace_ids: Vec<String>` (doc: model call trace ids that have a captured API call). `LiveSession` gains `pub fn set_captures(&mut self, records: &[CaptureRecord])` (rebuilds the index), `pub fn capture_key(&self) -> Option<String>` (= `session_key(Some(session_id))`), and `view()` fills `captured_trace_ids`.

Rules:
- Versions are numbered from 1 in order of first appearance, separately for system prompts and tool sets.
- `other_call_ids`: captures whose `trace_id` is `None`, in time order.
- Previous capture for `detail`: if the capture has a `trace_id`, take `previous_model_call(trace, trace_id)` and the capture mapped to it; otherwise the latest earlier capture (by `started_at_ms`) with the same `system_hash`. `diff` = `capture_core::diff(previous, this)` when both exist.
- `calls` are in `started_at_ms` order; `duration_ms` = ended - started.

- [ ] **Step 1: Write failing tests** in `captures.rs`, using the Claude Code fixture trace and `fixtures/captures/basic/calls.jsonl` parsed into records:
  - `index_maps_every_fixture_call_but_the_title_call` (captured ids = all model call ids; one entry without trace id).
  - `overview_lists_versions_and_other_calls` (1 system version for the main agent plus 1 for the subagent = 2; 2 tool versions; `other_call_ids` = the title call).
  - `detail_pairs_with_the_previous_call_of_the_same_agent` (for the main agent's second call, previous = the first main call's capture, `diff.shared_prefix > 0`; for the subagent's first call, previous = None; for the call where WebFetch appears, `diff.tools_added == ["WebFetch"]`).
  - `detail_of_an_unknown_id_is_none`.
- [ ] **Step 2: Implement** `captures.rs` and the `LiveSession` / `SessionView` changes (no store access in `LiveSession` yet; Task 8 wires it).
- [ ] **Step 3: Snapshot for the UI mock.** In `sessions.rs` tests, add `ui_capture_data_matches_the_backend` using the same compare-or-write helper (`check_snapshot`) and `SNITCHCRAFT_UPDATE_SNAPSHOTS`: write `ui/src/mock/capture-data.json` = `{ "overview": overview(...) with total_bytes pinned to 123456, "details": { capture_id: detail(...) for every capture } }`. Add `captured_trace_ids` to the existing fixture-data and live-steps snapshots (regenerate both).
- [ ] **Step 4: Add `captured_trace_ids: string[]` to `SessionView` in `ui/src/types.ts`** so the UI typechecks. Regenerate snapshots (PowerShell command in CLAUDE.md), run all checks including UI.
- [ ] **Step 5: Commit** (`git commit -m "Build capture overviews and details in the backend"` with the trailer).

---

### Task 8: Proxy, store and commands wired into the app

**Files:**
- Modify: `src-tauri/src/main.rs`, `src-tauri/src/watch.rs`, `src-tauri/src/live.rs`, `src-tauri/build.rs`, `src-tauri/capabilities/default.json`, `src-tauri/Cargo.toml`, `docs/DECISIONS.md`, `docs/superpowers/specs/2026-10-05-m4-proxy-capture-design.md` (captures folder line only)

**Interfaces:**
- Consumes: everything from Tasks 3, 4, 5, 7.
- Produces:
  - `src-tauri/src/capture_sink.rs` (new): `pub struct AppSink { store: Arc<CaptureStore>, shared: Arc<Shared>, cache: Arc<RecordCache> }` implementing `CaptureSink`: `record()` computes the key with `session_key(record.header(SESSION_HEADER))` (fallback `UNKNOWN_SESSION`), appends to the store (log errors with `tracing::warn!` and set a `last_error` in `ProxyState`), invalidates the cache for that key, then if the active session's `capture_key()` equals the key, reloads that session's records, calls `set_captures`, bumps the version and sends `LiveMessage::Updated` with `changed_trace_ids` = the new capture's trace id (if any).
  - `pub struct RecordCache` (last key + `Arc<Vec<CaptureRecord>>`), with `get_or_load(&self, store, key) -> Result<Arc<Vec<CaptureRecord>>, StoreError>` and `invalidate(&self, key)`.
  - `pub struct ProxyState { pub listening: AtomicBool, pub error: Mutex<Option<String>>, pub last_save_error: Mutex<Option<String>> }`.
  - `LiveSession::refresh_captures(&mut self, records: &[CaptureRecord]) -> LiveMessage` (bumps version, returns Updated with the given changed ids).
  - Commands (all `rename_all = "snake_case"`, registered and listed in `build.rs`, granted in the capability):
    - `capture_status() -> CaptureStatus { listening: bool, port: u16, command: String, error: Option<String>, last_save_error: Option<String> }` with `command` = `$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude`.
    - `session_captures(project, session_id) -> CaptureOverview` (validates names with `session_path`; key = `session_key(Some(session_id))`; uses the active session's trace if it is this session, else opens a temporary `LiveSession`).
    - `capture_detail(project, session_id, capture_id) -> Option<CaptureDetail>` (same validation; `capture_id` length-checked like trace ids).
    - `delete_captures(project, session_id) -> ()` (same validation; `store.delete(key)`; invalidates the cache; if the session is active, `set_captures(&[])` and sends an Updated).
- Startup in `main.rs` `setup`: resolve `app.path().app_data_dir()?.join("captures")` (log and disable capture if it fails); create `CaptureStore`; manage `Arc<CaptureStore>`, `Arc<RecordCache>`, `Arc<ProxyState>`; spawn on `tauri::async_runtime` a task that calls `Proxy::bind(sink)`: on success set `listening` and `serve().await`; on error store the message in `ProxyState.error` and log it.
- `load_session` also loads the session's captures (through the cache) and calls `set_captures` before taking the first view.

Rules:
- Lock order: never hold `shared.active()` while doing store I/O except the reload in `AppSink::record`, which must load records first (outside the lock) and only then lock `active` to apply them.
- No command returns header values beyond what `filter_headers` kept (records are stored filtered already; do not add any).

- [ ] **Step 1: Write failing tests** (in `capture_sink.rs` / `main.rs` tests, using temp folders and the fixture):
  - `sink_saves_and_updates_the_open_session`: an `Active` LiveSession for the fixture (in a temp projects root, as in `watch.rs` tests) with a test `Sink<LiveMessage>`; call `AppSink::record` with a fixture capture of that session: the store has 1 record and the sink received an `Updated` whose `changed_trace_ids` contains the capture's model call id and whose view lists it in `captured_trace_ids`.
  - `sink_for_another_session_only_saves`: no message sent.
  - `unknown_session_goes_to_the_unknown_folder`.
  - `delete_clears_the_open_sessions_markers`.
  - `capture_status_has_the_copyable_command`.
- [ ] **Step 2: Implement** the sink, cache, state, commands, startup, permissions (`build.rs` command list and `capabilities/default.json`: add `allow-capture-status`, `allow-session-captures`, `allow-capture-detail`, `allow-delete-captures`; update the description sentence to mention reading and deleting the app's own captures; nothing else).
- [ ] **Step 3: Run tests and all checks.** Run `cargo build -p snitchcraft`. Do not run `cargo tauri dev` (the coordinator does the real run).
- [ ] **Step 4: Docs.** DECISIONS: "Captures folder" (planning decision 1), "Captures are read on demand" (planning decision 7), "Captures kept until deleted (chosen by the project owner)", "Opt-in capture per session (chosen by the project owner)". Fix the captures folder line in the spec.
- [ ] **Step 5: Commit** (`git commit -m "Run the capture proxy in the app and serve capture views"` with the trailer).

---

### Task 9: UI types, API, helpers and mock

**Files:**
- Create: `ui/src/captures.ts`, `ui/src/captures.test.ts`
- Modify: `ui/src/types.ts`, `ui/src/api.ts`, `ui/src/mock/mockApi.ts`

**Interfaces:**
- Consumes: Rust shapes from Tasks 1, 2, 7, 8 exactly as serialised (snake_case; `Body` is `{ kind: 'empty' } | { kind: 'json', value } | { kind: 'text', value }`; `settings` is an array of `[key, value]` pairs).
- Produces:
  ```ts
  // types.ts: CaptureRecord, CapturedRequest, CapturedResponse, Header, Body, RequestSummary, MessageSummary, SettingChange, RequestDiff, CallSummary, VersionSummary, CaptureOverview, CaptureDetail, CaptureStatus
  // api.ts (Api gains)
  captureStatus(): Promise<CaptureStatus>
  sessionCaptures(project: string, sessionId: string): Promise<CaptureOverview>
  captureDetail(project: string, sessionId: string, captureId: string): Promise<CaptureDetail | null>
  deleteCaptures(project: string, sessionId: string): Promise<void>
  // captures.ts
  export function requestBody(record: CaptureRecord): Record<string, unknown> | null
  export function systemBlocks(body: Record<string, unknown> | null): { text: string; cached: boolean }[]
  export function toolList(body: Record<string, unknown> | null): { name: string; description: string; schema: unknown }[]
  export function messageList(body: Record<string, unknown> | null): { role: string; blocks: { type: string; text: string; cached: boolean; raw: unknown }[] }[]
  export function formatBytes2(n: number): string   // only if format.ts has no suitable helper; reuse formatBytes otherwise
  export function captureIdFor(overview: CaptureOverview | null, traceId: string): string | null
  ```
  These helpers only reshape the backend's JSON for display (they do not interpret transcripts). `messageList` block `text`: the block's `text`, `thinking`, a tool_use's `name` plus compact input, or a tool_result's text; `cached` = has `cache_control`.

- [ ] **Step 1: Write failing tests** in `captures.test.ts` against `ui/src/mock/capture-data.json`: `requestBody` of a fixture detail is an object with `messages`; `systemBlocks` gives 3 blocks with the last two `cached`; `toolList` names match the summary's `tool_names`; `messageList` roles alternate starting with `user`; `captureIdFor` finds the capture of a model call and returns null for an unknown id.
- [ ] **Step 2: Implement** types, api (Tauri `invoke` with snake_case args), helpers, and the mock: `captureStatus` returns listening on 47821 with the command; `sessionCaptures` returns the snapshot overview for the fixture session (empty overview otherwise); `captureDetail` returns the snapshot detail; `deleteCaptures` resolves (and makes later `sessionCaptures` return an empty overview within the page session); `?mock&fail=captures` makes `sessionCaptures` throw. Document the new flag in the mock header comment.
- [ ] **Step 3: Run the UI checks.**
- [ ] **Step 4: Commit** (`git commit -m "Add capture types, API and display helpers to the UI"` with the trailer).

---

### Task 10: The three views in the UI

**Files:**
- Modify: `ui/src/components/DetailsPanel.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/components/nodes.tsx`, `ui/src/App.tsx`, `ui/src/app.css`
- Create (if it keeps files small): `ui/src/components/CapturePanel.tsx`, `ui/src/components/CaptureOverview.tsx`

**Interfaces:**
- Consumes: Task 9's API and helpers; `SessionView.captured_trace_ids`.

Required behaviour:
1. **Marker:** model call boxes whose trace id is in `captured_trace_ids` show a small "API" tag (text plus `title="Raw API request captured"`), consistent with the existing tag style.
2. **Full request** (DetailsPanel, for a model call with a capture): a "Raw request" section, collapsed by default, loaded with `captureDetail` only when opened. Inside: settings (model, max_tokens, thinking, other settings as `key: compact JSON`, betas as a list); "System prompt (N blocks, X chars)" with each block collapsible and a "cached" chip on cached blocks; "Tools (N)" with each tool collapsible (description and schema as formatted JSON); "Messages (N)" with each message collapsible, blocks shown with type, text, and a "cached" chip; "Response" with the rebuilt message as formatted JSON and the raw stream in a collapsible `<pre>`; "Copy request JSON" button using `navigator.clipboard.writeText`, with a short "Copied" confirmation and a visible message if copying fails. Long text is shown in scrollable `<pre>` blocks with a max height.
3. **Changes since the previous call** (same panel, above the full request, open by default when a diff exists): "Kept N messages", lists of removed and added messages (role, block types, chars, preview), "System prompt changed" / "Tools changed: +A -B", setting changes `key: before -> after`, beta changes. With no previous call: "First call of this agent". A diff with removed messages gets a highlighted note "History was removed or replaced (for example by compaction)".
4. **Session overview** (session panel, a collapsible "API calls" section): loaded with `sessionCaptures` when a session opens and again whenever an update arrives whose view's `captured_trace_ids` length changed. Shows: number of captured calls and total size (`formatBytes`); system prompt versions and tool set versions (version number, call count, size or tool count; tool names in a `title`); "Other API calls (N)" listing each with model, time and status, clickable to show its full request in the details panel (the details panel then shows a capture by id instead of a node: add a small state for "selected capture" alongside the selected node); a Delete button that asks for confirmation inside the panel ("Delete N captured calls? This cannot be undone." with Delete and Cancel buttons), then calls `deleteCaptures` and reloads the overview.
5. **Start command and status:** in the session list sidebar (top), a compact "Capture" box: when `captureStatus().listening`, the command in a monospace field with a Copy button and one line "Start Claude Code with this command to capture its API calls."; when not listening, the error ("Capture is off: <error>"). Show `last_save_error` as a warning when set. Refresh the status when the list refreshes.
6. A session with no captures shows, in the overview section, "No API calls captured for this session." and the start command hint.
7. Errors from capture commands show inline in their section and never break the rest of the page.

- [ ] **Step 1: Implement** the behaviour above, keeping components small (split into the two new files if `DetailsPanel.tsx` or `Sidebar.tsx` would grow past about 400 lines).
- [ ] **Step 2: Run the UI checks.**
- [ ] **Step 3: Browser check with the mock** (`cd ui; npm run dev`, open `http://localhost:5173/?mock`): the API tag on model call boxes; open a model call's Raw request and see system blocks with cached chips, tools, messages, response; the diff on the second call and "Tools changed: +WebFetch" on the call where it appears; the session overview with 2 system versions, 2 tool sets and 1 other call; open the other call; copy the start command; Delete with confirmation clears the overview. Use browser automation tools if available (load them with ToolSearch); if not possible, say so plainly. Report exactly what was seen.
- [ ] **Step 4: Commit** (`git commit -m "Show captured API requests, changes between calls and a session overview"` with the trailer).

---

### Task 11: Real capture check and docs (done by the coordinator, not a subagent)

- [ ] **Step 1:** Run all checks on the branch.
- [ ] **Step 2:** Run `cargo tauri dev`. Confirm the capture box shows the command and "listening".
- [ ] **Step 3:** In a temp folder outside `.claude`, run a short Claude Code session with the command (Haiku, a few tool calls and one subagent). Confirm: Claude Code works normally; the session appears; model call boxes get the API tag live; the Raw request shows system prompt, tools and messages; the diff between consecutive calls is short; the overview lists versions and any other API calls (for example title generation). Check that no stored file contains the login token: search the captures folder for `authorization` and for the first characters of the token value read at run time from the request (do not print it).
- [ ] **Step 4:** Delete the test session's captures from the app and confirm the folder is gone.
- [ ] **Step 5:** Docs: `docs/HARNESS-NOTES.md` (what the raw requests showed: structure only, no system prompt text), `README.md` (how to capture a session, where captures live, privacy notes, milestone status M4 done), `CLAUDE.md` (current milestone M4 complete with its steps, M5 next; the network rule now names M4's scoped access; commands list gains the capture command and the fixture generator).
- [ ] **Step 6:** Commit.
