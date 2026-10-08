# Trace schema

This is the format any agent harness can emit so that Snitchcraft can draw it. It is defined in `crates/trace-core`. A complete example lives in `fixtures/trace-core/example-trace.jsonl`.

## The idea in one paragraph

A trace is a stream of **events**. Each event describes one **node** in a tree. Every node has an `id` and a `parent_id`, so the tree can be rebuilt without guessing. If an event arrives with an `id` that was already sent, it **replaces** the earlier version of that node. This is how a node gets updated, for example when a tool call is re-sent with its result filled in.

## The tree

```
Run                      a whole session
  Turn                   one user prompt and everything it caused
    ModelCall            one request to the model and its response
      ToolCall           one tool use the response asked for
        Run              (only if the tool started a subagent)
          Turn ...
    Marker               something notable that is not a call
    ContextUpdate        a change to the hidden input sent with the conversation
```

- A `Run`'s children are its turns, plus markers and context updates for anything that happened outside a turn (for example a hook that ran when the session started, or the system prompt sent before the first prompt).
- A `Turn`'s children are its model calls, markers and context updates, in the order they happened.
- A `ModelCall`'s children are the tool calls it requested. All tool calls under the same model call were requested together, so they may run in parallel.
- A subagent is a `Run` whose parent is the `ToolCall` that started it.

## Event fields

Every event is one JSON object. In a file, one event per line (JSONL).

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Stable, unique within the trace. |
| `parent_id` | string or null | yes | Parent node's `id`. `null` only for a top-level `Run`. |
| `node` | object | yes | The node kind and its data. See below. |
| `started_at_ms` | integer | no | Start time, milliseconds since 1970-01-01 UTC. |
| `ended_at_ms` | integer | no | End time, same units. Leave out while running or if unknown. |
| `raw` | array | no | The original records this node was built from. See below. |
| `metadata` | object | no | Harness-specific extras that have no place in the core fields. |

## Node kinds

`node.type` says which kind it is.

### `run`

| Field | Type | Required | Meaning |
|---|---|---|---|
| `harness` | string | yes | Name of the harness, for example `"claude-code"`. |
| `title` | string | no | Human-readable title. |

### `turn`

| Field | Type | Required | Meaning |
|---|---|---|---|
| `prompt` | content list | yes | The prompt that started the turn. |

### `model_call`

| Field | Type | Required | Meaning |
|---|---|---|---|
| `model` | string | no | Model id as reported by the provider. |
| `output` | content list | no | Text and thinking the model produced, in order. Tool uses are not listed here; they are child `tool_call` nodes. |
| `stop_reason` | stop reason | no | Why the model stopped. |
| `usage` | usage | no | Token counts. |

### `tool_call`

| Field | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Tool name, for example `"Read"`. |
| `input` | any JSON | yes | What the model passed to the tool. |
| `result` | object | no | Leave out until the tool finishes. Then `{ "content": <content list>, "is_error": bool }`. |

### `marker`

For things that are not model or tool calls: context compaction, hooks, interrupts, API errors, retries.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `kind` | string | yes | Short label such as `"compaction"`, `"hook"`, `"interrupt"`, `"api_error"`. You can define your own. |
| `summary` | string | yes | One line for humans. |

### `context_update`

For the hidden input a harness sends to the model besides the conversation: the system prompt, tool definitions, and injected instructions. Each `context_update` is a change to the run's current set of parts.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `parts` | list of context parts | no (default empty) | Parts added or replaced from this point on. |
| `remove` | list of strings | no (default empty) | Keys of parts that are no longer sent. |

Context part:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `key` | string | yes | Stable identifier within the run. A later part with the same key replaces this one. |
| `kind` | context part kind | yes | What sort of part it is. See below. |
| `label` | string | yes | Short name for people, for example `"skills list"`. |
| `text` | string | yes | The text as sent to the model, or the harness's best rendering of it (tool definitions as JSON). |

Context part kind: one of `"system_prompt"`, `"tool_definitions"`, `"instructions"` (files the user wrote, such as project instructions), `"reminder"` (other injected text), or `{ "other": "<name>" }`. A reader that meets an unknown plain string treats it as `{ "other": "<that string>" }`.

Rules:

- A `context_update` belongs to the nearest enclosing `run`. It applies to every model call that comes after it in that run, in tree order, and not to nested runs.
- The hidden context of a call is the result of applying every earlier `context_update` of its run in order: set or replace each part by key, then drop each key in `remove`.
- Applying is independent of compaction markers. A harness whose compaction drops parts sends a `context_update` with `remove`.
- A `context_update` has no children.
- It is not drawn as a box in the diagram.

## Shared types

**Content list**: an array of blocks. Each block has a `type`:

- `{ "type": "text", "text": "..." }`
- `{ "type": "thinking", "text": "..." }` (text may be empty if the provider hid it)
- `{ "type": "image", "media_type": "image/png" }` (the image data is not kept)
- `{ "type": "other", "kind": "..." }` for anything else; the original is in `raw`

**Stop reason**: one of `"end_turn"`, `"tool_use"`, `"max_tokens"`, `"stop_sequence"`, or `{ "other": "<provider's name>" }`.

**Usage**: all fields optional, all whole numbers.

| Field | Meaning |
|---|---|
| `input_tokens` | Input tokens not read from or written to a cache. |
| `output_tokens` | Tokens the model generated. |
| `cache_read_tokens` | Input tokens read from the prompt cache. |
| `cache_write_tokens` | Input tokens written to the prompt cache. |

The total context size of a call is `input_tokens + cache_read_tokens + cache_write_tokens`. This is what the diagram uses to show context growth.

**Raw source**: `{ "source": "<file or stream name>", "line": <1-based line number>, "text": "<the record exactly as read>" }`. One node can have several, because some harnesses spread one model response over several records. A harness that emits this format directly can leave `raw` out.

## Rules

1. **Parents first.** A node's parent must be sent before the node.
2. **Re-sending replaces.** An event with a known `id` replaces that node's data. It must keep the same `parent_id` and the same kind. Anything else is rejected.
3. **Order is first-sent order.** Siblings are shown in the order they were first sent. A re-sent node keeps its place.
4. **Unknowns are allowed.** Leave out any optional field you do not know. Readers must not assume a missing count is zero.
5. **Extras go in `metadata`.** If a field only makes sense for one harness, put it in `metadata` instead of inventing a core field.

## Example

A tool call is sent when the model asks for it, then re-sent once the result is in:

```json
{"id":"tc-1","parent_id":"mc-1","node":{"type":"tool_call","name":"Read","input":{"path":"hello.txt"}},"started_at_ms":1767225601900}
{"id":"tc-1","parent_id":"mc-1","node":{"type":"tool_call","name":"Read","input":{"path":"hello.txt"},"result":{"content":[{"type":"text","text":"Hello"}],"is_error":false}},"started_at_ms":1767225601900,"ended_at_ms":1767225602000}
```

See `fixtures/trace-core/example-trace.jsonl` for a full trace with parallel tool calls, a subagent and a compaction marker.
