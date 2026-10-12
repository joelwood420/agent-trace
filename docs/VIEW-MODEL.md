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
| `hidden_context` | list | The main agent's hidden context as the transcript records it (5.1b): the final state of its `context_update` parts in first-set order, each row `{ "trace_id", "key", "kind", "label", "chars", "count", "part_keys" }`. Tool definition parts are collapsed into one row per group, grouped like the breakdown (`built-in` or `MCP: <server>`): `key` is `tools:<group>`, `label` is the group, `chars` is the sum, `count` is the number of tools and `part_keys` lists the member part keys; the row sits where its first tool was set. Every other part is its own row with `count` 1 and `part_keys` holding just its key. `trace_id` is the `context_update` node that last set the part (for a group, the one that most recently set any of its tools), so `node_detail` on it shows the full text and raw line. Subagent parts are left out. |

`call_context(project, session_id, trace_id)` returns one call's `ContextBreakdown`, or null if the id is not a model call.

`ContextBreakdown`:

| Field | Type | Meaning |
|---|---|---|
| `source` | string | `captured` (measured from the raw request) or `transcript` (measured from the trace alone). |
| `total_tokens` | integer | The call's input context in tokens. |
| `total_is_reported` | boolean | True when the total is the API's reported count; false when it was estimated at 4 characters per token. |
| `slices` | list | In this order, empty ones left out: `system_prompt`, `tool_definitions`, `instructions`, `files_read`, `tool_results`, `conversation`, `not_captured`. Each has `kind`, `label` (display name), `tokens`, `share` (0 to 1) and `items`. The slice tokens add up exactly to `total_tokens`. |
| `advice` | list | `{ "level": "warn" or "info", "slice", "text" }`, finished sentences, most important first. |

An item is `{ "label", "tokens", "count", "largest_tokens", "from_transcript" }` (`from_transcript` is true when it comes from a transcript `context_update` part rather than the conversation or a capture): for example one file read (`count` is how many times it is in context), one MCP server (`count` is its number of tools) or one reminder section. Items are sorted largest first. Item tokens are rounded separately and need not add up to the slice.

`ContextBar` is the same without labels, items or advice: `source`, `total_tokens`, `total_is_reported` and `slices` as `{ "kind", "tokens" }`.

All token splits are estimates: each part is measured in characters and scaled to the reported total. See `DECISIONS.md` (2026-10-08).

## Flags (M5.2)

The `session_flags` command returns the waste and problem flags of one session. It works with capture off, and reads the open session when it is the same one.

`SessionFlags`:

| Field | Type | Meaning |
|---|---|---|
| `flags` | list | Every flag found, in session order of each flag's first node. |

`Flag`:

| Field | Type | Meaning |
|---|---|---|
| `kind` | string | The pattern; see the list below. |
| `level` | string | `warn` or `info`. |
| `text` | string | A finished plain sentence. Never empty. |
| `trace_ids` | list of string | Every node involved, in session order. |
| `run_id` | string | The run (main agent or one subagent) the flag is about. Runs are checked separately. |

Kinds: `repeated_read` (the same file read 3 or more times with no edit in between), `failed_retry` (a failed tool call retried with the same input), `loop` (the same tool call 3 or more times in one prompt with no edit in between), `large_result` (one tool result of about 10k tokens or more), `compaction` (the conversation was compacted), `reread_after_compaction` (a file read again after compaction dropped it).

Placement rule: the UI puts a flag on every box whose `trace_ids` include one of the flag's `trace_ids`. A summary box that holds such a box gets the flag too, and a flag is counted once per screen.

## Compare runs (M5.3)

The `compare_runs` command puts two runs side by side. A run is a whole session or one prompt of it. The numbers come from `insights::compare`; the UI shows the rows as they are and does no maths. If either side fails to load, the whole command fails with one error that names the side.

Request: `a` and `b`, each a `RunRef`.

`RunRef`:

| Field | Type | Meaning |
|---|---|---|
| `project` | string | The encoded project folder name. |
| `session_id` | string | The session id. |
| `prompt_trace_id` | string, optional | A main-run turn to limit the side to. Missing means the whole session. |

