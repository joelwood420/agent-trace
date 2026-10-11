# M5.3: compare two runs

## Goal

Put two runs of the same task side by side (for example with a different CLAUDE.md, model, or later a different harness) and show what the change did: the setup that differed, the work done, tokens and context size, problems, and the final answer.

5.3 is the summary comparison. A step-by-step alignment of the two runs (where they first went different ways) is a later sub-milestone, 5.3b.

## Decisions made while designing

Chosen by the project owner:

- Both a summary and a step-by-step alignment are wanted; the summary comes first (this spec), the alignment later.
- Scope, option C: whole sessions by default, and each side can be narrowed to one prompt.
- The summary contents in "The summary" below.
- Architecture, option A: a new `insights::compare` module (pure, harness-neutral, reusing 5.1 and 5.2), a `compare_runs` command, and a Compare view in the UI. The UI does no maths.
- The flow in "UI" below: "Compare with..." in the session overview, pick a session from the list, Compare view in the main area, scope drop-downs, swap, back to the diagram.

## Scope

In:

- `insights`: a `compare` module with `run_summary` and `compare`.
- App: a `compare_runs` command.
- UI: "Compare with..." button, session picker mode, Compare view with a plain SVG line chart, mock data.

Out:

- The step-by-step alignment (5.3b).
- More than two runs, saving comparisons, comparisons across app restarts.
- Token cost in dollars.
- Any schema change. Everything here is computed from the existing trace, the 5.1 context breakdown, the 5.2 flags and captures when present.

## The summary (`insights::compare`)

### Scope

```
Scope = Session | Prompt(turn_trace_id)
```

`Session` covers the main run and every subagent run inside it. `Prompt` covers one turn of the main run and every subagent run started inside that turn.

### `run_summary(trace, scope, inputs) -> RunSummary`

Pure, no I/O. `inputs` carries what the app already has: the `ContextRules` and `FlagRules` from the adapter, and the measured context of the first main-run model call in scope (from a capture if one exists, otherwise the transcript estimate), built by the app the same way `call_context` does.

`RunSummary`:

- **Setup**
  - `models`: each model name with its call count, in first-seen order.
  - `first_call`: the first main-run model call in scope, split into the 5.1 slices (system prompt, tool definitions, instructions, the rest), with `source` = captured or transcript.
  - `tool_count` and `tool_groups` (built-ins plus one per MCP server, using the 5.1 `tool_group`), each group with its tool count.
  - `instruction_parts`: the instruction items of that first call (CLAUDE.md files, memory, other instructions) by label with their token size.
- **Work**
  - `prompts`, `model_calls`, `tool_calls`, `subagents`.
  - `tools_by_name`: each tool name with its call count.
  - `wall_ms`: from the first event's start to the last event's end in scope.
  - `model_ms` and `tool_ms`: summed durations of model calls and tool calls.
- **Tokens**
  - `input`, `output`, `cache_read`, `cache_write` totals.
  - `largest_context`: the largest reported context of one call.
  - `context_per_call`: the main run's model calls in order, each with its reported context total.
- **Problems**
  - `flags_by_kind`: 5.2 flag counts per kind, counting only flags whose first node is in scope.
  - `failed_tool_calls`, `compactions`.
- **Outcome**
  - `final_message`: the text of the last main-run assistant message in scope.

Unknown values stay unknown (`None`), never 0. A total is unknown only if no call contributed a value; if some calls are unknown the total counts the known ones and carries `partial: true`.

### `compare(a, b) -> Comparison`

Holds both summaries, plus for every number a diff: the difference (`b - a`) and which side is higher (`a`, `b`, `equal`, or `unknown` if either side is unknown). For named lists (models, tools by name, tool groups, instruction parts) each name is marked `both`, `only_a` or `only_b`.

## App

`compare_runs(a, b) -> Comparison`, where each side is `{ project, session_id, prompt_trace_id? }`.

- Loads each session (open sessions from memory, others from disk, as `session_flags` does), builds its summary with the adapter's rules and the capture store, and compares.
- No new permissions; it reads only what existing commands read.
- An error names the side that failed ("Run A: ..." or "Run B: ...").
- Logs its duration.
- The same session on both sides is allowed (useful with two different prompts).

## UI

- Session overview: a "Compare with..." button.
- Clicking it turns the session list into a picker headed "Pick a session to compare with", with Cancel. The current session is greyed out.
- Picking one shows the Compare view in the main area:
  - Header: one column per run with its title and time, a scope drop-down ("Whole session" or "Prompt N: <first words>"), and a "Swap" button.
  - Sections Setup, Work, Tokens, Problems, Outcome as a table: one row per value with A, B and the difference. The higher side is lightly highlighted in a neutral colour (more is not always worse). Names on one side only get an "only in A" or "only in B" tag. Unknown shows "?", partial totals show a note.
  - Tokens: a small line chart of context per call, both runs in different colours, drawn as plain SVG.
  - Outcome: the two final messages side by side, collapsed by default, scrollable.
- "Back to diagram" returns to session A as it was.
- Live: if either side is live, the comparison is refetched on the same throttle as the session context.
- Formatting rows, the scope options and the chart points are plain functions with unit tests.
- Mock: under `?mock`, the basic sample session compared with the compaction sample session.

## Error handling

- A side that fails to load: the view shows that side's error with Retry; the other side still shows.
- A scoped prompt that no longer exists: that side falls back to "Whole session" with a note.
- No model calls: rows show "?", no crash.
- Parse problems are skipped and logged as today.

## Testing

- `insights`: invented traces covering counts with subagents, per-tool counts, unknown and partial usage, prompt scope, flags limited to scope, the final message, setup from a captured and a transcript measure, `only_a` / `only_b` names, and the higher side including unknown.
- Fixtures: the sample and compaction fixtures compared; a snapshot test pins the `Comparison` JSON, which also becomes `ui/src/mock/compare-data.json` (refreshed with `SNITCHCRAFT_UPDATE_SNAPSHOTS`).
- App: `compare_runs` on the two fixtures, on one session with two prompts, and with a missing session on one side.
- UI: unit tests for the scope options, row formatting ("?", differences, tags, partial note) and chart points; lint, typecheck, test, build.
- Real-app check: two real sessions compared, numbers checked by hand against their diagrams, prompt scope, swap and back.

## Docs

- `docs/VIEW-MODEL.md`: the `Comparison` shape.
- `docs/DECISIONS.md`: comparison in `insights`, plain SVG chart, neutral highlight.
- `docs/HARNESS-NOTES.md`: anything the real comparison teaches.
- README milestone status and CLAUDE.md current milestone when 5.3 lands.
- No `docs/SCHEMA.md` change.
