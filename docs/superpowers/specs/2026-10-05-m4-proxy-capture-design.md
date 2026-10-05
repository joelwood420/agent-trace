# M4: proxy capture of raw API requests

Status: design approved in chat on 2026-10-05, awaiting review of this written spec.

## Goal

Show what Claude Code actually sends to the model API on every call, next to the model call it belongs to: the system prompt, the tool definitions, the full message list, the request settings, and the raw response. Make the differences between consecutive calls readable, so compaction, injected reminders and changes to the system prompt or tools stand out.

The transcripts show what the agent did. The raw requests show how the harness is built. This is the reference for the project owner's own harness (M5), which will later write the same capture format.

## Feasibility (checked on 2026-10-05)

A throwaway pass-through proxy confirmed, on this machine with a Claude subscription login (no API key):

- Claude Code honours `ANTHROPIC_BASE_URL` with a subscription login and works normally through a local plain-HTTP proxy that forwards to `https://api.anthropic.com`. The login token passes through untouched.
- Streaming responses (server-sent events) pass through fine.
- Before the first request, Claude Code sends `HEAD /api/hello` to the same address. The proxy must forward it.
- Requests go to `POST /v1/messages?beta=true`. One request for a one-word reply was about 300 KB: a 3-block system prompt (about 28,000 characters, two blocks marked for caching), 132 tool definitions, one user message of 11 text blocks, `max_tokens` 32000, thinking enabled with its text hidden (`display: "omitted"`), and a `context_management` option.
- Every request carries an `x-claude-code-session-id` header equal to the transcript file name, and the response's message id (`msg_...`) appears in the transcript. The adapter's model call ids are `model:<message id>`, so a capture joins to its model call exactly.

## Decisions made while designing (chosen by the project owner)

These will be recorded in `docs/DECISIONS.md`.

1. **Opt-in per session.** A session is captured only when the user starts Claude Code with `ANTHROPIC_BASE_URL` pointing at the app's proxy. Snitchcraft never changes Claude Code's settings and never writes under `.claude`. If the app is not running, a session started this way cannot reach the API; normal sessions are never affected.
2. **All three views, in this order:** the full request per model call, the changes since the previous call, and a session overview.
3. **Captures live outside the trace schema.** They get their own harness-agnostic format in a new crate. `trace-core` does not change. Captures are joined to model calls by message id.
4. **Captures are kept on disk until the user deletes them**, per session, in the app's own data folder.

## Scope

In scope:

- A local pass-through proxy started with the app.
- Saving each API call (request, response, timing) with credentials removed.
- Storage with repeated system prompts and tool sets kept once.
- Joining captures to model calls, plus a list of API calls that have no model call in the transcript.
- The three views, live updates for the open session, and deleting a session's captures.
- Sanitised capture fixtures and a mock mode that shows the three views.

Out of scope (noted for later):

- Capturing the agent's scratchpad files. Hidden thinking cannot be captured from outside: Claude Code asks the API to omit thinking text, so it is never sent back.
- Token cost breakdowns.
- Capturing harnesses other than Claude Code, or APIs other than Anthropic's.
- Changing Claude Code's settings for the user, or any always-on capture.

## Architecture

New crates:

- `crates/capture-core`: the capture format, rebuilding a final message from a response stream, and the diff between two requests. Pure logic, no I/O, nothing Claude Code specific. Doc comments on all public types, since the format is a contract the M5 harness will write.
- `crates/capture`: the proxy server and the on-disk store. Depends on `capture-core`.

Changes to existing crates:

- `crates/adapter-claude-code`: the Claude Code specific bits of capture handling: which header names the session (`x-claude-code-session-id`) and how a capture maps to a model call id (`model:<message id>`).
- `src-tauri`: starts the proxy, joins captures to the open session, new commands, live updates.
- `ui`: the three views, capture markers on boxes, the start command, Delete.

Data flow: Claude Code -> proxy (forward and stream back) -> capture record -> store -> join with the session's trace -> commands and live updates -> UI.

## The proxy

- Listens on `127.0.0.1:47821` only. If the port is taken, capture is off, the app shows a message, and everything else works.
- Forwards to the fixed upstream `https://api.anthropic.com`. It is never an open relay to other hosts. Tests may point it at a local fake upstream through a test-only constructor; the production path has no way to change the upstream.
- Forwards every method and path, including `HEAD /api/hello`, with the request headers and body unchanged except hop-by-hop headers (`host`, `connection`, `content-length`, `transfer-encoding`). It asks the upstream for an uncompressed response so the stream can be both forwarded and recorded.
- Streams the response back to Claude Code as it arrives, chunk by chunk, and records a copy at the same time. The record is written after the response ends.
- If the upstream cannot be reached, Claude Code gets a `502` with a short message, and the failed call is recorded with the error.
- A failure to save a record never affects forwarding. It is logged and shown as a warning in the app.
- Forwarding must not add noticeable delay: no buffering of the whole response before forwarding.

## The capture format (`capture-core`)

One record per HTTP exchange:

- `id`: unique within the store (for example a timestamp plus a counter).
- `started_at_ms`, `first_byte_at_ms`, `ended_at_ms`.
- `request`: `method`, `path`, `headers` (filtered, see below), `body` (the JSON value, or the text if it is not JSON).
- `response`: `status`, `headers` (filtered), `stream` (the raw response text, for example the server-sent events), and `message` (the final message rebuilt from the stream, or the JSON body for a non-streaming response).
- `message_id`: the response message id, when there is one.
- `error`: set when the exchange failed.

