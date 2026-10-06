# Snitchcraft

A local desktop app that watches Claude Code sessions in real time and renders a diagram of what the agent did for each prompt: model calls, tool calls, results, context growth, and stop reasons.

## Why this exists

This is a learning tool and a future debugging tool. The long-term goal is to build my own agent harness. This app exists to understand how an existing harness behaves, and later to troubleshoot mine. Decisions should favour fidelity and clarity of the captured data over visual polish.

## Environment

- I develop and run Claude Code on native Windows (not WSL), using PowerShell. Every command you run or document must work in PowerShell. No bash-only syntax.
- Transcripts live under `%USERPROFILE%\.claude\projects\`. Resolve the home directory at runtime and build paths with `PathBuf`. Never hardcode a path, a username, or a path separator.
- Tauri on Windows needs the MSVC build tools and WebView2. Check these are present before scaffolding and tell me if anything needs installing.
- Keep the code cross-platform where it costs nothing, since the repo is public, but Windows is the only platform that must work and be tested.

## Architecture

Tauri 2 desktop app in a Cargo workspace:

- `crates/trace-core`: the harness-agnostic trace schema (Run > Turn > ModelCall > ToolCall, where a ToolCall that spawns a subagent contains a nested Run) and nothing else. No I/O, no Claude Code specifics, no Tauri.
- `crates/adapter-claude-code`: reads Claude Code JSONL transcripts from the `.claude\projects\` folder in my home directory and converts them into `trace-core` events.
- `src-tauri/`: the app backend. Watches transcript files, runs the adapter, and pushes trace events to the UI through Tauri events or channels.
- `ui/`: React + TypeScript + React Flow frontend (Vite) that renders one diagram per prompt inside the Tauri webview.

Data flow: JSONL file -> file watcher -> adapter -> trace events -> Tauri event -> diagram.

All parsing and logic lives in Rust. The UI only renders trace events and must not reinterpret raw transcript data itself.

## Hard rules

- `trace-core` must never depend on anything Claude Code specific. My own harness will emit this schema directly later. If a field only makes sense for Claude Code, it goes in an optional `metadata` map, not the core types.
- The Claude Code transcript format is not a stable API. Parse defensively: unknown fields and unknown event types must be skipped and logged, never cause a panic.
- Keep the raw source line alongside every parsed event so the UI can always show the original data.
- This app is read-only. Never write to, move, or delete anything under the `.claude` folder.
- Keep Tauri permissions minimal: read-only file access scoped to the `.claude\projects\` folder, and no network access unless a milestone needs it. Since M4 the backend runs a capture proxy that listens on `127.0.0.1:47821` only and forwards only to `https://api.anthropic.com`; the web page has no network access. The only folder the app writes to is its own captures folder.
- Every event has a stable `id` and a `parent_id`, so the trace is always a tree that the UI can render without guessing.
- Claude Code may be writing a transcript while the app reads it. Open files in a way that does not block the writer, and treat an incomplete last line as "not ready yet", never as a parse error.
- Transcripts can contain secrets and private code. Never commit real transcripts. Test fixtures in `fixtures/` must be sanitised.

## The diagram

The default view for each prompt is a tree:

- The root is my prompt. Its children are the model calls in order, and each model call's children are the tool calls it made.
- A subagent appears as a tool call with its own nested subtree, collapsed by default.
- Tool calls made in parallel are shown as a visibly grouped set of siblings.
- Long runs of similar calls (for example, 8 file reads in a row) collapse into one summary node that can be expanded.
- Each node shows a short label, duration, and token usage where known. Clicking a node shows the full input, output, and raw source line.

Other views (flowchart, timeline) are possible later and must be built from the same trace events.

## Public repo

This repo is public and is a portfolio piece.

- Nothing personal in the repo: no real transcripts, usernames, machine paths, API keys, or project names from my other work. Check fixtures, docs, screenshots, and test output for these before every commit.
- Sanitised fixtures must replace real file paths, file contents, and prompts with invented ones while keeping the event structure intact.
- Maintain a `README.md` with what the app does, a screenshot, how to build and run it on Windows, and the milestone status. Update it when a milestone lands.
- MIT licence, a sensible `.gitignore`, and a GitHub Actions workflow on `windows-latest` that runs tests, clippy, and fmt.
- Commit locally in small steps. Never push, force-push, or change remote settings. I do the pushing.

