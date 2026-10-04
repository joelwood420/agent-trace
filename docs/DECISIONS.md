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

No UI package was added for M2 step 4. The diagram layout is hand-written, and UI unit tests use Node's built-in test runner (`node --test`), which runs TypeScript directly in Node 24, so no test framework is needed.

The app crate (`snitchcraft`) depends on the workspace's own `trace-view` crate to build the diagram model.

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

Updated: a third command, `node_detail`, was added later. See "load_session returns the diagram model, and node_detail is a third command" below.

`src-tauri/build.rs` lists the app commands with `AppManifest::commands`, so Tauri generates `allow-list-sessions` and `allow-load-session` permissions and denies any command not granted. `capabilities/default.json` grants exactly those two to the `main` window and nothing else: no `core:default`, no plugins. Without the list, every registered command would be open to every window. If the UI later needs a core API (for example window or event functions), the matching `core:` permission must be added to the capability.

### 2026-10-04: Strict content security policy

The production CSP allows scripts, styles, fonts and images only from the app itself (`'self'`, plus `data:` images), IPC through `ipc:` and `http://ipc.localhost`, and nothing else: no remote origins, objects, frames or form targets. `devCsp` additionally allows inline styles and the Vite dev server's websocket on `localhost:5173`, which hot reload needs. `freezePrototype` is on and the asset protocol is off. React sets inline styles through the DOM, which CSP does not block. If a library needs `<style>` tags injected at runtime, `style-src` has to be loosened, and that should be recorded here.

### 2026-10-04: Session file access is validated in Rust

