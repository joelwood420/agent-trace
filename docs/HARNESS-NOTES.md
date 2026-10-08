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
- In a real captured `claude -p` session (Haiku, one subagent, 6 model calls) the tool set grew during the session: the first call offered 123 tools, later calls 169 and then 186, as connected tool servers finished starting. So the tool list is not fixed for a session, and early calls can see fewer tools than later ones.
- The subagent's requests used a different, much shorter system prompt (about 2,800 characters against about 27,700 for the main agent), and its second call kept its first message and only added the newest assistant turn and tool result.
- In the diffs checked, a call kept every earlier message of its agent and only appended new ones, so the diff between calls is small even though each request re-sends the whole history.
- `HEAD /api/hello` carries no `x-claude-code-session-id` header. Snitchcraft forwards it but does not record it.
- No `Origin` header is sent, and the `Host` header matches the `ANTHROPIC_BASE_URL` host (the proxy refuses anything else, and every request was accepted).

## What fills the context (seen during M5.1)

Checked on 2026-10-08 with a short captured `claude -p` session (Haiku, one Read) and the app's context breakdown. Structure and sizes only.

- On a call of about 82k input tokens, tool definitions were 78% (about 64k tokens, 123 tools, 89 of them from MCP servers; the largest servers added about 16k and 14k tokens each). The system prompt was about 10%, injected instructions and reminders about 8%, and the actual conversation under 5%. For a short task, almost everything the model reads is setup, not the task.
- Injected reminders are `<system-reminder>` blocks inside user messages. One request held 12 reminder sections: SessionStart hook output, the environment block, the model name, the agent type list, MCP server instructions, the skills list, the instructions files (each one starts with `Contents of <path> (<description>):`), user context, the date, and git attribution notes. Reminders also appear inside tool results.
- The instruction files are not only CLAUDE.md: an `AGENTS.md` in a parent folder was injected the same way.
- Recent transcripts record much of this hidden context too, as `attachment` lines: `prompt_snapshot` (the system prompt blocks, 14 to 15 of them), `instructions` (each instructions file with its path and content), `skill_listing` and `mcp_instructions_delta`. Tool definitions are recorded too: the first `prompt_snapshot` of a session has only the system prompt, and the second one (written after the first call) also carries `tools`, an array of `{name, description, schema}` objects. In one checked session it held 123 tools, 89 of them named `mcp__<server>__<tool>`, the same set the real request sent. `deferred_tools_record` and `deferred_tools_delta` list only tool names.
- Since 5.1b the adapter turns these lines into `context_update` nodes: `prompt_snapshot` is the system prompt, plus one tool definitions part per tool when `tools` is present (key `tool:<name>`; a new tool list is the full set, so tools missing from it are removed), each `instructions` file is an instructions part, and `skill_listing`, `mcp_instructions_delta`, `agent_listing_delta`, `deferred_tools_delta`, `hook_additional_context`, `environment`, `date`, `model` and `session_context` are reminder parts. Sizes match the real request closely (within about 2% for the skills list, MCP instructions, agent list and hook output in a captured session). Lists are joined with one newline per line and blocks with a blank line, which is how they read in the request.
- Reminders pile up: in the real request each injected reminder stays in the history, so a later `date` or `environment` line adds text rather than replacing the earlier one. The adapter gives each reminder its own key, and at a compaction it removes the reminder parts it emitted before.
- How close a transcript-only split gets: on one call checked against its capture, each slice was within about 10 to 35% of the captured split (system prompt 5.6k against 7.8k tokens, tool definitions 69k against 64k, instructions 4.2k against 6.2k, files read 2.5k against 3.5k), with the same total. The gaps come from scaling: the estimate of 4 characters per token overshot the reported total, so every slice was scaled down by the same ratio, while the real token cost per character differs by kind of text.
- First-call tools gap: Claude Code writes the tools snapshot just after the first model call, so a session's first call shows its tool definitions as not captured in a transcript-only split; later calls count them.
- Still not recorded in transcripts: `total_tokens_reminder` text, and whatever makes up the large remainder on long sessions (open question above).
- Open question: on a long transcript-only session (about 990 model calls, 1M-token model), the transcript accounted for only about a third of the latest call's reported 630k input tokens. The rest is far more than a system prompt plus tools. Possible causes, not yet checked: hidden thinking sent back to the model, content the transcript shortens, or reminders the transcript does not keep. A captured long session would answer it.
- Compaction shows in the transcript as a `system` line with subtype `compact_boundary` (its `compactMetadata` holds the trigger, `auto` or `manual`, and the token count before compaction), followed by a `user` line with `isCompactSummary: true` that holds the summary and becomes the new start of the history. The adapter turns the boundary into a `compaction` marker (the summary line becomes an ordinary prompt), and the transcript-only context estimate resets there, so calls after a compaction count only the summary and what follows it.
