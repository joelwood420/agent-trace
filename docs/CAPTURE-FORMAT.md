# Capture format

This is how Snitchcraft stores one HTTP exchange between an agent harness and a model API. It is defined in `crates/capture-core`. It is separate from the trace schema (`docs/SCHEMA.md`): a trace describes what the agent did, a capture is the raw request and response.

## Record

One `CaptureRecord` per exchange. In a file, one record per JSON object.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Stable, unique within a capture store. |
| `started_at_ms` | integer | yes | When the request was received, milliseconds since 1970-01-01 UTC. |
| `first_byte_at_ms` | integer or null | yes | When the first response byte arrived. |
| `ended_at_ms` | integer or null | yes | When the response finished. |
| `request` | object | yes | See Request. |
| `response` | object or null | yes | See Response. Null if the upstream never answered. |
| `message_id` | string or null | yes | The API message id (`msg_...`) of the reply, when known. |
| `error` | string or null | yes | A failure description, for example an unreachable upstream or an error event in the stream. |

## Request

| Field | Type | Meaning |
|---|---|---|
| `method` | string | HTTP method. |
| `path` | string | Path and query, for example `/v1/messages`. |
| `headers` | array of Header | Filtered, see Header rules. |
| `body` | Body | The request body, for requests to `/v1/messages` (with or without a query). For any other path it is always `empty`: only model call bodies are kept, so nothing else Claude Code might send through the base URL is stored. |

## Response

| Field | Type | Meaning |
|---|---|---|
| `status` | integer | HTTP status code. |
| `headers` | array of Header | Filtered, see Header rules. |
| `stream` | string or null | The raw event stream text, when the response was streamed. |
| `message` | any JSON or null | The complete message: the JSON body, or the message rebuilt from the stream. |

## Header

`{ "name": string, "value": string }`. The name is lowercased. Input order is kept.

## Body

Stored as `{ "kind": ..., "value": ... }`.

| `kind` | `value` | When |
|---|---|---|
| `empty` | absent | No bytes. |
| `json` | the parsed JSON, key order kept | The bytes are valid JSON. |
| `text` | string | Anything else, as lossy UTF-8. |

## Header rules

- Credential headers are dropped entirely: `authorization`, `proxy-authorization`, `x-api-key`, `cookie`, `set-cookie`, and any name containing `token`, `secret`, `auth`, `key`, `password` or `credential`.
- Names starting `anthropic-ratelimit-` are exempt from the name rules (they carry only numbers and timestamps, for example `anthropic-ratelimit-tokens-remaining`), so they keep their values.
- Otherwise this check runs before the allowlist, so `anthropic-api-key` is dropped.
- Values are kept for names starting `anthropic-` or `x-stainless-`, and for `user-agent`, `content-type`, `x-app`, `x-claude-code-session-id`, `request-id` and `retry-after`.
- Every other header is kept with the value `<omitted>`.

## Stream rebuild

`rebuild_message(content_type, body)` returns `message`, `message_id`, `error` and `unknown_events`.

For `text/event-stream` bodies, events are separated by blank lines (LF or CRLF). Each has an `event:` line and one or more `data:` lines, joined with a newline.

| Event | Effect |
|---|---|
| `message_start` | The message is `data.message`. |
| `content_block_start` | `content[index]` is set to `data.content_block`. |
| `content_block_delta` | `text_delta` appends to `text`. `thinking_delta` appends to `thinking`. `signature_delta` sets `signature`. `input_json_delta` is buffered per block. `citations_delta` pushes onto `citations`. Any other delta type is listed in `unknown_events` as `content_block_delta:<type>`. |
| `content_block_stop` | A buffered `input_json_delta` text is parsed into `input`. If it does not parse, `input` is the raw string and `content_block_stop:bad_json` is listed in `unknown_events`. |
| `message_delta` | Every key of `data.delta` is copied onto the message, and every key of `data.usage` is merged into `usage`. |
| `message_stop`, `ping` | Nothing. |
| `error` | `error` is `data.error.message`, or the raw data. |
| anything else | The event name is listed in `unknown_events`. An event with no `event:` line is listed as `<unnamed>`. |

Nothing unexpected is dropped silently. These markers go in `unknown_events`:

| Marker | Meaning |
|---|---|
| `<event>:bad_data` | The event data is not a JSON object, or lacks a needed field. A bad `message_start` keeps any earlier message. |
| `<event>:before_message_start` | A block or `message_delta` event arrived before any `message_start`. |
| `content_block_start:bad_index` | The index is beyond the next free slot. Only the next block or an existing one is accepted, which bounds memory. |
| `content_block_delta:orphan`, `content_block_stop:orphan` | The block was never started. |
| `truncated_input_json` | The stream ended with tool input that never got its stop event. The partial text is kept as the block's `input` string. |

The `text/event-stream` content type is matched ignoring case.

`message_id` is `message.id`. For a plain JSON body, `message` is the JSON, `message_id` is its `id` if that starts with `msg_`, and a body with `"type": "error"` sets `error` to `error.message`. Any other body gives nothing.

## Comparing requests

`summarise` and `diff` work on request bodies that are JSON objects and return nothing otherwise.

- `content_hash` is the sha256 (lowercase hex) of the compact JSON text of a value, with keys in stored order. It identifies a system prompt or a tool set.
- `summarise` gives the model, the system and tools hashes, tool names in order, `system_chars` (total length of system `text` fields, or the string length), the message count, every other top-level body key as a setting in body order, and the `anthropic-beta` header split on commas and trimmed.
- `diff(previous, next)` compares messages after removing every `cache_control` key at any depth, because Claude Code moves the cache marker to the newest message on each call. `shared_prefix` is the number of leading equal messages. `removed` is the previous messages from there on, and `added` is the next messages from there on, each with its index in its own request.
- A message summary has `role`, the content block types (a string content is `text`), `chars` (length of the content as JSON text) and a `preview` (first text found, including inside a `tool_result`, newlines turned to spaces, cut to 160 characters).
- `system_changed` and `tools_changed` compare content hashes (an absent field is its own value). `tools_added` and `tools_removed` compare tool names.
- `settings_changed` lists keys whose value differs, previous order first and then new keys, with `null` for absent. `betas_added` and `betas_removed` compare the beta header values.

## Example

```json
{
  "id": "c1",
  "started_at_ms": 1700000000000,
  "first_byte_at_ms": 1700000000400,
  "ended_at_ms": 1700000001200,
  "request": {
    "method": "POST",
    "path": "/v1/messages",
    "headers": [
      { "name": "anthropic-version", "value": "2023-06-01" },
      { "name": "x-forwarded-for", "value": "<omitted>" }
    ],
    "body": { "kind": "json", "value": { "model": "test-model", "messages": [] } }
  },
  "response": {
    "status": 200,
    "headers": [{ "name": "content-type", "value": "text/event-stream" }],
    "stream": null,
    "message": { "id": "msg_test1", "type": "message", "role": "assistant", "content": [] }
  },
  "message_id": "msg_test1",
  "error": null
}
```