The UI can only name a session by project folder name and session id, both returned by `list_sessions`. `load_session` accepts each only if it is a single plain path component (not empty, not `.` or `..`, no `/`, `\`, `:` or NUL), then canonicalises the file and checks it is still inside the projects folder, which also stops links that point elsewhere. The projects folder comes from Tauri's home directory lookup at startup. All file logic takes the folder as a parameter so tests run against `fixtures/` instead of the real home folder.

### 2026-10-04: Command arguments use snake_case

The commands are declared with `rename_all = "snake_case"`, so the UI calls `invoke('load_session', { project, session_id })`, matching the snake_case field names in every response. Tauri's default would be camelCase arguments next to snake_case results.

### 2026-10-04: Commands run file work on a blocking thread

The commands (now three, including `node_detail`) are `async` and do their file reading in `spawn_blocking`, so a large session never freezes the window. Errors are returned as plain messages, never as panics.

### 2026-10-04: Session list reads titles by scanning

`list_sessions` reads each transcript line by line and parses only lines that contain `"ai-title"`, keeping the last title. This is simple and was fast enough for the sessions on the development machine, but it reads whole files. If listing gets slow with many large sessions, it can read only the tail of each file or cache by modified time.


### 2026-10-04: The app is called Snitchcraft

The working name `looptrace` is replaced by Snitchcraft, with the tagline "snitches get traces". The app crate, UI package, window title, bundle identifier (`dev.snitchcraft.app`) and snapshot env var (`SNITCHCRAFT_UPDATE_SNAPSHOTS`) use the new name. The library crates keep their descriptive names (`trace-core`, `trace-view`, `adapter-claude-code`) because they name what each crate does, not the brand.

### 2026-10-04: load_session returns the diagram model, and node_detail is a third command

`load_session` builds the trace in Rust and returns `{ diagram, skipped }`: the `trace-view` session diagram plus the lines that could not be used. The UI never receives raw trace events, so it cannot reinterpret them. Events the trace rejects (which would be an adapter bug) are added to `skipped` instead of failing the whole session.

`node_detail(project, session_id, trace_id)` returns the full content of one trace node, or `null` if there is none. It is listed in `build.rs` and granted as `allow-node-detail`, so the window can now call exactly three commands. It validates `project` and `session_id` the same way as `load_session`, and rejects an empty or very long trace id.

### 2026-10-04: The last loaded session's trace is kept in memory

Clicking a box would otherwise re-read and re-parse the transcript every time. The app keeps one entry, the most recently loaded session's trace, behind a mutex. `load_session` always reads the file fresh and replaces the entry; `node_detail` uses the entry when the project and session id match and otherwise loads that session and caches it. One entry is enough because the UI shows one session at a time, and it keeps memory use bounded.

### 2026-10-04: Diagram layout is a hand-written indented tree

Positions are computed in `ui/src/layout.ts`, a pure function with unit tests. It is a top-down tree drawn like an outline: the children of a prompt, subagent or summary are stacked below their parent and indented, in order, and a model call's tool calls sit to its right on the same row. A classic centred tree (or dagre/elkjs) puts all of a prompt's model calls side by side, and real prompts have dozens to hundreds of model calls (one checked session had 245 in a single prompt), which makes rows thousands of pixels wide. The indented layout grows downwards, keeps time order top to bottom, and needs no library. Every box of a kind has a fixed size and single-line text, so the layout is exact without measuring the DOM.

A diagram opens fitted when it fits at a readable zoom, otherwise at the top-left at zoom 0.7 to 1, so a long prompt opens on its first steps. The view is left aligned so opening the details panel does not hide part of the tree. A minimap is shown only for diagrams with more than 30 boxes, since on small ones it only covers boxes.

### 2026-10-04: Expanded and collapsed state is UI state only

Which boxes are open is kept in the UI per session (a map from box id to open or closed); boxes not in the map use `collapsed_by_default`. "Expand all" and "Collapse all" act on the current prompt. Box ids are stable across reloads, so "Reload" keeps what was open.

### 2026-10-04: UI unit tests use Node's built-in test runner

`npm test` runs `node --test` on `src/**/*.test.ts`. Node 24 runs TypeScript directly by stripping types, so no test framework or extra package is needed. Test files are type-checked by `tsconfig.node.json` (Node types) and left out of `tsconfig.app.json`; they may only import modules that do not touch the DOM (`layout.ts`, `format.ts`, `grouping.ts`, `types.ts`). The layout tests run against the sanitised fixture's real diagram and check that no boxes overlap, groups contain their tool calls, and collapsing hides children.

### 2026-10-04: Dev-only mock mode for viewing the UI in a browser

Opening the Vite dev server with `?mock` replays the command responses for the sanitised fixture from `ui/src/mock/fixture-data.json`. A Rust test in `src-tauri` generates that file from the real command code and fails if it is out of date, so the mock cannot drift from the backend. The mock is loaded with a dynamic import behind `import.meta.env.DEV`, which is `false` in production builds, so neither the code nor the data is in `ui/dist` (checked by searching the built bundle for fixture strings). This made it possible to check the layout with browser screenshots, which the Tauri window does not allow.

### 2026-10-04: freezePrototype stays on; one d3-color statement is rewritten at build time

With Tauri's `freezePrototype` on, `Object.prototype` is frozen. d3-color, which React Flow uses for zoom and pan, runs `prototype.constructor = constructor` on a plain object at load time, and assigning a property that exists on a frozen prototype throws in strict mode. The whole UI failed to load in the app window (it worked in a normal browser, which is why the mock mode did not catch it). The fix keeps the hardening and rewrites that single statement to the equivalent `Object.defineProperty` call with a small Vite plugin in `vite.config.ts`, applied both to the production build and to the dev server's dependency pre-bundling. If d3-color changes and the statement disappears, the build fails so the fix gets reviewed. The alternative was turning `freezePrototype` off.

### 2026-10-04: How the production CSP was checked for style injection

The production CSP allows styles only from bundled files. All UI styles are in `index.css`, `app.css` and React Flow's `@xyflow/react/dist/style.css`, imported from `main.tsx`, and React Flow itself does not inject style tags. The only `createElement("style")` in the built bundle is React DOM's support for `<style precedence>` elements, which the app never renders. Box sizes and positions are set through React `style` props, which go through the DOM and are not blocked by `style-src`.

### 2026-10-04: Diagram boxes are keyboard accessible

Each box is a focusable element with the button role: Tab moves between boxes and Enter or Space opens the details, like a click. React Flow's own node focus is turned off because its built-in hint talks about moving and deleting nodes, which this read-only viewer does not allow. Status is shown by colour and also by an icon and a word, so it does not rely on colour alone.

### 2026-10-04: Decision models belong in the harness, not the viewer

Fast decision models such as Jev (TypeSafe AI, released 2026-09-15) answer typed questions about a piece of text with choices, scores, or probabilities. They were considered as a way to explain traces in Snitchcraft and rejected for that: they give their own judgement of the text, not the agent's real reasons, most labels they could add are already exact in the trace, and using one would send private transcripts to a third party. Instead, the M5 toy harness will try one inside its loop for routing and stop checks, and Snitchcraft will trace each decision call as its own node. That shows how often a router chose wrong, which is the debugging use this project exists for.

### 2026-10-05: Subagents are linked from their meta file as well as the tool result

A foreground Agent call only reports its subagent's id in the tool result when the subagent has finished, so a live view would not show a running subagent. The subagent's `.meta.json` names the starting tool call in `toolUseId`, so the adapter also links from there, as soon as that tool call is in the trace. Result-based linking is kept for subagents without that field (forked skills). This also applies to loading a finished session, so a subagent whose tool result is missing (for example an interrupted session) now shows up.

### 2026-10-05: Live reading holds back a last line with no newline

`TranscriptTail` only returns lines that end in a newline. Claude Code ends every line with one, so a line without it is still being written. Loading a finished file in one go (`load_session`) still accepts a valid JSON last line without a newline, as before.

### 2026-10-05: The backend sends a full diagram on every update (chosen by the project owner)

While a session runs, the backend keeps its parsers open and reads only new lines, but after each change it rebuilds the whole diagram model and sends it to the UI. This keeps all diagram logic in Rust and the UI a plain renderer. Diagrams are a few kilobytes, so resending is cheap. Revisit this and send only the changes if diagrams grow large enough for updates to feel slow, or when the M5 harness streams events directly.

### 2026-10-05: A session is live if written in the last 10 minutes (chosen by the project owner)

Claude Code writes no "session ended" line and any session can be resumed, so the transcript's last write time is the only signal. 10 minutes covers normal pauses while the user reads or types. The flag only drives the live markers; the open session is watched for as long as it is selected.

### 2026-10-05: Views carry a version number

Each `SessionView` has a `version` that goes up with every update of the open session. A live update can reach the UI before the reply to `load_session`, so the UI keeps whichever view has the higher version.
