# Diagram view model

This is what the UI receives to draw a session. It is built in Rust by `crates/trace-view` from a `trace-core` trace (see `SCHEMA.md`). The UI lays out and draws these boxes; it does not decide what to group, collapse, or label. Every rule below lives in Rust and is covered by tests.

The model is built for one session at a time:

- `build_session(trace)` returns a **SessionDiagram**: run facts, run-level markers, and one **PromptDiagram** per prompt.
- `node_detail(trace, trace_id)` returns a **NodeDetail**: the full content of one trace node, for a details panel.

The diagram carries short labels and summary numbers only. Full prompts, model output, tool input and tool results come from `node_detail`.

All JSON names are snake_case. **Every field is always present.** An unknown value is `null`, never left out.

## SessionDiagram

| Field | Type | Meaning |
|---|---|---|
| `run_id` | string or null | Trace id of the session's run. `null` if the trace has no run. |
| `title` | string or null | Run title, if the harness gave one. |
| `harness` | string or null | Harness name. |
| `started_at_ms` | integer or null | Run start, milliseconds since 1970-01-01 UTC. |
| `ended_at_ms` | integer or null | Run end. |
| `duration_ms` | integer or null | End minus start, when both are known. |
| `markers` | DiagramNode list | Markers that happened outside any prompt (for example a hook at session start), in order. All have kind `marker`. |
| `prompts` | PromptDiagram list | One per top-level prompt, in order. |

If the trace has several root runs, only the first is used.

## PromptDiagram

| Field | Type | Meaning |
|---|---|---|
| `index` | integer | Position in the session, starting at 0. |
| `turn_id` | string | Trace id of the turn. |
| `prompt_preview` | string | The prompt on one line, at most 80 characters. Same as `root.label`. |
| `started_at_ms`, `ended_at_ms`, `duration_ms` | integer or null | Turn timing. |
| `totals` | object | See below. |
| `root` | DiagramNode | The tree. Always kind `prompt`. |

`totals`:

| Field | Type | Meaning |
|---|---|---|
| `model_calls` | integer | Model calls in this prompt, including subagents. |
| `tool_calls` | integer | Tool calls, including subagents. |
| `errors` | integer | Tool calls whose result is an error, including subagents. |
| `max_context_tokens` | integer or null | Largest context size of a model call made by the main agent. Subagents are left out because they have their own context window. |
| `output_tokens` | integer or null | Sum of output tokens, including subagents. `null` if no call reported any. |

## DiagramNode

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Unique within the session diagram, and the same every time the same trace is built. See "Ids". |
| `kind` | string | `prompt`, `model_call`, `tool_call`, `parallel_group`, `subagent`, `summary` or `marker`. |
| `label` | string | Short main text. |
| `detail_label` | string or null | Second line. |
| `started_at_ms` | integer or null | Start time. |
| `duration_ms` | integer or null | Duration. For groups and summaries, from the first start to the last end of everything inside. |
| `context_tokens` | integer or null | Context size in tokens. For boxes holding several model calls, the largest. |
| `output_tokens` | integer or null | Output tokens. For boxes holding several model calls, the sum. |
| `status` | string | `ok`, `error`, `running` or `none`. See "Status". |
| `collapsed_by_default` | boolean | `true` if the box should start with its children hidden. |
| `trace_ids` | string list | The trace node ids this box stands for. Pass one to `node_detail`. |
| `children` | DiagramNode list | Child boxes, in order. |

### What each kind holds

