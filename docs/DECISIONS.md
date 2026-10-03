# Decisions

Significant decisions and the reasons for them. Every added dependency is recorded here.

## Dependencies

| Crate | Used by | Why |
|---|---|---|
| `serde` | trace-core, trace-view, snitchcraft (src-tauri) | Derive JSON (de)serialisation for the schema and view model types. Named in CLAUDE.md. |
| `serde_json` | trace-core, adapter-claude-code, trace-view, snitchcraft | JSON values for tool inputs and metadata, the JSONL wire format, and defensive parsing of transcript lines. Named in CLAUDE.md. |
| `thiserror` | trace-core, adapter-claude-code, snitchcraft | Error types for invalid trace events and unreadable files. Named in CLAUDE.md for library crates. |
| `tracing` | adapter-claude-code, snitchcraft | Logs skipped transcript lines and app activity. Named in CLAUDE.md. |
| `tauri` 2 | snitchcraft (src-tauri) | The desktop app framework. Named in CLAUDE.md. Pinned to major version 2 because 3.0 alphas exist on crates.io. |
| `tauri-build` 2 | snitchcraft (build script) | Required by Tauri to embed the config and generate command permissions. |
| `tracing-subscriber` | snitchcraft | Prints `tracing` logs to the terminal. Only the default `fmt` output, no extra features. |
| `anyhow` | snitchcraft | Error type for `main` in the app binary. CLAUDE.md allows it only there. |

UI packages (`ui/package.json`):

| Package | Why |
|---|---|
| `react`, `react-dom` | UI framework. Named in CLAUDE.md. |
| `@xyflow/react` | React Flow, for the diagram. Named in CLAUDE.md. |
| `@tauri-apps/api` | `invoke()` to call the backend commands. Kept on the same minor version as the `tauri` crate (2.11), since the Tauri CLI warns about mismatches. |
| `vite`, `@vitejs/plugin-react`, `typescript`, `@types/react`, `@types/react-dom`, `@types/node` | Build and type checking. Vite and TypeScript are named in CLAUDE.md. These came with the Vite React template. |
| `eslint`, `@eslint/js`, `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, `globals` | `npm run lint`. The current Vite template ships oxlint instead; ESLint was swapped in because it is the requested and more widely known linter. |

`trace-view` uses `adapter-claude-code` as a dev-dependency only, so its tests and example can load the Claude Code fixture. The library itself depends only on `trace-core`, `serde` and `serde_json`.

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

### 2026-10-04: The diagram model is built in Rust, in its own crate

`crates/trace-view` turns a `Trace` into a diagram model (`docs/VIEW-MODEL.md`), so the UI only lays out and draws boxes. CLAUDE.md says all logic lives in Rust; grouping, collapsing, labels and status are logic. It is a separate crate, not part of `trace-core`, because `trace-core` is the schema my harness will emit and should stay small. It is harness-agnostic and has no I/O, so the Tauri backend can call it on every update.

### 2026-10-04: The diagram carries labels, not content

Diagram boxes hold short labels, timing, token numbers and status. Full prompts, outputs, tool inputs and results come from `node_detail` when a box is selected. Large sessions have hundreds of tool calls with long results, and sending all of that to draw boxes would be wasteful.

### 2026-10-04: A subagent is a child box of its tool call

A tool call that started a subagent stays a normal `tool_call` box, with a `subagent` box under it that is collapsed by default. The alternative was replacing the tool call with a subagent box. Keeping both shows the tool call's own status and timing, and gives the UI one obvious box to expand. A subagent is recognised only by having a child `Run`, never by tool name.

### 2026-10-04: Parallel group for two or more tool calls under one model call

The schema says all tool calls under one model call were requested together. A model call with two or more tool calls gets one `parallel_group` box around them, so the UI can draw them as a set without inspecting siblings.

### 2026-10-04: Summary rule for runs of similar calls

Three or more (`SUMMARY_MIN_CALLS`) consecutive model calls that each made exactly one successful tool call with the same tool name, and no subagent, collapse into one `summary` box. Markers between them do not break the run, because Claude Code logs a hook marker before every tool call when hooks are configured, which would otherwise stop any summary from forming. Errors, running calls and subagents always break a run, so they are never hidden inside a collapsed box. The rule is deliberately simple and easy to predict; the threshold of 3 means two repeated calls stay visible as they are.

### 2026-10-04: Container boxes show the worst status inside

Prompt, group, subagent and summary boxes show `error` if anything inside failed, at any depth, else `running` if anything is still running. A model call keeps its own status. This way an error inside a collapsed subagent is still visible.

### 2026-10-04: Every view model field is always present

Unlike the trace schema, which leaves out unknown optional fields, the view model always includes every field and uses `null` for unknown values. This gives the TypeScript side one fixed shape. A snapshot test of the JSON for the schema example guards the contract.

### 2026-10-04: Diagram ids are the kind plus a trace id

Box ids are `<kind>:<trace id>`, and synthetic boxes use the id of the node they belong to (`parallel_group:<model call id>`, `summary:<first box id>`). This makes them unique without counters and stable across reloads, which React Flow needs to keep layout and expanded state when M3 updates a diagram live.
### 2026-10-04: App crate is a plain binary called `snitchcraft`

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


### 2026-10-04: The app is called Snitchcraft

The working name `looptrace` is replaced by Snitchcraft, with the tagline "snitches get traces". The app crate, UI package, window title, bundle identifier (`dev.snitchcraft.app`) and snapshot env var (`SNITCHCRAFT_UPDATE_SNAPSHOTS`) use the new name. The library crates keep their descriptive names (`trace-core`, `trace-view`, `adapter-claude-code`) because they name what each crate does, not the brand.
