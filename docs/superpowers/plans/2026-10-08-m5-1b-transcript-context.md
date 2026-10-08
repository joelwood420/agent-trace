# M5.1b Transcript Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry the hidden context Claude Code records in transcripts into the trace as a new `context_update` node and use it in the context breakdown, so transcript-only calls show system prompt, instruction files and reminders instead of one grey slice.

**Architecture:** `trace-core` gains `Node::ContextUpdate` (keyed parts plus removals, applied per run in tree order). The adapter maps Claude Code `attachment` lines to it. `insights` keeps a per-run context state in the transcript measure. The app exposes the main run's final state as `hidden_context`; the UI lists it and never draws these nodes as boxes.

**Tech Stack:** Rust stable (edition 2024), serde, Tauri 2, React + TypeScript.

**Spec:** `docs/superpowers/specs/2026-10-08-m5-1b-transcript-context-design.md` (read it before starting any task; its tables hold the exact keys, labels and texts).

## Global Constraints

- PowerShell on native Windows: every command in docs must work in PowerShell.
- No `unwrap()` or `expect()` outside tests. Unknown or malformed transcript data is skipped and logged, never a panic.
- `trace-core` stays harness neutral: no Claude Code names in it. Doc comments on every new public type and field (the schema is the contract).
- `insights` stays harness neutral: it only knows `context_update` and its part kinds.
- The UI renders what Rust returns; no measuring in TypeScript.
- Fixtures and test data are invented: no real transcript text, usernames or real paths. Use paths like `C:\work\example\CLAUDE.md`.
- No em dashes in prose, comments or docs.
- Commit after each task, message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; UI tasks also `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.
- Snapshot refresh (PowerShell): `$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test --workspace; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS`, then run without it and review every changed snapshot. If `crates/trace-view/tests/snapshots/example-trace.diagram.json` changes only in key order, revert it.

## Review Focus

1. A transcript where attachment lines arrive before the first prompt (the normal case): the `context_update` nodes must hang under the run and still apply to the first model call.
2. Subagent transcripts carry their own attachments: their parts must never leak into the main run's calls or the main run's `hidden_context` (Task 3 and Task 4 tests).
3. A live session: attachment lines appear while the app is watching; `context_update` events must flow through the live update path like other events without creating boxes or changing prompt counts (Task 4 test on `live-steps.json`).
4. Very large text (a 30k-character system prompt repeated in two snapshots, hundreds of reminder lines in a long session): the per-call measure must not clone part texts per call; store lengths, not text, in the running state (Task 3).
5. Old transcripts with none of these attachments behave exactly as before (Task 3 test: no parts gives the old rule 7 text).

---

### Task 1: `trace-core` node kind and schema docs

**Files:** `crates/trace-core/src/event.rs`, `crates/trace-core/src/lib.rs` (re-exports), `crates/trace-core/tests/*` as needed, `docs/SCHEMA.md`, plus the minimal match-arm additions everywhere `Node` is matched exhaustively so the workspace compiles: `crates/trace-view/src/build.rs`, `crates/trace-view/src/model.rs`, `crates/trace-view/src/detail.rs` (if it matches), `crates/adapter-claude-code/examples/print_tree.rs`, `crates/insights/src/transcript.rs`, `src-tauri/src/*` (grep for `Node::Marker`).

**Interfaces (produces):**

```rust
/// Parts of the hidden input a harness sends besides the conversation
/// (system prompt, tool definitions, injected instructions), as a change
/// to the run's current set. See docs/SCHEMA.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ContextUpdate {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ContextPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPart { pub key: String, pub kind: ContextPartKind, pub label: String, pub text: String }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPartKind { SystemPrompt, ToolDefinitions, Instructions, Reminder, Other(String) }
// Node gains: ContextUpdate(ContextUpdate)  -> JSON "type": "context_update"; kind_name() returns "context_update".
```

`ContextPartKind` must deserialise unknown string values to `Other(<value>)` rather than failing (custom `Deserialize` or `#[serde(untagged)]` fallback); `{"other": "x"}` also parses to `Other("x")`. Serialisation: known kinds as plain strings, `Other(x)` as `{"other": "x"}` (same convention as `StopReason`).

Steps:
- [ ] Failing tests in `event.rs`: round trip of a `context_update` event with two parts and one removal; `"kind": "made_up"` parses as `Other("made_up")`; empty `parts`/`remove` are omitted when serialising; `kind_name()` is `"context_update"`.
- [ ] Implement. Add a match arm wherever the compiler requires one: `trace-view` skips it completely: no box, not counted in "events before the first prompt" and not in prompt totals or status (add a hand-built trace test in `trace-view` for this now, so Task 2's snapshot refresh shows no diagram changes); `print_tree` prints a one-line `context update: N parts, M removed`; `insights` transcript walk ignores it for now (Task 3 implements it).
- [ ] `docs/SCHEMA.md`: add a `### context_update` section after `marker`, the context part table, the kind list, and the rules from the spec's "Schema" section; add a `context_update` line to the example only if the example file is easy to extend (do not change `fixtures/trace-core/example-trace.jsonl` otherwise).
- [ ] Full checks, commit "Add the context_update node to the trace schema".

### Task 2: Adapter mapping

**Files:** `crates/adapter-claude-code/src/parser.rs` (or a new `src/context_parts.rs` holding the mapping functions, called from `on_attachment`), adapter tests, `crates/adapter-claude-code/tests/basic_session.rs` (expected counts change).

**Interfaces:** consumes Task 1 types. Produces `context_update` events per the spec's adapter table, parented like markers (`turn_or_run()`), raw line kept, id `context:<line uuid>` (fallback `context:line-<n>`).

Steps:
- [ ] Failing unit tests with invented JSON lines, one per mapped attachment type, asserting key, kind, label and text exactly as in the spec table; `prompt_snapshot` with and without `tools`; `instructions` with two files and one without `path`; a line with wrong field types gives no part and no panic; an unknown attachment type gives no event.
- [ ] Failing test: a `compact_boundary` after two reminder updates emits a `context_update` whose `remove` lists exactly those two `reminder:` keys (and not `system` or `instructions:` keys), and reminders from a different run (subagent) are not included.
- [ ] Implement. Keep the existing hook markers for `hook_*` lines; `hook_additional_context` additionally emits the reminder part. Track emitted reminder keys per run for the compaction removal.
- [ ] Update `basic_session.rs` expectations (the sanitised fixture has these lines) and add a test counting context updates and parts by kind for the fixture.
- [ ] Refresh snapshots (command in Global Constraints) and review: `ui/src/mock/fixture-data.json` and `live-steps.json` may change only by trace ids lists if any; diagrams must show no new boxes. If a diagram snapshot gains boxes, that is a bug in Task 1's trace-view arm.
- [ ] Full checks, commit "Read the hidden context Claude Code records in transcripts".

### Task 3: `insights` context state

**Files:** `crates/insights/src/transcript.rs`, `measure.rs`, `breakdown.rs`, `advice.rs`.

**Interfaces:**
- `MeasuredItem` gains `pub from_transcript: bool` (set when the item comes from a context part); `ContextMeasure::add_part(&mut self, kind: SliceKind, label: &str, chars: u64)` adds an item with `from_transcript = true` (one occurrence). `Item` gains `pub from_transcript: bool` (serialised).
- `ContextMeasure` gains `pub fn has_parts(&self) -> bool`.
- In `measure_transcript_all`, per run: a state `BTreeMap<String, (ContextPartKind, String /*label*/, u64 /*chars*/)>` in first-set order (use an `IndexMap`-free approach: a `Vec<String>` of keys plus a `HashMap`, or keep insertion order manually; do not add dependencies). Apply each `ContextUpdate` (set/replace, then remove). At each model call, clone the conversation measure and add the state's parts (kind mapping per the spec). Store only lengths in the state.
- Compaction clears the conversation measure only.
- Rule 7: if `has_parts()` then the new text from the spec, else the current text.

Steps:
- [ ] Failing tests: replace by key (second system prompt replaces the first: one SystemPrompt item with the new length); remove drops a part; parts in a nested run do not reach the parent's calls and vice versa; compaction clears conversation but keeps parts; items from parts have `from_transcript` true and others false; rule 7 text with and without parts; a call before any context update has no parts.
- [ ] Implement; check existing tests still pass (old behaviour without parts unchanged).
- [ ] Full checks; refresh snapshots (context-data.json gains `from_transcript` fields) and review; commit "Count transcript context parts in the breakdown".

### Task 4: App and view model

**Files:** `src-tauri/src/context.rs`, `src-tauri/src/sessions.rs` snapshot tests, `ui/src/mock/*.json` (regenerated).

**Interfaces:** `SessionContext` gains `pub hidden_context: Vec<HiddenPart>` with

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HiddenPart { pub trace_id: String, pub key: String, pub kind: ContextPartKind, pub label: String, pub chars: u64 }
```

the final state of the main run (first run from `trace_view::model_calls_by_run`, or the first root run if it has no model calls), in first-set order, `trace_id` = the `context_update` node that last set the part. Built while copying facts under the lock (lengths only), like the transcript measures.

Steps:
- [ ] Failing tests: `hidden_context` for the fixture lists the expected kinds and excludes subagent parts; a live test (follow the existing live tests in `src-tauri/src/live.rs` or `sessions.rs`) where a context update line arrives later produces no new box.
- [ ] Implement; `node_detail` for a `context_update` id returns its node (check it already works generically).
- [ ] Refresh snapshots and review; full checks; commit "List the hidden context of the main agent".

### Task 5: UI

**Files:** `ui/src/types.ts`, `ui/src/context.ts` + test, `ui/src/components/ContextSection.tsx`, `ui/src/components/DetailsPanel.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/App.tsx` (selection of a trace id that has no box), `ui/src/app.css`.

**Interfaces:** TS types `ContextPartKind`, `ContextPart`, `ContextUpdateNode` (in the `TraceNode` union, `type: 'context_update'`), `HiddenPart`, `SessionContext.hidden_context`, `ContextItem.from_transcript`. Helper `hiddenContextRows(parts: HiddenPart[]): { traceId, kindName, label, size }[]` (size formatted like "27.5k chars") with tests.

Steps:
- [ ] Failing tests for `hiddenContextRows` (kind names: "System prompt", "Tool definitions", "Instructions", "Reminder", other names as given; size formatting; empty list).
- [ ] Session overview: a collapsed `<details>` "Hidden context (N)" under the Context card listing rows; clicking a row opens the details panel for that trace id even though no diagram box exists (extend App's selection so the details panel can show a node by trace id; do not select a box).
- [ ] Details panel for a `context_update` node: a "Parts" section with each part's label, kind and full text in a collapsible block, removed keys listed, and the existing raw source section.
- [ ] Context items with `from_transcript` show a small "from transcript" tag in the details breakdown.
- [ ] Run all UI checks; check in `?mock` (the fixture has context updates now); screenshot; commit "Show the hidden context Claude Code recorded".

### Task 6 (coordinator): real-app check and docs

- [ ] Real-app check: open a recent transcript-only session; the Context card should show system prompt, instructions and reminders, with "not captured" much smaller; compare with a captured run of the same setup (system prompt and reminder sizes within about 10%).
- [ ] Docs: `docs/VIEW-MODEL.md` (`hidden_context`, `from_transcript`), `docs/DECISIONS.md` (new node kind, reminder keys and compaction removal, not drawn as boxes), `docs/HARNESS-NOTES.md` (attachment mapping, what is still missing), `README.md`, `CLAUDE.md` milestone status.
- [ ] Final whole-branch review on the most capable model, one fix wave, report with "Rulings I made".
