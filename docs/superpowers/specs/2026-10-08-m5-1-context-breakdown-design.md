# M5.1: context breakdown per call

## Goal

Show what fills the model's context on every call, and turn the biggest costs into plain advice such as "MCP server `docs` adds about 9k tokens to every call (14 tools)" or "`src/main.rs` is in context 4 times".

M5 is split into four sub-milestones, each with its own spec, plan and branch: 5.1 context breakdown (this spec), 5.2 waste and problem flags, 5.3 compare two runs, 5.4 shareable report.

## Decisions made while designing

Chosen by the project owner:

- Works on every session. A call with a capture gets the full split. A call with only the transcript gets the conversation split, and the rest is one grey "not captured" slice with a hint to use the proxy.
- Shown in three places: a thin stacked bar on every model-call box, a fuller breakdown in the details panel, and a "Context" card with the advice in the session overview.
- The analysis lives in a new `insights` crate: pure functions, no I/O, nothing Claude Code specific. The two Claude Code specific rules (what an injected reminder looks like, which tool reads files) are data supplied by the adapter.
- The slices are the seven listed below.

Made by the coordinator (owner said "your call" / "go for it"):

- Token counts per slice are estimates: each part is measured in characters, then scaled so the parts add up to the real input total the API reported for that call. No network calls. The UI says "estimated split of N reported tokens".
- The session card shows the latest model call of the main agent, so it follows a live session.
- No cache hit or cost figures (CLAUDE.md lists token cost breakdowns as out of scope).
- `trace-core` and `docs/SCHEMA.md` do not change.

Found while planning (owner chose to defer it): real transcripts also record much of the hidden context, as `attachment` lines: `prompt_snapshot` (the system prompt blocks), `instructions` (each CLAUDE.md path and content), `skill_listing` and `mcp_instructions_delta`. Tool definitions are not recorded, only tool names. Using them needs a trace schema addition, so it becomes its own step, 5.1b, with its own schema design. 5.1 notes the finding in `docs/HARNESS-NOTES.md`.

## Scope

In:

- `insights` crate: measure a captured request body, measure a call from the trace alone, scale to tokens, produce advice.
- Adapter: the Claude Code rules value.
- App: compute breakdowns once per capture (next to the existing summaries) and on demand for transcript-only calls; two new commands.
- UI: bar on model-call boxes, breakdown section in the details panel, Context card in the session overview, mock data.
- Fixture work: the invented capture fixture gains reminder blocks, MCP tools and `Read` results so every slice has content.

Out:

- Waste detection (loops, retries, compaction losses): 5.2. 5.1 only reports what is in context right now.
- Context window limits per model, cache and cost figures, comparing runs, exporting.

## The slices

Every call's input splits into these slices, in this order. Each slice has items for the details panel.

| # | Slice | What counts | Items |
|---|-------|-------------|-------|
| 1 | System prompt | the request's `system` value | one per system block ("block 1", "block 2") |
| 2 | Tool definitions | the request's `tools` array, compact JSON length of each | "built-in" plus one per MCP server, from the `mcp__<server>__` name prefix, each with its tool count |
| 3 | Instructions and reminders | text between the reminder tags in any message | one per section, labelled by the rules (for example "CLAUDE.md: <path>", "memory: <path>", "skills list", "other reminders") |
| 4 | Files read | results of tool calls the rules mark as file reads | one per file path, with how many times it is in context |
| 5 | Other tool results | every other tool result | one per tool name, with count |
| 6 | Conversation | prompt text outside reminders, model text and thinking, tool call inputs | "your prompts", "model replies", "tool inputs" |
| 7 | Not captured | transcript-only calls: reported total minus slices 4 to 6 | none; labelled "system prompt, tools and instructions (not captured)" |

Measuring rules:

- Text counts its character length. JSON values (tool definitions, tool inputs, unknown blocks) count the length of their compact JSON text.
- An image counts as a fixed 6,000 characters (about 1,500 tokens), so it is visible without decoding it.
- In a captured request, a `tool_result` block is matched to its tool by the `tool_use` block with the same id in an earlier assistant message of the same request. A result whose tool cannot be found goes to "Other tool results" as "unknown tool".
- Reminder splitting applies to every text in user messages, including text inside `tool_result` content, because harnesses also append reminders to tool results.
- A reminder section is the text between the opening and closing reminder tags. Inside one reminder, every occurrence of a section marker (for Claude Code: `Contents of `) starts a new item, so one reminder holding three CLAUDE.md files gives three items. An item that starts with the marker is labelled by matching the rules against its header line only (the text after the marker up to the first line end), so a CLAUDE.md whose body mentions MEMORY.md is still a CLAUDE.md item. Its detail is the header line up to its last ` (`, or the whole line if there is none, trimmed and without a trailing `:`, so a path that contains ` (` is kept whole. An item that starts with the marker but matches no rule is labelled "file: <detail>". Text before the first marker, or a reminder with no marker, is labelled by the first rule whose needle appears anywhere in it, else "other reminders".

Scaling:

- `tokens_per_char = reported_total / measured_chars` for captured calls, where `reported_total` is the model call's `Usage::context_tokens()` from the trace.
- If a captured call has no reported total (no usage yet, or the capture maps to no model call), use the fixed estimate of 4 characters per token and mark the total as estimated.
- Transcript-only calls always use 4 characters per token for slices 4 to 6. Slice 7 is `reported_total` minus their sum, never below zero. Without a reported total there is no slice 7 and the total is marked estimated.
- If the transcript estimate of slices 4 to 6 is larger than the reported total, they are scaled down to fit it and slice 7 is zero.
- Rounding: each slice rounds to whole tokens, and the largest slice absorbs the rounding difference so the slices add up exactly to the total shown.

## Measuring a call from the trace