## Tech choices

- Rust stable, Tauri 2 (check the current Tauri docs before scaffolding, do not rely on memory for config formats or APIs)
- `tokio` for async
- `serde` and `serde_json` for parsing
- `notify` for file watching
- `thiserror` for library crates, `anyhow` only in the app binary
- `tracing` for logs
- UI: React, TypeScript, Vite, React Flow

Keep dependencies few. When you add one, record it and the reason in `docs/DECISIONS.md`.

## Commands

- Install the Tauri CLI once: `cargo install tauri-cli --version "^2" --locked`
- Install UI packages once: `cd ui; npm ci`
- Run app in dev: `cargo tauri dev` (from the repo root; starts the Vite dev server itself)
- Build app: `cargo tauri build` (builds the UI first; installers land in `target\release\bundle\`)
- Rust tests: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Print a session tree: `cargo run -p adapter-claude-code --example print_tree -- <session.jsonl>` (add `--stats` for counts only)
- UI checks: `cd ui; npm run lint; npm run typecheck; npm test; npm run build`
- View the UI in a browser with the sanitised sample session (dev only): `cd ui; npm run dev`, then open `http://localhost:5173/?mock` (add `&live` to replay the session growing; the sample session includes invented captured API calls)
- Capture a Claude Code session through the app's proxy (app must be running): `$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude`
- Regenerate the invented capture fixture: `cargo run -p capture --example make_fixture`
- Refresh the UI mock data (`ui/src/mock/fixture-data.json`, `live-steps.json` and `capture-data.json`) after a view model change: set `$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'`, run `cargo test -p snitchcraft`, then `Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS` and review the diff of all three files

If a command here turns out to be wrong after scaffolding, fix this file.

## How to work with me

You are doing the implementation and I will mostly not read the code. That means you own correctness, and I judge the work by running the app and reading your summaries.

- Verify everything yourself. A task is done only when tests, clippy, fmt, and UI checks pass and you have actually run the thing you changed.
- Every adapter or schema change needs a test against a fixture file. Tests are the review, since I am not reviewing line by line.
- Never report something as working unless you ran it. If you could not verify something, say so plainly.
- After each task, give me a short plain-language summary: what changed, how you verified it, and anything I should try in the app.
- Make routine implementation choices yourself. Stop and ask me only for decisions that change the trace schema, the architecture, the scope of a milestone, or what the app can access on my machine.
- Record significant decisions and their reasons in `docs/DECISIONS.md`.
- Keep `docs/SCHEMA.md` up to date as a readable description of the trace schema. This is the one part I will read closely, because it is what my own harness will emit.
- When you learn something about how Claude Code behaves (loop structure, compaction, subagents, parallel tool calls), add it to `docs/HARNESS-NOTES.md`. These notes are a main output of the project.
- Do not use em dashes in any prose, comments, or docs.

## Code conventions

- No `unwrap()` or `expect()` outside tests. Return `Result` and handle errors.
- Prefer small, plain functions and concrete types over clever generics and macros.
- Doc comments on all public types in `trace-core`, since the schema is the contract.
- Commit in small, working steps with clear messages written for a public reader.

## Current milestone

M4: proxy capture of raw API requests (system prompt, tool definitions, compaction).

Status: complete. M5 (toy harness) is next; plan its steps with me before starting.

1. `capture-core`: the capture record format, header filtering, rebuilding a streamed response, request summaries and the diff.
2. `capture`: the on-disk store (compressed, system prompts and tool sets kept once) and the local proxy.
3. Pairing model calls by agent (`trace-view`) and mapping captures to Claude Code sessions (adapter).
4. App: proxy started with the app, captures saved and joined to the open session live, capture commands.
5. UI: API tag on boxes, changes since the previous call, full raw request, session overview, start command, Delete.
6. Checked with a real captured session, docs updated.

Out of scope for now: capturing scratchpad files, token cost breakdowns, other harnesses, and the toy harness.

## Later milestones (for context only)

- M2: Tauri app shell that loads a saved session and renders one diagram per prompt
- M3: live file watching with diagrams updating as a session runs
- M4: proxy capture of raw API requests (system prompt, tool definitions, compaction)
- M5: toy harness in Rust that emits `trace-core` events natively. Try a fast decision model (for example Jev) inside the loop for routing and stop checks, and trace each decision call as its own node so wrong routing choices are visible.
