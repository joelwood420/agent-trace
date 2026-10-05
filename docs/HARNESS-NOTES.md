# Harness notes

Observations about how Claude Code behaves (loop structure, compaction, subagents, parallel tool calls), learned while building the adapter. Based on Claude Code transcripts from October 2026; the format is not a stable API and may change.

## Transcript files

- Each session is one JSONL file: `<project folder>/<session id>.jsonl`.
- The project folder name is the project's path with separators and some other characters replaced by `-` (a drive path like `C:\work\demo` becomes `C--work-demo`). The original path cannot be recovered exactly, because a `-` in a folder name looks the same.
- A session can have several `ai-title` lines as Claude Code revises the title. The last one is the current title.
- Each subagent gets its own file: `<project folder>/<session id>/subagents/agent-<id>.jsonl`. Lines in these files have `isSidechain: true` and an `agentId`.
- Besides `user` and `assistant` lines, transcripts contain many bookkeeping line types: `attachment`, `system` (with subtypes such as `turn_duration` and `stop_hook_summary`), `mode`, `permission-mode`, `ai-title`, `last-prompt`, `file-history-snapshot`, `cost-state`, `queue-operation` and others.
- Message lines are linked by `uuid` and `parentUuid`, which form a chain through the conversation.

## One API response becomes several lines

- Each `assistant` line holds exactly one content block: one `thinking`, one `text`, or one `tool_use`.
- One API response is spread over 1 to 7 consecutive lines. They share the same `message.id` and the same `requestId`.
- `stop_reason` is copied onto every line of a response, not only the last one, and `usage` is almost always identical on every line. The adapter must group lines by `message.id` instead of treating each line as a model call.
- A response that requests several tools has several `tool_use` lines. These are the tool calls the model asked for together.

## Tool results

- A tool result comes back as a `user` line whose content is a `tool_result` block with the matching `tool_use_id`, and optionally `is_error`.
- The same line also has a `toolUseResult` field with richer, tool-specific data (for example `stdout`/`stderr` for shell commands, or patch details for file edits). Its shape differs per tool.

## Tools run while the model is still answering

- In a response that asks for several tools, the lines can interleave: the first `tool_use` line, then that tool's result, then the next `tool_use` line of the same response. Claude Code starts running a tool as soon as its block has streamed in, before the rest of the response arrives.
- So tool calls from one response are concurrent, and a response's lines are not always next to each other in the file.

## Timing

- There is no "request sent" timestamp. The best estimate is the time of the line that triggered the request: the user prompt or the previous tool result.
- `attachment` lines (reminders, tool lists, environment info) are written when a response arrives, just before its first `assistant` line, so their timestamps are not request times. Using them made some model calls look like they took a few milliseconds.
- `system` lines with subtype `turn_duration` close each main-session turn and carry `durationMs`. Subagent transcripts have no such line.
- `usage` on an `assistant` line can be a mid-stream snapshot: one subagent line had `stop_reason: null` and `output_tokens: 16`. The most complete values seen for the response should be used.

## Subagents from forked skills

- A skill can run as a forked subagent. The `Skill` tool returns at once, with `toolUseResult` containing `status: "forked"`, `background: true` and an `agentId`.
- The subagent's transcript is `subagents/agent-<agentId>.jsonl`, with sidecar files `agent-<agentId>.meta.json` (has `agentType` and a `description`), `.forked-skill.json` and `.forked-skill.marker.json`.
- The subagent's first line is a `user` line marked `isMeta: true`: its task. Every line in the file has `isSidechain: true`.
- When the subagent finishes, the result reaches the main session as a new user line starting with `<task-notification>`, with `origin.kind = "task-notification"` and `promptSource = "system"`. This starts a new turn that the user did not type.

## Linking subagents early

- `agent-<id>.meta.json` for a subagent started with the Agent tool has `toolUseId`: the id of the `tool_use` block that started it. It also has `agentType`, `description` and `spawnDepth`, and for background agents `requestShape: "background"`.
- A background Agent call returns at once with `toolUseResult.status = "async_launched"` and the `agentId`. A foreground call only reports its `agentId` in the tool result once the subagent has finished.
- So while a session is running, the meta file is the only way to know which tool call a running subagent belongs to. Snitchcraft links a subagent from either source, whichever comes first. The meta files of forked skills seen so far have no `toolUseId`.

## Writing while running (seen during M3)

- The transcript file is created when the session starts, before the first model response. In a short `claude -p` run with Haiku, about 9 seconds passed between the file appearing and the first `assistant` line.
- Lines then arrive one response block at a time, so a live view grows in small steps (one model call or one tool result every one to two seconds in that run).
- A foreground subagent's transcript gets its first line about 30 ms after the parent's `tool_use` line for the Agent tool. Its `.meta.json` (with `toolUseId`, and `requestShape: "foreground"`) is written about 200 ms after that, and is not changed afterwards.
- In `claude -p` (print mode) the main transcript has no `system` lines at all, so there is no `turn_duration` and the prompt has no end time.
- With the watcher plus one-second polling, updates reached the open diagram within about two seconds of being written. Whether each update came from a file notice or from polling was not measured, so it is still open whether Windows notices for a file Claude Code keeps open arrive late.

## Hooks

- Hook activity shows up three ways: `attachment` lines with types like `hook_success`, `hook_additional_context` and `hook_non_blocking_error` (with `hookEvent`, for example `SessionStart` or `PreToolUse`, and for tool hooks a `toolUseID`), and a `system` line with subtype `stop_hook_summary` at the end of each turn.

## Local actions

- A slash command typed by the user (for example `/model`) is written as `user` lines whose text starts with `<command-name>` and `<local-command-stdout>`, plus a meta `<local-command-caveat>` line. Newer sessions also write it as a `system` line with subtype `local_command`.
- A shell command typed with `!` is written as `<bash-input>` followed by `<bash-stdout>` and `<bash-stderr>`.
- None of these reach the model as a turn.

## Other line types seen

- `system` subtypes: `informational` (a warning shown to the user, with `level`) and `away_summary` (a recap shown when the user returns).
- Session-level lines: `ai-title` (generated title), `agent-name`, `agent-setting`, `continued-in` (with `continuedInSessionId`, when a session continues in a new file).

## Raw API requests (seen during M4)

Seen by putting a pass-through proxy between Claude Code and the API (checked on 2026-10-05, subscription login). Structure and sizes only.

- Claude Code honours `ANTHROPIC_BASE_URL` with a subscription login and works normally through a local plain-HTTP proxy. The login token passes through untouched, and streaming (server-sent events) responses pass through fine.
- Before the first request it sends `HEAD /api/hello` to the same address.
- Model calls are `POST /v1/messages?beta=true`.
- One request for a one-word reply was about 300 KB: a 3-block system prompt (about 28,000 characters, two blocks marked for caching), 132 tool definitions, one user message of 11 text blocks, `max_tokens` 32000, thinking enabled with its text hidden (`display: "omitted"`), and a `context_management` option.
- Every request carries an `x-claude-code-session-id` header equal to the transcript file name. The response's message id (`msg_...`) appears in the transcript, so a capture joins to its model call exactly (`model:<message id>`).
- Because thinking is requested with its text omitted, hidden thinking is never sent back and cannot be captured from outside.
