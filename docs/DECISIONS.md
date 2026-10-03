# Decisions

Significant decisions and the reasons for them. Every added dependency is recorded here.

## Dependencies

| Crate | Used by | Why |
|---|---|---|
| `serde` | trace-core | Derive JSON (de)serialisation for the schema types. Named in CLAUDE.md. |
| `serde_json` | trace-core | JSON values for tool inputs and metadata, and the JSONL wire format. Named in CLAUDE.md. |
| `thiserror` | trace-core | Error type for invalid trace events. Named in CLAUDE.md for library crates. |

## Decisions

### 2026-10-04: Scaffold only the Rust crates for M1

`src-tauri/` and `ui/` are left out until M2, since the Tauri app is out of scope for M1. This keeps the build and CI small.

### 2026-10-04: Enforce "no unwrap or expect" with clippy

The workspace denies `clippy::unwrap_used` and `clippy::expect_used`, and `clippy.toml` allows them in tests. The rule from `CLAUDE.md` is checked by the linter instead of by review.

### 2026-10-04: Forbid unsafe code

The workspace sets `unsafe_code = "forbid"`. Nothing in this app needs it.

### 2026-10-04: LF line endings in the repo

`.gitattributes` normalises text files to LF, so checkouts look the same on every machine and CI formatting checks are stable.

### 2026-10-04: Re-sending a node replaces it (chosen by the project owner)

Nodes are updated by sending them again with the same `id`. The alternative was immutable nodes with a separate ToolResult node. Replacing keeps the tree at four levels, keeps a tool call's input and output on one node for the UI, and works the same way for live watching in M3. `Trace::apply` rejects a re-send that changes the parent or the node kind, so an update can never reshape the tree.

### 2026-10-04: A fifth node kind, Marker (chosen by the project owner)

Compaction, hooks, interrupts and API errors are not model or tool calls but matter for understanding a run. They become `Marker` nodes with a free-form `kind` label, so they show up in the diagram where they happened, and other harnesses can define their own kinds.

### 2026-10-04: Timestamps are Unix milliseconds

`started_at_ms` and `ended_at_ms` are integers. Any harness can produce them without a date library, durations are a subtraction, and the core crate needs no time dependency.

### 2026-10-04: Prompt-cache token counts are core Usage fields

Cache reads and writes are needed to compute context size, which is a main thing the diagram shows. Several providers report cached tokens, so these are optional core fields instead of metadata.

### 2026-10-04: Integration tests may use expect()

`clippy.toml` allows `expect()` only inside `#[test]` functions. Integration test files are test-only code, so they allow `clippy::expect_used` at the top of the file for their fixture-loading helpers.
