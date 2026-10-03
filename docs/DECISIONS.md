# Decisions

Significant decisions and the reasons for them. Every added dependency is recorded here.

## Dependencies

| Crate | Used by | Why |
|---|---|---|
| `serde` | trace-core, looptrace (src-tauri) | Derive JSON (de)serialisation for the schema types. Named in CLAUDE.md. |
| `serde_json` | trace-core, adapter-claude-code, looptrace | JSON values for tool inputs and metadata, the JSONL wire format, and defensive parsing of transcript lines. Named in CLAUDE.md. |
| `thiserror` | trace-core, adapter-claude-code, looptrace | Error types for invalid trace events and unreadable files. Named in CLAUDE.md for library crates. |
| `tracing` | adapter-claude-code, looptrace | Logs skipped transcript lines and app activity. Named in CLAUDE.md. |
| `tauri` 2 | looptrace (src-tauri) | The desktop app framework. Named in CLAUDE.md. Pinned to major version 2 because 3.0 alphas exist on crates.io. |
| `tauri-build` 2 | looptrace (build script) | Required by Tauri to embed the config and generate command permissions. |
| `tracing-subscriber` | looptrace | Prints `tracing` logs to the terminal. Only the default `fmt` output, no extra features. |
| `anyhow` | looptrace | Error type for `main` in the app binary. CLAUDE.md allows it only there. |

UI packages (`ui/package.json`):

| Package | Why |
|---|---|
| `react`, `react-dom` | UI framework. Named in CLAUDE.md. |
| `@xyflow/react` | React Flow, for the diagram. Named in CLAUDE.md. |
| `@tauri-apps/api` | `invoke()` to call the backend commands. Kept on the same minor version as the `tauri` crate (2.11), since the Tauri CLI warns about mismatches. |
| `vite`, `@vitejs/plugin-react`, `typescript`, `@types/react`, `@types/react-dom`, `@types/node` | Build and type checking. Vite and TypeScript are named in CLAUDE.md. These came with the Vite React template. |
| `eslint`, `@eslint/js`, `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, `globals` | `npm run lint`. The current Vite template ships oxlint instead; ESLint was swapped in because it is the requested and more widely known linter. |

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

### 2026-10-04: Adapter node ids come from transcript ids

Runs are `run:<session id>` or `run:agent-<agent id>`, turns and markers use the line's `uuid`, model calls use `model:<message.id>`, and tool calls use `tool:<tool_use id>`. These are stable across re-reads of the same file, which live watching needs.

### 2026-10-04: The adapter is incremental

`Parser::push_line` takes one line and returns the events it produced, re-sending nodes as they fill in. M1 feeds it a whole file, but M3 can feed it lines as they are written without a redesign.

### 2026-10-04: Which lines become nodes

Attachments are context injected into the prompt, not steps, so they are dropped, except hook results, which become `hook` markers. Known bookkeeping line types are ignored on purpose. Anything else is reported as skipped with a reason and logged, never a panic. Markers that happen before the first prompt (session-start hooks, local commands) are children of the Run.

### 2026-10-04: Hand-written timestamp parser

Transcript timestamps are fixed-format RFC 3339. A 60-line parser with tests avoids a date-time dependency for one function.

### 2026-10-04: Fixtures are sanitised from real sessions (chosen by the project owner)

A local script (kept out of the repo) copies an excerpt of a real session and replaces every string except an allowlist of format values (line types, roles, stop reasons, tool names, model ids), remaps every id consistently, and shifts timestamps to 2026-01-01 while keeping gaps. It then fails if any original string of 5 or more characters, the username, or the home path appears in the output. Each flagged generic word (JSON schema keywords, tool names) was reviewed by hand.

### 2026-10-04: App crate is a plain binary called `looptrace`

`src-tauri` builds one binary with a `sessions` module for the file logic. Tauri's template also builds a library for mobile targets, which this app does not need.

### 2026-10-04: Clippy and tests do not need the built UI

Debug builds of a Tauri 2 app point the window at the dev server (`build.devUrl`) and do not embed `ui/dist`, so `cargo clippy` and `cargo test` work from a clean checkout without running npm first. This was checked by running clippy with `ui/dist` removed. Only `cargo tauri build` embeds the UI, and its `beforeBuildCommand` builds it first. CI therefore runs the Rust job and a separate UI job (`npm ci`, lint, typecheck, build) side by side.

### 2026-10-04: Only two commands are reachable from the window

`src-tauri/build.rs` lists the app commands with `AppManifest::commands`, so Tauri generates `allow-list-sessions` and `allow-load-session` permissions and denies any command not granted. `capabilities/default.json` grants exactly those two to the `main` window and nothing else: no `core:default`, no plugins. Without the list, every registered command would be open to every window. If the UI later needs a core API (for example window or event functions), the matching `core:` permission must be added to the capability.

### 2026-10-04: Strict content security policy

The production CSP allows scripts, styles, fonts and images only from the app itself (`'self'`, plus `data:` images), IPC through `ipc:` and `http://ipc.localhost`, and nothing else: no remote origins, objects, frames or form targets. `devCsp` additionally allows inline styles and the Vite dev server's websocket on `localhost:5173`, which hot reload needs. `freezePrototype` is on and the asset protocol is off. React sets inline styles through the DOM, which CSP does not block. If a library needs `<style>` tags injected at runtime, `style-src` has to be loosened, and that should be recorded here.

### 2026-10-04: Session file access is validated in Rust

The UI can only name a session by project folder name and session id, both returned by `list_sessions`. `load_session` accepts each only if it is a single plain path component (not empty, not `.` or `..`, no `/`, `\`, `:` or NUL), then canonicalises the file and checks it is still inside the projects folder, which also stops links that point elsewhere. The projects folder comes from Tauri's home directory lookup at startup. All file logic takes the folder as a parameter so tests run against `fixtures/` instead of the real home folder.

### 2026-10-04: Command arguments use snake_case

The commands are declared with `rename_all = "snake_case"`, so the UI calls `invoke('load_session', { project, session_id })`, matching the snake_case field names in every response. Tauri's default would be camelCase arguments next to snake_case results.

### 2026-10-04: Commands run file work on a blocking thread

Both commands are `async` and do their file reading in `spawn_blocking`, so a large session never freezes the window. Errors are returned as plain messages, never as panics.

### 2026-10-04: Session list reads titles by scanning

`list_sessions` reads each transcript line by line and parses only lines that contain `"ai-title"`, keeping the last title. This is simple and was fast enough for the sessions on the development machine, but it reads whole files. If listing gets slow with many large sessions, it can read only the tail of each file or cache by modified time.

