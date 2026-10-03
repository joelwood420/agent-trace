# Harness notes

Observations about how Claude Code behaves (loop structure, compaction, subagents, parallel tool calls), learned while building the adapter. Based on Claude Code transcripts from October 2026; the format is not a stable API and may change.

## Transcript files

- Each session is one JSONL file: `<project folder>/<session id>.jsonl`.
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

## Hooks

- Hook activity shows up three ways: `attachment` lines with types like `hook_success`, `hook_additional_context` and `hook_non_blocking_error` (with `hookEvent`, for example `SessionStart` or `PreToolUse`, and for tool hooks a `toolUseID`), and a `system` line with subtype `stop_hook_summary` at the end of each turn.

## Local actions

- A slash command typed by the user (for example `/model`) is written as `user` lines whose text starts with `<command-name>` and `<local-command-stdout>`, plus a meta `<local-command-caveat>` line. Newer sessions also write it as a `system` line with subtype `local_command`.
- A shell command typed with `!` is written as `<bash-input>` followed by `<bash-stdout>` and `<bash-stderr>`.
- None of these reach the model as a turn.

## Other line types seen

- `system` subtypes: `informational` (a warning shown to the user, with `level`) and `away_summary` (a recap shown when the user returns).
- Session-level lines: `ai-title` (generated title), `agent-name`, `agent-setting`, `continued-in` (with `continuedInSessionId`, when a session continues in a new file).
