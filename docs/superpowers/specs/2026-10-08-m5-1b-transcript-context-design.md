# M5.1b: hidden context from transcripts

## Goal

Shrink the grey "not captured" slice for sessions that were not run through the proxy. Recent Claude Code transcripts already record most of the hidden context a model call receives (system prompt, instruction files, skills list, MCP and agent lists, hook output, environment). Carry it into the trace as a new harness-neutral node kind and use it in the context breakdown. What stays grey is mainly tool definitions, which transcripts do not record.

## Decisions made while designing (chosen by the project owner)

- A new trace node kind, `context_update`, rather than metadata on a marker or a field on every model call.
- `context_update` nodes are not drawn as diagram boxes. They feed the breakdown, a "Hidden context" list in the session overview, and the details panel.

Made by the coordinator:

- Each run keeps a set of parts by key; a part with a known key replaces it, `remove` drops keys. Nothing is implied at compaction; a harness that drops parts says so.
- Claude Code reminders each get their own key (they pile up in the real request's history). At a compaction the adapter removes the reminder parts it emitted earlier in that run.
- The hook marker for `hook_additional_context` stays; the same line also gives a reminder part.

## What the transcripts record (checked on 2026-10-08, structure and sizes only)

At session start, before the first model call, a burst of `attachment` lines: `hook_additional_context`, `environment`, `model`, `deferred_tools_delta`, `agent_listing_delta`, `mcp_instructions_delta`, `skill_listing`, `instructions`, `session_context`, `date`, `prompt_snapshot`, then `deferred_tools_record`. A second `prompt_snapshot` follows after the first call. Later lines are mostly changes (`mcp_instructions_delta`, `deferred_tools_*`, `environment`, `date`, `skill_listing`). Subagent transcripts carry their own set.

Sizes in a captured session matched the real request closely: `prompt_snapshot.systemPrompt` about 27.5k characters (the request's system prompt), `skill_listing.content` 8,138 against an 8,056-character skills reminder, `mcp_instructions_delta` 3,813 against 3,846, `agent_listing_delta` 3,274 against 3,246, `hook_additional_context` 3,509 against 3,445.

Some `prompt_snapshot` lines (seen in the sanitised fixture) also carry a `tools` array of tool definitions; current real transcripts do not.

## Schema: `context_update`

New `Node::ContextUpdate(ContextUpdate)` in `trace-core`, documented in `docs/SCHEMA.md`:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `parts` | list of context parts | no (default empty) | Parts added or replaced from this point on. |
| `remove` | list of strings | no (default empty) | Keys of parts that are no longer sent. |

Context part:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `key` | string | yes | Stable identifier within the run. A later part with the same key replaces this one. |
| `kind` | context part kind | yes | `"system_prompt"`, `"tool_definitions"`, `"instructions"` (files the user wrote, such as CLAUDE.md), `"reminder"` (other injected text), or `{ "other": "<name>" }`. |
| `label` | string | yes | Short name for people, for example `"CLAUDE.md: <path>"` or `"skills list"`. |
| `text` | string | yes | The text as sent to the model, or the harness's best rendering of it (tool definitions as JSON). |

Rules (added to SCHEMA.md):

- A `context_update` belongs to the nearest enclosing `run`. It applies to every model call that comes after it in that run, in tree order, and not to nested runs.
- The hidden context of a call is the result of applying every earlier `context_update` of its run in order: set or replace each part by key, then drop each key in `remove`.
- Applying is independent of compaction markers. A harness whose compaction drops parts sends a `context_update` with `remove`.
- A `context_update` has no children.

## Adapter: Claude Code mapping

Each attachment line below becomes one `context_update` event, a child of the current turn or the run (the same parent rule as markers), with the raw line kept. Text comes from the attachment fields; anything missing gives no part, never a panic.

| Attachment | Part key | Kind | Label | Text |
|---|---|---|---|---|
| `prompt_snapshot` | `system` | system_prompt | `System prompt` | `systemPrompt` strings joined with a blank line |
| `prompt_snapshot` with `tools` | `tool:<name>` per tool (`tool:#<index>` without a string `name`) | tool_definitions | `<name>` (`tool <index>`) | that tool object as compact JSON |
| `instructions` | `instructions:<path>` per file | instructions | `<file name>: <path>` (for example `CLAUDE.md: C:\x\CLAUDE.md`) | the file's `content` |
| `skill_listing` | `reminder:<line uuid>` | reminder | `skills list` | `content` |
| `mcp_instructions_delta` | `reminder:<line uuid>` | reminder | `MCP server instructions` | `addedBlocks` joined; removed names listed as one line |
| `agent_listing_delta` | `reminder:<line uuid>` | reminder | `agent list` | `addedLines` joined |
| `deferred_tools_delta` | `reminder:<line uuid>` | reminder | `deferred tools list` | `addedLines` joined |
| `hook_additional_context` | `reminder:<line uuid>` | reminder | `hook output` | `content` strings joined |
| `environment` | `reminder:<line uuid>` | reminder | `environment` | `snapshot` (and `changes`) as compact JSON |
| `date` | `reminder:<line uuid>` | reminder | `date` | `date` |
| `model` | `reminder:<line uuid>` | reminder | `model` | `text` |
| `session_context` | `reminder:<line uuid>` | reminder | `session context` | `context` as compact JSON |

A line without a `uuid` uses `reminder:line-<line number>`. A `prompt_snapshot` with `tools` is the full tool set: its `context_update` also removes the `tool:` keys of the run's previous tool snapshot that are not in the new one (tracked per run). A `prompt_snapshot` without `tools` changes nothing about tools.

Other attachment types (`total_tokens_reminder`, `deferred_tools_record`, `auto_mode`, `credential_org`, `remote_session_change`, ...) are skipped as now (logged once at debug level). `hook_*` markers are unchanged.

At a `compact_boundary` (already a `compaction` marker), the adapter also emits a `context_update` that removes every `reminder:` key it emitted earlier in that run. System prompt, tools and instruction parts stay.

## `insights`

- `measure_transcript_all` keeps a per-run context state (key to part) next to the running conversation measure. A `context_update` applies to the state; the state is added to each model call's measure when the call is reached:
  - system_prompt parts go to SystemPrompt (item = label),
  - tool_definitions to ToolDefinitions, grouped the way a captured request's tools are: a label starting with `mcp__` goes to the item `MCP: <server>` (the text up to the next `__`), any other to `built-in`; each tool is one occurrence, so the item's count is its number of tools and advice rules 1 and 2 work as for captured calls,
  - instructions and reminder parts to Instructions (item = label),
  - other parts to Instructions (item = label).
- A compaction marker clears the conversation measure only, never the context state.
- Each measured item records whether it came from the transcript's context parts: `ContextMeasure` gains a flag per slice item or a separate marker set, so `Item` in the breakdown gets `from_transcript: bool` (serialised).
- Advice rule 7 changes when a transcript call had any context parts: "Tool definitions are not visible for this call. The grey part is everything in the reported total that the transcript does not show. Run the session through the capture proxy to see it." Without parts it stays as now.
- Captured calls are unchanged (the capture wins).

## App and UI

- `trace-view`: `context_update` nodes are never boxes, are not counted in "events before the first prompt", and do not affect prompt totals. `node_detail` works for them (full node data and raw line).
- `SessionContext` gains `hidden_context`: the final context state of the main run as a list of `{ trace_id, key, kind, label, chars }` in first-set order, where `trace_id` is the `context_update` node that last set the part. Subagent runs are left out of this list.
- Session overview: a "Hidden context" list under the Context card (kind, label, size in characters, collapsed by default). Clicking a row selects that `context_update` node and the details panel shows its parts with full text and the raw line.
- Details panel for a model call: transcript items marked "from transcript".
- The UI does no measuring; sizes come from Rust.

## Error handling

- Missing or wrongly typed attachment fields give no part (or a part with the fields that exist), never a panic.
- An `instructions` file entry without `path` uses `instructions:<index>` as key and its `type` as label.
- `ContextUpdate` with empty parts and remove is valid and ignored.

## Testing

- `trace-core`: serde round trip for `context_update`; unknown `kind` values parse as `other`.
- Adapter: each mapped attachment type gives the expected part (invented text); a line with missing fields; compaction removes reminder keys only; the sanitised fixture produces context updates (snapshot of counts by kind).
- `insights`: replace by key, remove, nested run isolation, compaction keeps the state but clears conversation, `from_transcript` on items, rule 7 variant.
- App: `hidden_context` from the fixture; snapshots regenerated (`fixture-data.json`, `context-data.json`, `live-steps.json`) and reviewed.
- UI: rendering helpers for the Hidden context list.
- Real-app check: one transcript-only session; its slices compared with a captured session of the same setup.

## Docs to update

- `docs/SCHEMA.md` (the new node kind and rules), `docs/VIEW-MODEL.md` (`hidden_context`, `from_transcript`), `docs/DECISIONS.md`, `docs/HARNESS-NOTES.md` (attachment mapping and what still is not recorded), `README.md`, `CLAUDE.md` milestone status.

## Out of scope

Tool definition sizes when transcripts do not record them (a later idea: estimate from the most recent capture), cost figures, and drawing context updates in the diagram.