| Kind | Label | Detail label | Tokens | Children | Trace ids |
|---|---|---|---|---|---|
| `prompt` | Prompt preview | Counts, for example `3 model calls, 2 tool calls, 1 error` | Prompt totals | Model calls, markers and summaries, in order | The turn |
| `model_call` | `Model call` | Model and stop reason, for example `claude-opus-5-5, stop: tool_use` | Its own | Its tool call, or one `parallel_group` | The model call |
| `tool_call` | Tool name and input hint, for example `Read(hello.txt)` | `null` | `null` | A `subagent` box if the tool started one, else none | The tool call |
| `parallel_group` | `2 tool calls in parallel` | Tool names, for example `Grep x2` or `Read, ListFiles` | `null` | The tool calls | The tool calls |
| `subagent` | Run title, or `Subagent` | Counts | Subagent totals | The subagent's model calls, markers and summaries | The run, plus its turn if it has exactly one |
| `summary` | Count and tool, for example `5 x Read` | `5 model calls`, plus `, 2 markers` if any | Max context, summed output | The original boxes | Those boxes' trace ids |
| `marker` | Marker summary, at most 80 characters | Marker kind, for example `hook` or `compaction` | `null` | None | The marker |

## The rules

### Tree shape

- The root of each prompt diagram is the prompt. Its children are the turn's model calls and markers in the order they happened (after summaries are applied).
- A model call's children are its tool calls.

### Parallel groups

If a model call has **two or more** tool calls, they are wrapped in one `parallel_group` box, which is the model call's only tool child. A model call with one tool call has that tool call as its direct child. In the trace schema, all tool calls under one model call were requested together, which is why they count as parallel.

### Subagents

A subagent is any tool call that has a child run. It is not tied to any tool name. The tool call stays a normal `tool_call` box, and the subagent becomes a `subagent` child box under it, with `collapsed_by_default: true`. This keeps the tool call visible with its own status and timing, and gives the UI one box to expand.

Inside the `subagent` box:

- If the subagent run has exactly one turn (the usual case), that turn's model calls and markers are the subagent box's children directly, and the turn id is added to `trace_ids` so the UI can still show the subagent's task.
- If it has several turns, each becomes a `prompt` box.
- Markers directly under the subagent run are children too, in order.
- Summaries and parallel groups apply inside subagents with the same rules.

### Summaries

A run of similar model calls collapses into one `summary` box with `collapsed_by_default: true`. Its children are the original boxes, unchanged.

A model call is **similar** if all of these hold:

- it has finished (it has a stop reason or an end time);
- it has exactly one child, and that child is a tool call;
- that tool call has finished without an error;
- that tool call did not start a subagent.

A **run** is a sequence of sibling boxes that starts with a similar model call, continues over similar model calls **with the same tool name**, and may have markers between them. Markers inside a run are kept inside the summary. A run never starts or ends with a marker. Any other box (a model call that is not similar, or one using a different tool) ends the run.

A run becomes a summary only if it holds at least **3** similar model calls (`SUMMARY_MIN_CALLS`). Two in a row are shown as they are.

Markers are allowed inside a run because some harnesses log something between every step. For example, a hook that runs before each tool call would otherwise stop every summary from forming.

### Status

- `tool_call`: `running` until it has a result, then `error` if the result is an error, else `ok`.
- `model_call`: `running` until it has a stop reason or an end time, then `ok`. A failing tool call does not change its model call's status.
- `marker`: `none`.
- `prompt`, `parallel_group`, `subagent`, `summary`: the worst status of everything inside, at any depth: `error` beats `running`, `running` beats `ok`. So an error inside a collapsed box still shows on the box.

A tool call that never gets a result (for example because the session was interrupted) stays `running`.

### Ids

Each box's `id` is its kind and a trace id joined by a colon:

| Kind | Id |
|---|---|
| `prompt` | `prompt:<turn id>` |
| `model_call` | `model_call:<model call id>` |
| `tool_call` | `tool_call:<tool call id>` |
| `marker` | `marker:<marker id>` |
| `subagent` | `subagent:<run id>` |
| `parallel_group` | `parallel_group:<model call id>` |
| `summary` | `summary:<id of the first box inside>` |

Trace ids are unique, so these are unique, and they only depend on the trace, so they are stable across reloads. A box id is not a trace id. Use `trace_ids` to ask for details.

### Labels

