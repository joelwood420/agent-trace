# M3: live file watching

Status: design approved in chat on 2026-10-05, awaiting review of this written spec.

## Goal

While a Claude Code session is running, the diagram for that session updates by itself as Claude Code writes the transcript. New prompts, model calls, tool calls, tool results and subagents appear without the user reloading. The sidebar also shows new and active sessions without a manual refresh.

The app stays read-only and never blocks Claude Code from writing.

## Scope

In scope:

- The open session updates live, including subagent transcripts that appear partway through.
- The session list updates by itself and marks sessions that are still being written as live.
- A simulated live session in the dev mock mode, so the live UI can be checked in a browser.

Out of scope:

- Follow mode (automatically jumping to the newest prompt or session). Possible later.
- API proxy capture, token cost breakdowns, the toy harness.

## Decisions made while designing

These were chosen by the project owner and will be recorded in `docs/DECISIONS.md`.

1. **Live covers the open session and the session list** (not follow mode).
2. **The backend sends a full, freshly built diagram on each update**, not a list of changes. Parsing stays incremental, so files are never re-read; only the diagram model is rebuilt and resent. This keeps all diagram logic in Rust and the UI a plain renderer. Diagrams are a few kilobytes, so resending is cheap. Revisit and switch to sending changes if diagrams grow large enough for updates to feel slow, or when the M5 harness streams events directly.
3. **A session counts as live if its transcript was written in the last 10 minutes.** Claude Code writes no "session ended" line, and any session can be resumed, so file write time is the only signal. 10 minutes covers normal pauses while the user reads or types. The open diagram keeps watching its files for as long as it is selected, whether or not it counts as live; the flag only drives the markers.

## Backend design

### Watcher

- One recursive `notify` watcher on the `.claude\projects\` folder, started at app launch. It only reads change notices; it never writes.
- It covers new session files, growing session files, and subagent files appearing under `<session id>\subagents\`.
- File events are batched over about 250 ms, so a burst of writes becomes one update.
- If the watcher cannot start, the app works as in M2 (load on click), logs the error, and the UI shows "live updates unavailable".

### Polling safety net

On Windows, change notices for a file another process keeps open can arrive late. While a session is open, the backend also checks the size of its main and subagent files about once a second. A size change is handled exactly like a watcher event. Polling only touches the open session's files.

### Tail reading in the adapter

A new `TranscriptTail` type in `adapter-claude-code` reads a transcript incrementally:

- It remembers the byte offset read so far and any held-back partial line.
- Each read opens the file the same way as today (shared read, write and delete access, so Claude Code is never blocked), seeks to the offset, and reads only the new bytes.
- It returns the complete new lines with their line numbers. An unfinished last line is held back as "not ready yet", never a parse error. Bytes that end partway through a UTF-8 character are held back too.
- If the file is now shorter than the offset, it reports that the file was rewritten, so the caller can start over.

`load_session` is rebuilt on top of `TranscriptTail`, so loading a finished session and following a live one share one code path.

### Live session holder

A new module in `src-tauri` replaces the current single-trace `SessionCache`. For the open session it keeps:

- a `Parser` and a `TranscriptTail` for the main transcript and for each subagent,
- the `Trace` built so far and the skipped-line list.

On a change to any of its files it:

1. reads the new lines from the changed file,
2. feeds them to that file's parser and applies the resulting events to the trace,
3. starts a parser for any newly linked subagent (if the subagent's file does not exist yet, it is picked up when it appears),
4. rebuilds the diagram with `trace_view::build_session`,
5. emits a `session-updated` event to the UI.

The run's end time is refreshed on each update by sending the `Run` again (as `Parser::finish` does today), so durations stay correct while the session grows.

If any file of the session was rewritten (shorter than before), the holder discards everything and loads the session from scratch. It does not try to guess what changed.

`load_session` starts watching the session it loads and stops watching the previous one. No separate watch command. `node_detail` reads from the live trace.

### Session list

- On any file event under the projects folder, the backend emits `sessions-changed`, at most about once a second. The UI then calls `list_sessions` again.
- `SessionSummary` gains `live: bool` (file written in the last 10 minutes), computed in Rust.

### Events sent to the UI

| Event | Payload | When |
|---|---|---|
| `session-updated` | project, session id, a `SessionView` (diagram, skipped lines, `live`), and the trace ids whose content changed | The open session's files changed |
| `session-status` | project, session id, a status: `watching`, `unavailable`, or `deleted` | Watching starts, the watcher fails, or the open file disappears |
| `sessions-changed` | none | Anything changed under the projects folder |

`SessionView` gains `live: bool` so the header badge does not need its own logic in the UI.

The changed trace ids let the details panel refetch only when the node it shows has changed.

### Permissions

- No new file access. Watching and reading stay within the projects folder, and paths are validated in Rust as today.
- The window gains only the core event permissions needed to listen for and stop listening to events. Before implementing, check the current Tauri docs; if channels would avoid granting event permissions with no extra complexity, use channels instead and record why.

## UI design

- On `session-updated` for the open session, the UI swaps in the new diagram. Expanded and collapsed boxes and the selected node carry over by diagram id. If the selected node no longer exists, the details panel clears.
- If the node shown in the details panel is in the changed id list (for example, a tool call that just got its result), the panel refetches its detail.
- The prompt list grows as prompts arrive. The UI does not switch prompts by itself. New prompts get a short highlight.
- The header shows a "Live" badge when the open session is live. The sidebar shows a live dot next to live sessions.
- Every event carries the project and session id. The UI ignores updates for any session other than the one shown, which avoids races when switching quickly.
- `session-status` messages show in the header: "live updates unavailable" or "file no longer exists".
- Mock mode (`?mock`) gains a simulated live session that replays the sanitised fixture a few lines at a time through the same events, so the live UI can be checked in a browser and by UI tests.

## Error handling

- Watcher fails to start: M2 behaviour, logged, header message.
- Read error during an update: logged, last good diagram kept, retried on the next change.
- Open file deleted: watching stops, last diagram kept, header says the file no longer exists.
- Bad lines go into the skipped-lines list as today. Unknown fields and event types are skipped and logged. Nothing panics.

## Testing

Adapter:

- Feed the sanitised fixture to `TranscriptTail` in random-sized byte chunks, including cuts in the middle of a line and in the middle of a UTF-8 character. The final trace must equal a one-shot `load_session` of the same file.
- The same with a subagent transcript that appears after its parent has linked it.
- A file that gets shorter is reported as rewritten.

App:

- Live holder tests in a temp folder: append fixture lines step by step and check the diagram grows; shrink a file and check the session restarts; delete it and check the deleted status.
- One real `notify` test that writes to a temp file and waits for the update.

UI:

- Expanded state and selection survive a diagram swap.
- Updates for other sessions are ignored.
- The details panel refetches only when its node changed.

Manual:

- Run `cargo tauri dev`, start a real Claude Code session in another terminal, and watch the diagram, sidebar and live markers update. Report what was actually seen.

All existing checks must pass: `cargo test --workspace`, clippy, fmt, and the UI lint, typecheck, test and build.

## Docs to update

- `docs/DECISIONS.md`: the three decisions above, the polling safety net, the `notify` dependency, and the event or channel choice.
- `docs/HARNESS-NOTES.md`: what was learned about how Claude Code writes transcripts live (for example, whether each line arrives in one write, and when subagent files appear).
- `docs/SCHEMA.md`: no schema change is expected. If one turns out to be needed, stop and ask first.
- `README.md`: milestone status and how live updates work.
- `CLAUDE.md`: mark M3 steps and status as they land.
