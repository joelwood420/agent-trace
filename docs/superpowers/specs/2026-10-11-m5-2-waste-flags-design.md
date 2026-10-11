# M5.2: waste and problem flags

## Goal

Find the moments in a session where the agent behaved wastefully or something went wrong, and point at the exact boxes in the diagram: the same file read again and again, a failed tool retried the same way, the same step repeated in a loop, a huge tool result, and a compaction that dropped content the agent later had to fetch again.

5.1 describes what fills one call's context. 5.2 describes what the agent did over time. The two overlap on purpose (5.1 says "this file is in context 4 times", 5.2 says "this file was read 4 times with no edit in between").

## Decisions made while designing

Chosen by the project owner:

- All five flags from the M5 plan are in scope, including compaction. Before deciding, a real manual `/compact` was run and its transcript inspected (structure only), so the compaction flag is built against real data, not the documented format alone.
- Compaction flag, option A: one flag per compaction with the token numbers and what was re-attached, plus a separate flag each time a file whose content was dropped is read again afterwards.
- Display, option A: a badge on each flagged box, a "Flags" section in the details panel, and a "Flags" card in the session overview that jumps to the box.
- The flag definitions and thresholds in "The flags" below.
- Schema option A: a small additive schema change (`Marker.compaction` and a `File` context part kind), so the harness-neutral `insights` crate never reads Claude Code metadata and the owner's own harness can emit the same thing.
- Follow 5.1's architecture: flags are computed in `insights` from the trace, served by a new command, and placed on boxes by trace id in the UI.

## Scope

In:

- `trace-core`: `Marker.compaction` and `ContextPartKind::File`, documented in `docs/SCHEMA.md`.
- Adapter: fill the compaction details, map `file`, `compact_file_reference` and `invoked_skills` attachments, supply `FlagRules`.
- `insights`: a `flags` module, and `File` context parts counted in the "Files read" slice.
- App: a `session_flags` command.
- UI: badges, details panel section, overview card, jump with reveal, mock data.
- A new sanitised compaction fixture.

Out:

- Keeping the messages Claude Code preserves word for word across a compaction in the 5.1 transcript estimate (it still resets all history at the marker). Recorded in `docs/HARNESS-NOTES.md` and parked.
- A "show only flagged boxes" filter.
- Storing flags, or flags across sessions.
- Comparing runs (5.3) and export (5.4).

## What a real compaction looks like

From one real manual compaction (fields and types only, no content was read into this repo):

1. A `system` line with subtype `compact_boundary` and `compactMetadata`: `trigger` (`"manual"`), `preTokens`, `postTokens`, `cumulativeDroppedTokens`, `durationMs`, `preCompactDiscoveredTools`, `preservedSegment` (head, anchor and tail uuids) and `preservedMessages` (`uuids`, `allUuids`; 3 messages in that session).
2. A `user` line with `isCompactSummary: true` holding the summary (about 15k characters).
3. Three `user` lines for the `/compact` local command (caveat, command name, output).
4. Re-attachments as `attachment` lines: `compact_file_reference` (`filename`, `displayPath`, no content), `file` (`filename`, `displayPath`, and `content.file` with `filePath`, `content`, `numLines`, `startLine`, `totalLines`), `invoked_skills` (`skills`: list of `name`, `path`, `content`), then the usual hidden context deltas (`deferred_tools_delta`, `agent_listing_delta`, `mcp_instructions_delta`, `environment`, `model`, hook results).

The adapter already turns line 1 into a `compaction` marker and drops reminder parts there. Today it skips `file`, `compact_file_reference` and `invoked_skills`.

## Schema changes (`trace-core`)

Both are additive. Traces written before this change read unchanged.

### `Marker.compaction`

```
Marker {
  kind: String,
  summary: String,
  compaction: Option<Compaction>,   // new; only for kind "compaction"
}

Compaction {
  tokens_before: Option<u64>,       // context size just before compaction
  tokens_after: Option<u64>,        // context size just after
  files_restored: Vec<String>,      // paths re-attached with their content
  files_referenced: Vec<String>,    // paths named without their content
}
```