`CompareView` (the response):

| Field | Type | Meaning |
|---|---|---|
| `comparison` | `Comparison` | The two summaries and the rows. |
| `a`, `b` | `CompareSide` | Per-side data the picker needs. |
| `alignment` | `Alignment` | The two runs lined up step by step (see below). |

`CompareSide`:

| Field | Type | Meaning |
|---|---|---|
| `prompts` | list of `PromptChoice` | Every prompt of the session, for the scope drop-down. |
| `scope_missing` | bool | True when the asked-for prompt was not found. The side then shows the whole session. |

`PromptChoice`: `turn_id` (the turn's trace id), `index` (position from 0, as in the diagram), `preview` (a one-line preview of the prompt).

`Comparison`:

| Field | Type | Meaning |
|---|---|---|
| `a`, `b` | `RunSummary` | One summary per run. |
| `sections` | list of `Section` | The rows to show, in display order. |

`Section`: `title` and `rows`.

`Row`:

| Field | Type | Meaning |
|---|---|---|
| `label` | string | What the row is. |
| `unit` | string | `count`, `tokens` or `ms`. |
| `a`, `b` | number or null | Each side's value; null means unknown. |
| `a_partial`, `b_partial` | bool | True when that value counts only some calls. |
| `difference` | number or null | `b - a`, when both are known. |
| `higher` | string | `a`, `b`, `equal` or `unknown` (at least one side unknown). |
| `presence` | string | For named rows (tools, models, tool groups, instruction parts): `both`, `only_a` or `only_b`. Always `both` for every other row. |

`RunSummary` (one run reduced to its numbers):

| Field | Type | Meaning |
|---|---|---|
| `first_call_source` | string or null | Where the first call's context split came from (captured or transcript). |
| `first_call_tokens` | number or null | The first main-run call's input context. |
| `system_prompt_tokens`, `tool_definition_tokens`, `instruction_tokens` | number or null | The setup slices of the first call. `null` means the slice was not seen, which is not the same as 0. |
| `other_context_tokens` | number or null | The rest of the first call's context that was seen: files read, tool results and conversation. 0 when the first call has a breakdown but none of these; `null` without a breakdown. |
| `not_captured_tokens` | number or null | First-call context that could not be seen (the breakdown's "Not captured" slice). 0 when the breakdown has no such slice; `null` without a breakdown. The Setup section shows it as its own "Not captured" row, apart from "Everything else". |
| `tool_count` | number or null | Tools defined in the first call. |
| `tool_groups`, `instruction_parts` | list of `Named`, or null | Tools per group and instruction parts with tokens, for the first call. |
| `models` | list of `Named` | Model calls per model name. |
| `prompts`, `model_calls`, `tool_calls`, `subagents` | number | Counts. Model and tool calls include subagent runs. |
| `tools_by_name` | list of `Named` | Tool calls per tool name. |
| `wall_ms` | number or null | From the first start to the last end in scope. For a whole session that includes idle time between prompts. |
| `model_ms`, `tool_ms` | `Total` | Summed durations. |
| `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens` | `Total` | Summed over every model call. |
| `largest_context` | number or null | The largest reported context of one main-run call. |
| `context_per_call` | list of `CallPoint` | The main run's calls in order (`index`, `trace_id`, `tokens`), for the chart. |
| `flags_by_kind` | list of `KindCount` | A count for every flag kind, in a fixed order. |
| `failed_tool_calls`, `compactions` | number | Counts. |
| `final_message` | string or null | The text of the last main-run model call that wrote text. |

`Total` is `value` (null if no call had the number) and `partial` (true when some calls had it and some did not). `Named` is `name` and `value`. `KindCount` is `kind` (a flag kind, as in the flags section) and `count` (flags of that kind whose first node is in scope).

Words used above:

- Wall-clock time: from the first event to the last in scope. For a whole session it includes idle time between prompts.
- Time in model calls and time in tools: summed durations. Tool calls run in parallel and background subagents run beside the main agent, so these sums can be larger than the wall-clock time. A model call is timed from the event before it, so a call that waited on an idle gap (a resumed subagent, a late background notification) counts that wait too.
- Main run: the top-level agent. Per-call setup, the context chart and "Largest context" use the main run only.
- Subagent: a run nested in a tool call. Counts and totals include subagents.
- Partial: some calls had no number, so the value is a lower bound. The UI marks it.
- Unknown: nothing was seen. The UI shows unknown as `?`, never as 0, and a row with an unknown side has `higher` set to `unknown`.

Named rows show 0 for a name that one side lacks and carry `only_a` or `only_b` in `presence`, so the UI can tag them "only in". When one side's whole list is unknown (for example tool groups of a transcript-only side), that side's value is `null`, not 0, and `presence` stays `both`.

A prompt that is not found (for example the session changed) falls back to the whole session and sets `scope_missing` on that side.

### Step alignment (M5.3b)

`CompareView.alignment` lines the two runs up step by step. It comes from `insights::align` and the UI shows it as it is. A step is one tool call of the main run. Calls inside a subagent are not steps: a subagent is one step, the call that spawned it.

`Alignment`:

| Field | Type | Meaning |
|---|---|---|
| `pairs` | list of `PromptPair` | Prompt pairs in order. |
| `first_difference` | `DiffPoint`, optional | The first difference over all pairs. Missing when the runs match. |

`DiffPoint` is `pair` (index into `pairs`) and `row` (index into that pair's rows).

`PromptPair`:

| Field | Type | Meaning |
|---|---|---|
| `a`, `b` | `PromptSide`, optional | The prompt on each side. One is missing when only one run has the prompt. |
| `rows` | list of `AlignRow` | The steps lined up. |
| `first_difference` | number, optional | Index of the first row that is not the same clean step. `0` for a one-sided pair, even with no rows. |
| `approximate` | bool | True when the pair was too long to diff exactly and was matched in order instead. |

`PromptSide`: `turn_id` (the turn's trace id), `index` (position among the main run's prompts, from 0), `preview` (one line of the prompt, 80 characters at most).

`AlignRow`:

| Field | Type | Meaning |
|---|---|---|
| `kind` | string | `both`, `only_a` or `only_b`. |
| `a`, `b` | `Step`, optional | Each side's step. A one-sided row has only one. |
| `differs` | bool | Both took the step but exactly one of them failed. |

`Step`: `trace_id` (the tool call's id), `label` (`Tool(target)`, or `Tool` with no target; the target is cut to 40 characters), `failed` (the result is an error), `running` (no result yet) and `duration_ms` (number or null).

How steps line up:

- Step key: the tool name plus its target, with runs of whitespace collapsed to one space. Which input fields make the target is data from the adapter (`AlignRules`): fields per tool, fallback fields for other tools, and prompt prefixes that mark a notification. A tool with no target keys on its name alone.
- Parallel calls (siblings of one model call) are sorted by key, so the same set in another order matches.
- Prompts are paired first, by prompt text, with the same diff. Notification prompts share one key, so they pair by order. When both sides are limited to one prompt, those two prompts pair whatever their text.
- Each pair's steps are lined up with a longest-common-subsequence diff. Swapped steps show as one match plus two one-sided rows.
- `first_difference` of a pair is the first row that is one-sided or has `differs` set.
- `approximate` is set when steps of A times steps of B passes `MAX_CELLS` (4,000,000). The pair is then matched in order: equal keys at the same position are `both`, otherwise the two steps are shown as an `only_a` row then an `only_b` row.

## Example

The JSON for `fixtures/trace-core/example-trace.jsonl` is checked in at `crates/trace-view/tests/snapshots/example-trace.diagram.json`. A test fails if the output changes, so the contract cannot drift without a visible diff. To accept an intended change, run the tests with `SNITCHCRAFT_UPDATE_SNAPSHOTS=1` set and review the diff.

To see the model for any session:

```powershell
cargo run -p trace-view --example print_diagram -- <session.jsonl>
cargo run -p trace-view --example print_diagram -- --json <session.jsonl>
cargo run -p trace-view --example print_diagram -- --counts <session.jsonl>
```

`--counts` prints only box counts per kind and a duplicate id check, never content.