Header filtering uses an allowlist. Kept with their values: `anthropic-*`, `x-stainless-*`, `user-agent`, `content-type`, `x-app`, `x-claude-code-session-id`, `request-id`, and rate-limit headers. Every other header is kept by name only, with the value replaced by `<omitted>`. Headers that carry credentials (`authorization`, `x-api-key`, `cookie`, `set-cookie`, and any name containing `token`, `secret` or `auth`) are dropped by name and value: never written, logged or shown.

## Storage (`capture`)

- Location: the app data folder resolved at runtime (Tauri's app data folder for the app identifier `dev.snitchcraft.app`, so `%APPDATA%\dev.snitchcraft.app\captures\` on Windows), built with `PathBuf`. Never under `.claude`, never in the repo.
- One folder per session, named after the session id header. Calls without that header go in an `unknown` folder.
- Records are appended to a compressed JSON Lines file in that folder.
- The system prompt and the tool definitions of each request are stored once per distinct version, under their content hash, and the record refers to the hash. The message list is stored in full in each record. Compression keeps the repetition small.
- A record that cannot be read back (damaged or cut short) is skipped and reported, like a skipped transcript line.
- Deleting a session's captures removes its folder. This is the only delete the app performs, and only inside its own captures folder.

## Joining captures to the trace

- When a session is opened, the backend loads that session's captures and matches each one to the model call whose id is `model:<message id>`.
- Captures with no matching model call are listed as "Other API calls" for the session. Expected examples are title generation, compaction summaries and quota checks, which the transcript does not record.

## Views

All analysis is done in Rust; the UI only displays it.

1. **Full request** (details panel of a model call, "Raw request" section): settings (model, `max_tokens`, thinking, other request options, the beta features from the `anthropic-beta` header), the system prompt blocks, the tool definitions, the messages, each collapsible, with markers on content flagged for caching. Then the rebuilt response and the raw stream. A "Copy JSON" button copies the request body.
2. **Changes since the previous call:** the previous call is the previous model call of the same agent in trace order (main agent and subagents interleave, so time order would compare the wrong calls). A capture with no model call falls back to the previous capture with the same system prompt hash. The diff shows: messages kept (as a count of the shared prefix), messages removed or replaced, messages added, whether the system prompt or tool set changed (with tool names added and removed), and request settings that changed.
3. **Session overview** (session panel): each distinct system prompt version and tool set with the calls that used them, the "Other API calls" list, the total captured size with a Delete button, and the command to start a captured session, ready to copy: `$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude`.

Boxes with a capture get a small marker in the diagram.

## Live updates

When the proxy saves a record whose session id matches the open session, it notifies the live session directly (no file watching needed for the app's own files). The open session sends an update with the matched model call id among the changed trace ids, so the marker and the details panel update within about a second.

## Permissions and access

- The Rust backend opens outbound HTTPS connections to `api.anthropic.com` only, and listens on `127.0.0.1:47821` only.
- The web page gets no network access. Its only new abilities are the new commands: list a session's captures and overview, get one capture's detail and diff, delete a session's captures, and get the proxy status.
- `.claude` stays read-only. The only folder the app writes to is its own captures folder.
- The content security policy is unchanged.

## Error handling

- Port taken: capture off, message in the app.
- Upstream unreachable or erroring: Claude Code sees the error, the record keeps it.
- Save failure: logged and shown as a warning; forwarding unaffected.
- Damaged records: skipped and reported.
- Unknown request or response shapes (not JSON, unexpected stream events): recorded as text, shown raw, never a panic.

## Testing

- Proxy tests against a fake local upstream: requests and bodies arrive unchanged, streamed responses arrive chunk by chunk and unchanged, `HEAD` works, upstream failure gives `502` and a recorded error.
- Redaction tests: credential headers never appear in any stored byte, log line or command response.
- Stream rebuild tests on sanitised, invented event streams, including thinking, tool use and unknown event types.
- Store tests: round trip, deduplication of system prompts and tool sets, damaged lines skipped, delete removes only that session's folder.
- Diff tests: plain growth, compaction (history replaced by a summary), a tool added, settings changed, and interleaved subagent calls paired by agent.
- A sanitised capture fixture keyed to the message ids of the existing `fixtures/claude-code/basic` session, used by join tests and by mock mode, so all three views can be checked in a browser. No real requests are ever committed.
- Manual: run a real Claude Code session through the proxy, open it in the app, and report what was actually seen.
- All existing checks keep passing: `cargo test --workspace`, clippy, fmt, and the UI lint, typecheck, test and build.

## Docs to update

- `docs/DECISIONS.md`: the four decisions above, the port, the storage layout, the header allowlist, every new dependency with its reason, and the first network access.
- `docs/SCHEMA.md` is unchanged. A new `docs/CAPTURE-FORMAT.md` describes the capture format readably, since the M5 harness will write it.
- `docs/HARNESS-NOTES.md`: what the raw requests reveal (system prompt structure, tool set size, cache markers, hidden calls, compaction requests).
- `README.md`: how to capture a session, privacy notes, milestone status.
- `CLAUDE.md`: current milestone and status; the rule "no network access unless a milestone needs it" now names M4's scoped access.