`compaction` is left out of the JSON when `None`, the two lists when empty, the two numbers when `None`.

### `ContextPartKind::File`

A new kind, serialised as `"file"`: a file's content that the harness put into the context without a tool call (for Claude Code, re-attached after a compaction; other harnesses may use it for files the user mentions). Its `label` is the file path and its `text` the content. Older readers that do not know `"file"` already read it as `Other("file")`.

## Adapter changes (Claude Code)

- `compact_boundary`: `compaction.tokens_before` from `preTokens`, `tokens_after` from `postTokens`. The raw `compact_metadata` stays in the marker's `metadata`.
- After a boundary, until the next prompt or model call, `file` and `compact_file_reference` attachments add their path (`content.file.filePath` or `filename`) to `files_restored` or `files_referenced` (a `file` with no content counts as referenced), and the adapter re-sends the marker with the same id. Re-sending an id replaces the node, so this works in a live session.
- Every `file` attachment becomes a `File` context part keyed `file:<path>`. Only post-compaction `file` attachments have been seen in real transcripts so far; one found anywhere else is still a `File` part, just not counted as restored. File parts are removed at the next compaction, the same way reminder parts are.
- `invoked_skills` becomes one reminder part labelled `invoked skills`, its text the skills' content joined with blank lines.
- A missing or malformed field is skipped and logged, never a panic, as for every other line.
- `flag_rules()` returns the `FlagRules` below.

## `insights` changes

### `File` parts in the breakdown

A `File` context part counts in the "Files read" slice, as an item labelled by its path, merged with reads of the same path.

### `FlagRules`

Plain data from the adapter:

```
FlagRules {
  file_reads: Vec<FileReadRule>,    // existing type: tool + path_field
  range_fields: Vec<String>,        // input fields that pick a part of a file, e.g. offset, limit
  file_edits: Vec<FileEditRule>,    // tool + path_fields (first present field wins)
}
```

Claude Code: reads `Read` / `file_path`; range fields `offset`, `limit`, `pages` (PDF page chunks); edits `Edit`, `MultiEdit` (older transcripts), `Write` (`file_path`) and `NotebookEdit` (`notebook_path`, then `file_path`).

### `Flag`

```
Flag {
  kind: FlagKind,          // repeated_read | failed_retry | loop | large_result
                           // | compaction | reread_after_compaction
  level: AdviceLevel,      // warn | info (the existing 5.1 type)
  text: String,            // plain words
  trace_ids: Vec<String>,  // every node involved, in session order
  run_id: String,          // the run the flag is about
}
```

`session_flags(trace, rules) -> Vec<Flag>`: pure, no I/O, flags in session order of their first node. Each run (the main agent and each subagent) is checked on its own. A subagent's tool calls belong to the subagent's run, never the parent's.

## The flags

Thresholds are named constants, like 5.1's advice thresholds.

| Kind | Fires when | Level | `trace_ids` | Example text |
|---|---|---|---|---|
| `repeated_read` | The same file, with the same range fields, is read `REPEAT_READ_WARN` (3) or more times in one run with no edit to that path in between. | warn | every read in the series | "`src/main.rs` read 4 times with no edit in between." |
| `failed_retry` | A tool call's result has `is_error`, and a later call in the same run has the same name and an equal input. | warn if the last retry failed too, info if it succeeded | the failed call and each identical retry | "`Bash` failed and was retried 2 times with the same input; it failed every time." |
| `loop` | The same tool call (name and equal input) happens `LOOP_WARN` (3) or more times in one turn with no file edit of any path in between. | warn | every call in the series | "The same `Bash` call ran 3 times in this prompt with no file edit in between." |
| `large_result` | One tool result measures `LARGE_RESULT_TOKENS` (10,000) tokens or more. | warn | the tool call | "`WebFetch` returned about 14k tokens." |
| `compaction` | Every marker with `compaction` details. | info | the marker | "Compaction dropped about 361k tokens (kept about 19k). Re-attached 1 file in full, 2 by name only. 5 files read earlier were not re-attached." |
| `reread_after_compaction` | The first read, after a compaction, of a file that was read or restored before it and is not in its `files_restored`. | warn | the read | "`src/main.rs` was read again after compaction dropped it." |