For a transcript-only model call, its context is everything earlier in the same run (a subagent's run is separate from the main run):

- every `Turn` prompt in the run up to and including the turn holding this call,
- the output text and thinking of every earlier model call in the run,
- the input and result of every tool call of every earlier model call in the run.

A `Marker` of kind `compaction` in the run resets what has been collected so far, because the history before it is no longer sent. Tool calls that spawned a subagent count their own input and result, never the nested run's content. Prompt text is never treated as reminders here, because the transcript does not record injected reminders (they live in slice 7).

## The `insights` crate

Depends on `trace-core`, `serde` and `serde_json` only. Public surface, kept small:

- `ContextRules`: plain data the adapter supplies.
  - `reminder_open` and `reminder_close`: the tags around injected text.
  - `section_marker`: text that starts a new item inside a reminder.
  - `labels`: a list of `(needle, label)` pairs. The first pair whose needle appears in a section names it. A label may take the text after the section marker up to the first `" ("` or line end as its detail, for example `CLAUDE.md: <path>`.
  - `file_reads`: a list of `(tool name, input field)` pairs meaning "this tool reads the file named in this field".
- `measure_request(body: &serde_json::Value, rules: &ContextRules) -> ContextMeasure`: measures a captured request body. Unknown shapes are skipped, never a panic; a body that is not an object gives an empty measure.
- `measure_transcript(trace: &Trace, model_call_id: &str, rules: &ContextRules) -> Option<ContextMeasure>`: `None` if the id is not a model call.
- `breakdown(measure: &ContextMeasure, reported_total: Option<u64>) -> ContextBreakdown`: scales to tokens and adds the advice.
- `ContextBreakdown::bar(&self) -> ContextBar`: the same without items or advice, for the boxes.

`ContextMeasure` keeps characters per slice and per item plus whether it came from a capture. It is what the app caches, because scaling is cheap and the reported total can arrive later than the capture.

`ContextBreakdown` (serialised to the UI):

- `source`: `captured` or `transcript`.
- `total_tokens`, and `total_is_reported` (false when the 4-characters estimate was used).
- `slices`: in the order above, empty slices left out. Each has `kind`, `label`, `tokens`, `share` (0 to 1) and `items` (each with `label`, `tokens`, `count`, and `largest_tokens`: the biggest single occurrence, used by advice rule 5), largest item first.
- `advice`: a list of `{ level: "warn" | "info", slice, text }`, most important first.

`ContextBar`: `source`, `total_tokens`, `total_is_reported`, and `slices` as `(kind, tokens)` only.

## Advice rules

Advice is built in Rust as finished sentences; the UI only shows it. Thresholds are named constants in one place. Rules, in output order:

1. **warn** Tool definitions are at least 20% of the call: "Tool definitions are 38% of this call (about 41k tokens): 186 tools, 63 from MCP servers."
2. **warn** each MCP server with at least 2,000 tokens: "MCP server `docs` adds about 9k tokens to every call (14 tools). Turn it off in projects that do not use it."
3. **warn** instructions and reminders of at least 5,000 tokens: "Instructions and reminders add about 7k tokens to every call. Largest: CLAUDE.md: <path> (about 4k)."
4. **warn** a file in context 3 or more times: "`src/main.rs` is in context 4 times (about 12k tokens). Each read adds the full file again."
5. **warn** a single file read or other tool result of at least 10% of the call: "One `Bash` result is 14% of this call (about 15k tokens)."
6. **info** conversation is at least 60% of the call: "Conversation history is 64% of this call. /compact or a fresh session would shrink it."
7. **info** transcript-only call: "The system prompt, tool definitions and instructions are not visible for this call. Run Claude Code through the proxy to see them."

Numbers round to the nearest thousand as "about Nk" from 1,000 tokens up, else exact. Percentages are whole numbers.

## App wiring

- The adapter gains `context_rules() -> ContextRules`, so the adapter depends on `insights`. `insights` never depends on the adapter.
- `SessionRecords` keeps a `ContextMeasure` per capture, built in `summarise_missing` from the same decoded body as the summary, so each body is decoded once.
- New command `session_context(project, session_id) -> SessionContext`:
  - `bars`: a map from model call trace id to `ContextBar`, for every model call in the trace. A call with a mapped capture uses the capture's measure, otherwise the transcript measure.
  - `latest`: the full `ContextBreakdown` of the latest model call of the main run, or `None` if there is none, and `latest_trace_id`, its id, so the card can select it.
- New command `call_context(project, session_id, trace_id) -> Option<ContextBreakdown>` for the details panel.
- Trace facts are copied under the session lock as today; measuring runs outside the lock.
- Transcript measures are cheap for one call but a session has hundreds. `session_context` measures all calls of a run in one pass, carrying the running total forward, rather than walking the run again per call.
- The UI reloads `session_context` on the same throttled triggers as the capture overview.

## UI

- **Model-call box:** a thin bar (about 6 px) under the existing label, one colour per slice, grey hatching for "not captured". The tooltip lists the slices with tokens. No bar until the call has usage or a capture.
- **Details panel** (model call selected): a "Context" section with the bar at full width, a legend with tokens and share per slice, the slice items in collapsible lists, the advice for this call, and the note "Estimated split of N reported tokens" (or "Estimated total" when not reported).
- **Session overview:** a "Context" card at the top showing the latest main-agent call: the bar, the three largest slices, and the advice. Clicking it selects that call.
- Slice colours are one fixed palette shared by all three places, checked for contrast in light and dark themes.
- The UI does no measuring or scaling. It renders `SessionContext` and `ContextBreakdown` as given.
- Mock mode gets `context-data.json`, written by the same snapshot test switch as the other mock files.

## Error handling

- A capture body that cannot be decoded has no measure; its call falls back to the transcript measure.
- Unknown block types are measured as compact JSON in "Conversation" and logged once per type at debug level, never a panic.
- A call id that is not a model call gives `None` from `call_context`.
- `session_context` on a session with no model calls returns no bars and no latest.

## Testing

- `insights` unit tests for each measuring rule: system blocks, built-in vs MCP tool grouping, reminder sections with and without markers, file reads matched by tool use id, unknown tool results, images, unknown blocks.
- Scaling tests: slices add up exactly to the reported total; fixed estimate when no total; slice 7 never negative.
- Transcript measure tests on the sanitised Claude Code fixture: history grows call by call, compaction resets, a subagent's run is separate.
- One advice test per rule at, just below and just above its threshold.
- Fixture test: the extended invented capture gives a known breakdown (snapshot of the JSON).
- App tests: `session_context` mixes captured and transcript bars, `latest` picks the main run's last call, measures are built once per capture.
- UI tests: bar segment widths from a `ContextBar`, the legend and advice render, no bar without data.
- Real-app check: one fresh session captured through the proxy, compared by eye against its raw request. Also one old transcript-only session.

## Docs to update

- `docs/VIEW-MODEL.md`: `SessionContext`, `ContextBreakdown`, `ContextBar`.
- `docs/DECISIONS.md`: the new crate, estimate by scaling characters, fixed image size, advice thresholds.
- `docs/HARNESS-NOTES.md`: what a real request's context is made of, slice by slice, from the real-app check.
- `README.md`: feature line and milestone status. `CLAUDE.md`: current milestone.
