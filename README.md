# looptrace

A local desktop app that watches Claude Code sessions and draws a diagram of what the agent did for each prompt: model calls, tool calls, results, context growth, and stop reasons.

It is a learning and debugging tool for understanding how an agent harness behaves. The trace format is harness-agnostic, so other harnesses can emit it too.

> Screenshot coming with M2, when the app has a UI.

## Status

| Milestone | Description | Status |
|---|---|---|
| M1 | Replay a finished session from a file (schema, adapter, CLI tree printer) | In progress |
| M2 | Tauri app shell that renders one diagram per prompt | Planned |
| M3 | Live file watching | Planned |
| M4 | Proxy capture of raw API requests | Planned |
| M5 | Toy Rust harness that emits the trace format natively | Planned |

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