Rule details:

- Paths compare exactly as recorded, with no case folding or separator changes.
- Inputs compare as JSON values (`serde_json::Value` equality, so key order does not matter).
- A compaction restarts the `repeated_read` count for every file. The first read of a dropped file after it is a `reread_after_compaction`, and that read also starts the new `repeated_read` count.
- `loop` ignores read tools (covered by `repeated_read`). A series where any call failed is a `failed_retry`, not a `loop`. So one node never gets two flags for the same pattern.
- A tool call with no result yet is never a failure and has no size.
- Result size uses the 5.1 measuring helpers (text length, compact JSON length, `IMAGE_CHARS` per image) divided by `CHARS_PER_TOKEN`.
- Compaction text leaves out any sentence whose numbers are unknown. "files read earlier" counts distinct paths read or restored in the run before the marker that are in neither list. Numbers use 5.1's `about_tokens`.

## App

`session_flags(project, session_id) -> SessionFlags { flags: Vec<Flag> }`: computed from the open session's trace with the adapter's `flag_rules()`. No capture needed. Logs the flag count. Errors return a string, like the other commands.

## UI

- Badge: a small warning badge with a count on every box whose trace id is in some flag's `trace_ids`. Info-only flags use a quieter style than warn. A box that hides other boxes (a summary node, a parallel group, a collapsed subagent) shows the sum of its hidden children's flags.
- Details panel: a "Flags" section at the top, listing each flag that involves the selected node, with its text.
- Session overview: a "Flags" card, grouped by kind with a count per kind, each flag clickable. Clicking jumps to the flag's first node, opening any summary, group or subagent that hides it, and selects it.
- Live sessions: refetched on the same throttle as the session context.
- Mapping trace ids to boxes and summing hidden children are plain functions with unit tests. The UI does not decide what is a flag.
- Mock: under `?mock`, the compaction sample session shows every kind of flag and the basic sample session has none.

## Error handling

- Malformed compaction or attachment fields: skipped and logged in the adapter. A boundary with no numbers still gives a `compaction` marker, and the flag text leaves the numbers out.
- A flag whose node is not in the current diagram (for example a live update in between): the badge is not shown and the card entry still lists it, but jumping does nothing.
- `session_flags` on a session that is not open: loads it from disk, as `session_context` does.

## Testing

- `trace-core`: round-trip of `Marker.compaction` and `ContextPartKind::File`, and reading an old trace without them.
- Adapter: a new sanitised fixture with the real compaction's shape (boundary with metadata, summary, command lines, `file`, two `compact_file_reference`, `invoked_skills`, then a re-read of a dropped file). Tests check the marker's details, the re-sent marker, the `File` part and its removal at the next compaction, and the `invoked skills` part.
- `insights`: one test per flag kind plus the edge cases: an edit between reads, different ranges, a retry that succeeds, a loop broken by an edit, a loop where one call failed, a compaction that restores one file and drops another, a subagent run kept separate, a call with no result, and a `File` part in the breakdown.
- App: a test for `session_flags` on the fixture.
- UI: tests for the trace id to box mapping, the hidden-children sums, and the reveal path for a jump.
- Real-app check: the compacted session and one long session, checking badges, the card, jumps into collapsed boxes, and that flag texts match what the diagram shows.

## Docs

- `docs/SCHEMA.md`: the two schema additions.
- `docs/VIEW-MODEL.md`: the `SessionFlags` shape.
- `docs/HARNESS-NOTES.md`: what a real compaction records and re-attaches, and the preserved messages.
- `docs/DECISIONS.md`: the schema choice and the reasons.
- README milestone status and CLAUDE.md current milestone when 5.2 lands.
