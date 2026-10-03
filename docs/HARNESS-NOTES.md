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