- Text is put on one line (runs of whitespace become one space) and cut with `...` when too long: prompts and markers at 80 characters, tool input hints at 40.
- Tool input hint: if the input is a string, that string. If it is an object, the first non-empty string value under one of the keys `command`, `file_path`, `pattern`, `query`, `url`, `path` (in that order), else the first non-empty string value in alphabetical key order. If there is none, the label is the tool name alone.
- A prompt with no text shows `(image)` if it has an image, else `(no text)`.

## NodeDetail

Returned by `node_detail(trace, trace_id)`, or nothing if the id is unknown.

| Field | Type | Meaning |
|---|---|---|
| `trace_id` | string | The trace node's id. |
| `parent_id` | string or null | Its parent's id. |
| `kind` | string | Trace node kind: `run`, `turn`, `model_call`, `tool_call` or `marker`. |
| `started_at_ms`, `ended_at_ms`, `duration_ms` | integer or null | Timing. |
| `context_tokens` | integer or null | For a model call, its total context size. |
| `node` | object | The node's full data, exactly as the `node` field in `SCHEMA.md`: the prompt, model output, stop reason and usage, tool input and result with `is_error`, or marker kind and summary. |
| `metadata` | object | Harness-specific extras. |
| `raw` | list | The original source records: `{ "source", "line", "text" }`. |

## Context breakdown (M5.1)

Two commands describe what fills each model call's context. They are built by the `insights` crate (with Claude Code rules from the adapter) and are separate from the diagram model, so the diagram contract above does not change.

`session_context(project, session_id)` returns a `SessionContext`:

| Field | Type | Meaning |
|---|---|---|
| `bars` | object | Trace id of a model call to its `ContextBar`. A call with no reported token total and no capture has no entry. |
| `latest` | `ContextBreakdown` or null | The latest model call of the main agent, in full. |
| `latest_trace_id` | string or null | That call's trace id (may be set while `latest` is null, when the call has no token counts yet). |

`call_context(project, session_id, trace_id)` returns one call's `ContextBreakdown`, or null if the id is not a model call.

`ContextBreakdown`:

| Field | Type | Meaning |
|---|---|---|
| `source` | string | `captured` (measured from the raw request) or `transcript` (measured from the trace alone). |
| `total_tokens` | integer | The call's input context in tokens. |
| `total_is_reported` | boolean | True when the total is the API's reported count; false when it was estimated at 4 characters per token. |
| `slices` | list | In this order, empty ones left out: `system_prompt`, `tool_definitions`, `instructions`, `files_read`, `tool_results`, `conversation`, `not_captured`. Each has `kind`, `label` (display name), `tokens`, `share` (0 to 1) and `items`. The slice tokens add up exactly to `total_tokens`. |
| `advice` | list | `{ "level": "warn" or "info", "slice", "text" }`, finished sentences, most important first. |

An item is `{ "label", "tokens", "count", "largest_tokens" }`: for example one file read (`count` is how many times it is in context), one MCP server (`count` is its number of tools) or one reminder section. Items are sorted largest first. Item tokens are rounded separately and need not add up to the slice.

`ContextBar` is the same without labels, items or advice: `source`, `total_tokens`, `total_is_reported` and `slices` as `{ "kind", "tokens" }`.

All token splits are estimates: each part is measured in characters and scaled to the reported total. See `DECISIONS.md` (2026-10-08).

## Example

The JSON for `fixtures/trace-core/example-trace.jsonl` is checked in at `crates/trace-view/tests/snapshots/example-trace.diagram.json`. A test fails if the output changes, so the contract cannot drift without a visible diff. To accept an intended change, run the tests with `SNITCHCRAFT_UPDATE_SNAPSHOTS=1` set and review the diff.

To see the model for any session:

```powershell
cargo run -p trace-view --example print_diagram -- <session.jsonl>
cargo run -p trace-view --example print_diagram -- --json <session.jsonl>
cargo run -p trace-view --example print_diagram -- --counts <session.jsonl>
```

`--counts` prints only box counts per kind and a duplicate id check, never content.
