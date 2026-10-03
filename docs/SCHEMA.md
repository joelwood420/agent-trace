# Trace schema

This describes the `trace-core` schema, the format that any agent harness can emit. It will be written in M1 step 1.

Planned shape: Run > Turn > ModelCall > ToolCall, where a ToolCall that spawns a subagent contains a nested Run. Every event has a stable `id` and a `parent_id`.
