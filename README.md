# looptrace

A local desktop app that watches Claude Code sessions and draws a diagram of what the agent did for each prompt: model calls, tool calls, results, context growth, and stop reasons.

It is a learning and debugging tool for understanding how an agent harness behaves. The trace format is harness-agnostic, so other harnesses can emit it too.

> Screenshot coming with M2, when the app has a UI.

## Status

| Milestone | Description | Status |
|---|---|---|
| M1 | Replay a finished session from a file (schema, adapter, CLI tree printer) | Done |
| M2 | Tauri app shell that renders one diagram per prompt | Planned |
| M3 | Live file watching | Planned |
| M4 | Proxy capture of raw API requests | Planned |
| M5 | Toy Rust harness that emits the trace format natively | Planned |

## Try it

Print any Claude Code session as a tree. Sessions live in `%USERPROFILE%\.claude\projects\<project>\<session id>.jsonl`.

```powershell
cargo run -p adapter-claude-code --example print_tree -- "$env:USERPROFILE\.claude\projects\<project>\<session id>.jsonl"
```

Output for the sanitised fixture (first lines):

```text
Run: Example text 1690.  [9m27s]
  [hook] SessionStart hook: success
  [hook] SessionStart hook: additional context
  [local_command] Local command: Example text 25.  [0ms]
  Prompt: List the PowerShell scripts in this folder.  [24.3s]
    Model call (claude-opus-5-5) stop=tool_use context=70719 out=462  [4.9s]
      PowerShell(echo example-696) ok  [1.9s]
    Model call (claude-opus-5-5) stop=tool_use context=71302 out=468  [4.9s]
      PowerShell(echo example-1180) ok  [9.0s]
    Model call (claude-opus-5-5) stop=end_turn context=71808 out=219  [3.3s]
    [hook] Stop hooks ran (1)
  Prompt: Find every TODO comment in the source files.  [18.7s]
    Model call (claude-opus-5-5) stop=tool_use context=72081 out=337  [3.9s]
      Grep(Example text 1234.) ok  [98ms]
    Model call (claude-opus-5-5) stop=tool_use context=73075 out=386 2 tools in parallel  [4.1s]
      Grep(Example text 1253.) ok  [84ms]
      Grep(Example text 1263.) ok  [72ms]
    Model call (claude-opus-5-5) stop=tool_use context=73781 out=181  [2.2s]
      Read(C:\work\example\file-1285.txt) ok  [29ms]
    Model call (claude-opus-5-5) stop=end_turn context=74507 out=721  [7.9s]
    [hook] Stop hooks ran (1)
  Prompt: Which file defines the greeting function?  [9.9s]
...
```

Add `--stats` to print only node counts and skipped lines, which is handy for checking that the adapter understands a session without printing its content.

## Layout

- `crates/trace-core`: the trace schema (Run > Turn > ModelCall > ToolCall). No I/O, nothing harness-specific.
- `crates/adapter-claude-code`: converts Claude Code JSONL transcripts into `trace-core` events.
- `fixtures/`: sanitised example transcripts used by tests.
- `docs/`: schema description, design decisions, and notes on harness behaviour.

## Build and test on Windows

Requirements: Rust stable with the MSVC toolchain (Visual Studio Build Tools with the C++ workload).

In PowerShell:

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

## Privacy

The app is read-only and never modifies anything under `.claude`. Real transcripts can contain secrets and are never committed. Only sanitised fixtures live in this repo.

## Licence

MIT. See `LICENSE`.
