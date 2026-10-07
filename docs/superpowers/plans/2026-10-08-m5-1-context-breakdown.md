# M5.1 Context Breakdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show what fills the model's context on every call (a stacked bar per model-call box, a breakdown in the details panel, a Context card with plain advice in the session overview).

**Architecture:** A new pure crate `insights` measures a captured request body or a transcript-only call in characters per slice, scales that to the reported input total, and writes advice sentences. The adapter supplies the two Claude Code specific rules as data. The app caches one measure per capture next to the existing summaries and serves two new commands; the UI only renders.

**Tech Stack:** Rust stable (edition 2024), serde, serde_json, Tauri 2, React + TypeScript + Vite + React Flow, Vitest.

**Spec:** `docs/superpowers/specs/2026-10-08-m5-1-context-breakdown-design.md` (read it before starting any task).

## Global Constraints

- PowerShell on native Windows: every command in this plan and in docs must work in PowerShell.
- No `unwrap()` or `expect()` outside tests (the workspace lints deny them). Return `Result` or handle `Option`.
- Unknown JSON shapes are skipped and measured as compact JSON, never a panic.
- `insights` depends only on `trace-core`, `serde`, `serde_json`. It must never mention Claude Code, `Read`, `system-reminder` or `mcp__` outside of tests. Exception: the `mcp__<server>__` prefix rule is an Anthropic API tool naming convention used by every MCP client and is allowed as a constant in `insights`.
- `trace-core` and `docs/SCHEMA.md` do not change.
- The UI does no measuring or scaling; it renders what the commands return.
- No em dashes in any prose, comments or docs.
- Doc comments on all public types and functions in `insights`.
- Fixtures are invented: nothing from a real `.claude` folder, no usernames, no `Users` or `AppData` paths.
- Commit after each task with a clear message for a public reader, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; for UI tasks also `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.

## Review Focus

1. A huge real request (300 KB, 186 tools, hundreds of messages): measuring must be a single pass with no quadratic string work, since it runs once per capture on first open.
2. A tool result whose `content` is a plain string vs an array of blocks vs missing: all three must measure without panicking (Task 2 test).
3. A `<system-reminder>` that opens but never closes, or closes without opening: the rest of the text is still counted exactly once (Task 2 test).
4. A call whose reported total is smaller than the transcript estimate (cache-heavy or compacted sessions): slices must still add up to the total shown, never a negative "not captured" (Task 3 test).
5. A live session where usage arrives after the capture: the bar must update when `session_context` is reloaded, because only the measure is cached, never the scaled breakdown (Task 6 test).

---

## File Structure

- Create `crates/insights/Cargo.toml`, `crates/insights/src/lib.rs` (re-exports), `src/rules.rs` (`ContextRules`), `src/measure.rs` (`ContextMeasure`, slices, items), `src/request.rs` (measure a request body), `src/transcript.rs` (measure from the trace), `src/breakdown.rs` (scaling, `ContextBreakdown`, `ContextBar`), `src/advice.rs` (advice rules and number formatting).
- Modify `Cargo.toml` (workspace dependency `insights`), `crates/adapter-claude-code/Cargo.toml` and `src/lib.rs` plus new `src/context.rs` (`context_rules()`).
- Modify `crates/capture/examples/make_fixture.rs`, regenerate `fixtures/captures/basic/calls.jsonl`.
- Create `src-tauri/src/context.rs` (session and call context views); modify `src-tauri/src/captures.rs` (measures in `SessionRecords`), `src-tauri/src/capture_sink.rs` (command helpers), `src-tauri/src/main.rs` (commands), `src-tauri/Cargo.toml`, `src-tauri/src/sessions.rs` (snapshot test).
- UI: modify `ui/src/types.ts`, `ui/src/api.ts`, `ui/src/mock/mockApi.ts`; create `ui/src/mock/context-data.json` (generated), `ui/src/context.ts` + `ui/src/context.test.ts`, `ui/src/components/ContextBar.tsx`, `ui/src/components/ContextSection.tsx`; modify `ui/src/components/nodes.tsx`, `ui/src/layout.ts`, `ui/src/components/Diagram.tsx`, `ui/src/App.tsx`, `ui/src/components/DetailsPanel.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/app.css`.
- Docs (Task 9, coordinator): `docs/VIEW-MODEL.md`, `docs/DECISIONS.md`, `docs/HARNESS-NOTES.md`, `README.md`, `CLAUDE.md`.

---

### Task 1: `insights` crate, rules and measure types

**Files:**
- Create: `crates/insights/Cargo.toml`, `crates/insights/src/lib.rs`, `crates/insights/src/rules.rs`, `crates/insights/src/measure.rs`
- Modify: `Cargo.toml` (root, `[workspace.dependencies]`)

**Interfaces:**
- Produces (used by every later task):

```rust
// rules.rs
/// What a harness's injected reminders and file reads look like. Plain data
/// supplied by the adapter, so this crate stays harness neutral.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextRules {
    /// Text that opens an injected reminder, for example a tag.
    pub reminder_open: String,
    /// Text that closes an injected reminder.
    pub reminder_close: String,
    /// Text that starts a new item inside a reminder, if the harness has one.
    pub section_marker: Option<String>,
    /// Names for reminder sections; the first rule whose needle appears wins.
    pub labels: Vec<LabelRule>,
    /// Tools whose result is the content of a file named in their input.
    pub file_reads: Vec<FileReadRule>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct LabelRule {
    /// Text that must appear in the section.
    pub needle: String,
    /// The name shown for the section.
    pub label: String,
    /// When true and the section starts with the section marker, the text
    /// after the marker up to the first " (" or line end is added as
    /// "<label>: <detail>".
    pub with_detail: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct FileReadRule {
    /// Tool name, matched exactly.
    pub tool: String,
    /// Input field holding the file path.
    pub path_field: String,
}

// measure.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceKind { SystemPrompt, ToolDefinitions, Instructions, FilesRead, ToolResults, Conversation, NotCaptured }
impl SliceKind {
    /// Every kind, in display order.
    pub const ALL: [SliceKind; 7] = [/* in the order above */];
    /// The name shown in the UI: "System prompt", "Tool definitions",
    /// "Instructions and reminders", "Files read", "Other tool results",
    /// "Conversation", "Not captured".
    pub fn label(self) -> &'static str;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource { Captured, Transcript }

/// One named part of a slice, in characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredItem { pub label: String, pub chars: u64, pub count: u32, pub largest_chars: u64 }

/// A call's context measured in characters, slice by slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMeasure { pub source: ContextSource, slices: BTreeMap<SliceKind, Vec<MeasuredItem>> }
impl ContextMeasure {
    pub fn new(source: ContextSource) -> Self;
    /// Adds `chars` to the item `label` of slice `kind`, creating it if
    /// needed. Each call is one occurrence: `count` goes up by one and
    /// `largest_chars` keeps the biggest single occurrence.
    pub fn add(&mut self, kind: SliceKind, label: &str, chars: u64);
    /// Like `add` but adds `count` occurrences at once (for tool sets:
    /// one item per MCP server whose count is its number of tools).
    pub fn add_many(&mut self, kind: SliceKind, label: &str, chars: u64, count: u32);
    /// Clears everything collected (used at a compaction marker).
    pub fn clear(&mut self);
    /// The items of one slice, in insertion order.
    pub fn items(&self, kind: SliceKind) -> &[MeasuredItem];
    /// Total characters of one slice.
    pub fn slice_chars(&self, kind: SliceKind) -> u64;
    /// Total characters of every slice.
    pub fn total_chars(&self) -> u64;
}
/// Character weight of one image, so images show up without decoding them.
pub const IMAGE_CHARS: u64 = 6_000;
/// Length of `value` as compact JSON text.
pub fn json_chars(value: &serde_json::Value) -> u64;
/// Length of `text` in characters (`text.chars().count()`).
pub fn text_chars(text: &str) -> u64;
```

- [ ] **Step 1: Create the crate.** `crates/insights/Cargo.toml`:

```toml
[package]
name = "insights"
description = "Harness-neutral analysis of what fills a model call's context."
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
trace-core.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

Add `insights = { path = "crates/insights" }` to the root `[workspace.dependencies]`. `lib.rs` starts with a crate doc comment ("What fills a model call's context: measure a request or a trace, scale to tokens, explain the biggest costs. Nothing here knows which harness produced the data; harness rules come in as `ContextRules`."), declares the modules and re-exports their public items. Add `tracing` because Task 2 logs unknown block types; record nothing in DECISIONS yet (Task 9 does).

- [ ] **Step 2: Write failing tests in `measure.rs`.**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_merges_items_by_label_and_keeps_the_largest() {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        m.add(SliceKind::FilesRead, "a.rs", 100);
        m.add(SliceKind::FilesRead, "a.rs", 300);
        m.add(SliceKind::FilesRead, "b.rs", 50);
        let items = m.items(SliceKind::FilesRead);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], MeasuredItem { label: "a.rs".into(), chars: 400, count: 2, largest_chars: 300 });
        assert_eq!(m.slice_chars(SliceKind::FilesRead), 450);
        assert_eq!(m.total_chars(), 450);
    }

    #[test]
    fn add_many_counts_several_occurrences() {
        let mut m = ContextMeasure::new(ContextSource::Captured);
        m.add_many(SliceKind::ToolDefinitions, "MCP: docs", 900, 3);
        assert_eq!(m.items(SliceKind::ToolDefinitions)[0].count, 3);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut m = ContextMeasure::new(ContextSource::Transcript);
        m.add(SliceKind::Conversation, "your prompts", 10);
        m.clear();
        assert_eq!(m.total_chars(), 0);
        assert!(m.items(SliceKind::Conversation).is_empty());
    }

    #[test]
    fn json_and_text_lengths() {
        assert_eq!(json_chars(&json!({"a": 1})), 7);
        assert_eq!(text_chars("héllo"), 5);
    }

    #[test]
    fn kinds_have_labels_in_display_order() {
        assert_eq!(SliceKind::ALL[0], SliceKind::SystemPrompt);
        assert_eq!(SliceKind::ALL[6], SliceKind::NotCaptured);
        assert_eq!(SliceKind::Instructions.label(), "Instructions and reminders");
    }
}
```

- [ ] **Step 3: Run** `cargo test -p insights`. Expected: compile errors (types missing).
- [ ] **Step 4: Implement** the types and functions exactly as in Interfaces. `json_chars` uses `serde_json::to_string(value).map(|s| s.chars().count() as u64).unwrap_or(0)`. `items` returns `&[]` for an absent slice.
- [ ] **Step 5: Run** `cargo test -p insights`. Expected: PASS. Run fmt and clippy.
- [ ] **Step 6: Commit** "Add the insights crate with context rules and measures".

---

### Task 2: Measure a captured request body

**Files:**
- Create: `crates/insights/src/request.rs`
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 types.
- Produces: `pub fn measure_request(body: &serde_json::Value, rules: &ContextRules) -> ContextMeasure` (source `Captured`). Also `pub(crate) fn split_reminders(text: &str, rules: &ContextRules, out: &mut ContextMeasure) -> u64`: adds each reminder section to `Instructions` and returns the character count of the text outside reminders, which the caller adds to its own slice.

Measuring rules (from the spec, restated so this task stands alone):

- `system`: a string is one item "block 1"; an array gives "block 1", "block 2"... using each block's `text` length, or compact JSON length for non-text blocks.
- `tools`: each tool's compact JSON length. Name starts with `mcp__`: the server is the text between `mcp__` and the next `__` (whole remainder if there is none); item label `MCP: <server>`. Otherwise item "built-in". Use `add_many(kind, label, chars, 1)` per tool so `count` is the number of tools.
- `messages`: for each message, `content` may be a string (treat as one text block) or an array.
  - role `assistant`: `text` and `thinking` (`thinking` field) blocks go to Conversation "model replies"; `redacted_thinking` counts its `data` length in "model replies"; `tool_use` counts compact JSON of `input` in Conversation "tool inputs" and records `id -> (name, input)` for later results.
  - role `user`: `text` goes through `split_reminders` with outside = (Conversation, "your prompts"); `image` counts `IMAGE_CHARS` in Conversation "images"; `tool_result` looks up its `tool_use_id`.
  - `tool_result`: the tool is found from the recorded `tool_use`. If the tool matches a `FileReadRule` and the input has that field as a string, its content goes to FilesRead under that path; else to ToolResults under the tool name; an unknown id goes to ToolResults "unknown tool". The result's `content` may be a string, an array of blocks, or missing (zero). Text inside is passed through `split_reminders` with outside = (that slice, that label), so reminders appended to results count as Instructions. Images inside count `IMAGE_CHARS` in the same slice and label. Each result is ONE occurrence of its item: sum its outside characters and call `add` once (so `count` is the number of results and `largest_chars` the biggest result).
  - Any other block type anywhere: compact JSON length in Conversation "other", and `tracing::debug!(kind, "unknown content block measured as JSON")`.
- Every other top-level key (model, max_tokens, settings) is ignored. A body that is not an object gives an empty measure.

`split_reminders` behaviour:

- Walk the text with `find`, no regex. Outside text accumulates to the outside slice. A reminder starts at `reminder_open` and ends after the next `reminder_close`; an unclosed reminder runs to the end of the text. A stray `reminder_close` with no open is ordinary text.
- If `reminder_open` is empty the whole text is outside.
- Inside a reminder (tags excluded), if `section_marker` is set, split at every occurrence of the marker; each piece starting with the marker is one section, and the piece before the first marker is a section too if it is not only whitespace. Without a marker the whole reminder is one section.
- Each section is labelled by the first `LabelRule` whose needle it contains. With `with_detail` and a section that starts with the marker, the detail is the text after the marker up to the first `" ("` or newline, trimmed: label `"<label>: <detail>"`. No rule: "other reminders". Add each section with `add(Instructions, label, chars)`.
- `split_reminders` never adds outside text itself; it returns its character count. A user `text` block adds that count to Conversation "your prompts" (one occurrence per block, skipped when zero). A tool result sums the counts of all its texts and adds them once to its own slice and label.

- [ ] **Step 1: Write failing tests** in `request.rs` using a test rules value:

```rust
fn rules() -> ContextRules {
    ContextRules {
        reminder_open: "<r>".into(),
        reminder_close: "</r>".into(),
        section_marker: Some("Contents of ".into()),
        labels: vec![
            LabelRule { needle: "NOTES.md".into(), label: "memory".into(), with_detail: true },
            LabelRule { needle: "GUIDE.md".into(), label: "guide".into(), with_detail: true },
            LabelRule { needle: "skills".into(), label: "skills list".into(), with_detail: false },
        ],
        file_reads: vec![FileReadRule { tool: "ReadFile".into(), path_field: "path".into() }],
    }
}
```

Tests (each a `#[test]` with exact expected numbers computed from the literal strings; write the inputs with `json!`):

1. `system_string_and_blocks`: string system gives one item "block 1"; array of two text blocks gives "block 1" and "block 2" with their lengths.
2. `tools_group_by_mcp_server`: tools `Bash`, `mcp__docs__search`, `mcp__docs__fetch`, `mcp__odd` give items "built-in" (count 1), "MCP: docs" (count 2), "MCP: odd" (count 1), chars equal to the compact JSON lengths.
3. `assistant_blocks`: text, thinking, redacted_thinking, tool_use go to Conversation "model replies" and "tool inputs".
4. `reminders_split_into_sections`: user text `"hi <r>intro Contents of /p/GUIDE.md (project):\nbe kind Contents of /p/NOTES.md (memory):\nx</r> bye"` gives Instructions items "other reminders" (for "intro "), "guide: /p/GUIDE.md", "memory: /p/NOTES.md", and Conversation "your prompts" chars = len("hi ") + len(" bye").
5. `unclosed_and_stray_tags`: `"a </r> b <r>rest"`: outside chars = len("a </r> b "), one Instructions item "other reminders" with len("rest").
6. `file_reads_match_by_tool_use_id`: assistant tool_use `{id:"t1", name:"ReadFile", input:{path:"src/a.rs"}}` twice (ids t1, t2) and user tool_results for both, one with string content, one with array content: FilesRead item "src/a.rs" with count 2, largest_chars the bigger one.
7. `other_results_unknown_ids_and_missing_content`: a `Grep` result goes to ToolResults "Grep"; a result for id "zzz" goes to "unknown tool"; a result with no `content` counts zero characters but still one occurrence.
8. `reminder_inside_tool_result_counts_as_instructions`: a ReadFile result "line1<r>skills: a, b</r>" gives FilesRead chars len("line1") and Instructions "skills list".
9. `images_and_unknown_blocks`: a user image counts `IMAGE_CHARS` in Conversation "images"; a block `{type:"mystery", x:1}` counts its compact JSON in Conversation "other".
10. `not_an_object_is_empty`: `json!([1,2])` gives total 0.

- [ ] **Step 2: Run** `cargo test -p insights request`. Expected: FAIL (function missing).
- [ ] **Step 3: Implement** `measure_request` and `split_reminders` as described. Keep functions small: `measure_system`, `measure_tools`, `measure_messages`, `measure_assistant_block`, `measure_user_block`, `measure_tool_result`. A single pass over messages; `HashMap<String, (String, Option<String>)>` from tool use id to (tool name, file path).
- [ ] **Step 4: Run** `cargo test -p insights`. Expected: PASS. fmt, clippy.
- [ ] **Step 5: Commit** "Measure what fills a captured request".

---

### Task 3: Scale to tokens: `ContextBreakdown` and `ContextBar`

**Files:**
- Create: `crates/insights/src/breakdown.rs`
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 types.
- Produces:

```rust
/// Characters per token used when no reported total is known.
pub const CHARS_PER_TOKEN: f64 = 4.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextBreakdown {
    pub source: ContextSource,
    pub total_tokens: u64,
    pub total_is_reported: bool,
    pub slices: Vec<Slice>,
    pub advice: Vec<Advice>, // filled by Task 5; empty until then
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Slice { pub kind: SliceKind, pub label: String, pub tokens: u64, pub share: f64, pub items: Vec<Item> }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Item { pub label: String, pub tokens: u64, pub count: u32, pub largest_tokens: u64 }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContextBar { pub source: ContextSource, pub total_tokens: u64, pub total_is_reported: bool, pub slices: Vec<BarSlice> }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarSlice { pub kind: SliceKind, pub tokens: u64 }

/// Scales a measure to tokens. `reported_total` is the model call's input
/// context as the API reported it, if known.
pub fn breakdown(measure: &ContextMeasure, reported_total: Option<u64>) -> ContextBreakdown;
impl ContextBreakdown { pub fn bar(&self) -> ContextBar; }
```

Define `Advice` and `AdviceLevel` in `advice.rs` now as plain types (Task 5 fills the rules):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdviceLevel { Warn, Info }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Advice { pub level: AdviceLevel, pub slice: SliceKind, pub text: String }
```

Scaling rules:

- Captured with `Some(total)` and measured chars > 0: `ratio = total / chars` tokens per char. Captured otherwise: `ratio = 1 / CHARS_PER_TOKEN`, total = rounded sum, `total_is_reported = false` when total was `None`. (Measured chars of zero with `Some(total)`: no slices, total shown, reported true.)
- Transcript: estimate each slice with `1 / CHARS_PER_TOKEN`. With `Some(total)`: if the estimate sum <= total, NotCaptured = total - sum; else scale the estimated slices down by `total / sum` and NotCaptured = 0. Without a total: no NotCaptured slice, total = sum, not reported.
- Each slice's tokens = round(chars * ratio). Then the largest slice gets `total - sum_of_rounded` added (may be negative adjustment; use i128 math and clamp at 0). Slices with 0 tokens are dropped. Order follows `SliceKind::ALL`.
- `share = tokens / total` (0 when total is 0).
- Items: tokens = round(chars * ratio) (no exact-sum fix), largest_tokens likewise; sorted by tokens descending, ties by label.
- Slice label from `SliceKind::label`.
- `bar()` copies source, totals and `(kind, tokens)` per slice.

- [ ] **Step 1: Write failing tests:**

1. `captured_slices_add_up_to_the_reported_total`: measure with SystemPrompt 1000 chars, ToolDefinitions 3000, Conversation 1; `breakdown(&m, Some(1001))`: system 250, tools 751, the Conversation slice rounds to 0 and is dropped, slices sum to 1001, total_is_reported true. Add a second case where rounding leaves a gap (SystemPrompt 3 chars, ToolDefinitions 3 chars, total 5: both round to 3 (2.5 rounds up), sum 6, the largest (first in order on a tie: SystemPrompt) absorbs -1 -> 2 and 3).
2. `captured_without_total_uses_four_chars_per_token`: 4000 chars -> 1000 tokens, reported false.
3. `transcript_adds_not_captured`: Conversation 4000 chars, total 5000 -> Conversation 1000, NotCaptured 4000.
4. `transcript_estimate_larger_than_total_is_scaled_down`: Conversation 8000 chars, FilesRead 8000 chars, total 1000 -> slices sum 1000, NotCaptured absent.
5. `transcript_without_total_has_no_not_captured`.
6. `items_are_sorted_largest_first_with_largest_tokens`.
7. `bar_keeps_kinds_and_tokens_only`.
8. `zero_measure_with_total`: empty captured measure, `Some(500)` -> no slices, total 500.

- [ ] **Step 2: Run** `cargo test -p insights breakdown`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** tests, fmt, clippy. Expected: PASS.
- [ ] **Step 5: Commit** "Scale context measures to the reported token total".

---

### Task 4: Measure a call from the trace alone

**Files:**
- Create: `crates/insights/src/transcript.rs`
- Modify: `crates/insights/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 types, `split_reminders` is NOT used here (the transcript has no injected reminders; prompts are plain "your prompts").
- Produces:

```rust
/// Measures every model call of the trace from the trace alone, one pass
/// per run. Keys are model call ids.
pub fn measure_transcript_all(trace: &Trace, rules: &ContextRules) -> HashMap<String, ContextMeasure>;
/// Measures one model call from the trace alone. `None` if `model_call_id`
/// is not a model call.
pub fn measure_transcript(trace: &Trace, model_call_id: &str, rules: &ContextRules) -> Option<ContextMeasure>;
```

Algorithm (per run, depth first in child order, never descending into a nested `Run`):

- Keep one running `ContextMeasure::new(Transcript)`.
- `Turn`: add each prompt block (Text: chars; Image: `IMAGE_CHARS` to Conversation "images"; Thinking: chars; Other: `kind` length) to Conversation "your prompts" as one occurrence per turn. Then visit children.
- `ModelCall`: first store a clone of the running measure as this call's measure. Then add its `output` Text and Thinking blocks to Conversation "model replies" (one occurrence per call when non-empty). Then visit children.
- `ToolCall`: add compact JSON of `input` to Conversation "tool inputs". If it has a result: if the tool name matches a `FileReadRule` and `input[path_field]` is a string, result chars go to FilesRead under that path; otherwise to ToolResults under the tool name. Result chars: Text length, Image `IMAGE_CHARS`, Thinking length, Other kind length; one occurrence per result. Visit children except nested runs (those are measured as their own run).
- `Marker` with `kind == "compaction"`: `clear()` the running measure.
- `measure_transcript` finds the run holding the call (walk parents to the nearest `Run`), measures that run, and returns the call's entry.
- `measure_transcript_all` visits every run, including nested ones (use `trace_view`-like recursion locally; `insights` must not depend on `trace-view`).

- [ ] **Step 1: Write failing tests** with a small hand-built trace (copy the `add`, `run`, `turn` helpers style from `crates/trace-view/src/order.rs` tests): run R; turn T1 prompt "hello" (5 chars); M1 output "ok"; tool X (`ReadFile`, input `{"path":"a.rs"}`, result text of 40 chars); M2; marker C kind "compaction" under T1 after M2; M3; nested run S under tool X with S1.

1. `first_call_sees_only_the_prompt`: M1 measure = Conversation "your prompts" 5.
2. `later_call_sees_replies_inputs_and_results`: M2 has "model replies" 2, "tool inputs" = json len of the input, FilesRead "a.rs" 40.
3. `compaction_resets_history`: M3 measure total 0 (marker cleared everything before it, and nothing was added after).
4. `subagent_run_is_separate`: S1 measure total 0 and M2 does not include anything from S.
5. `not_a_model_call_is_none`: `measure_transcript(&t, "T1", &rules)` is `None`.
(The fixture test for this function lives in Task 6, because `insights` must not depend on the adapter, even as a dev-dependency: the adapter depends on `insights`.)

- [ ] **Step 2: Run** `cargo test -p insights transcript`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** tests, fmt, clippy. Expected: PASS.
- [ ] **Step 5: Commit** "Measure a call's context from the transcript alone".

---

### Task 5: Advice rules

**Files:**
- Modify: `crates/insights/src/advice.rs`, `crates/insights/src/breakdown.rs` (call `advise` at the end of `breakdown`)

**Interfaces:**
- Consumes: `ContextBreakdown`, `Slice`, `Item` (Task 3).
- Produces: `pub fn advise(b: &ContextBreakdown) -> Vec<Advice>`, and `pub fn about_tokens(n: u64) -> String` ("about 9k" from 1,000 up with `(n + 500) / 1000`, else the exact number, e.g. "about 41k", "640"). Named threshold constants:

```rust
pub const TOOLS_SHARE_WARN: f64 = 0.20;
pub const MCP_SERVER_TOKENS_WARN: u64 = 2_000;
pub const INSTRUCTIONS_TOKENS_WARN: u64 = 5_000;
pub const FILE_REPEAT_WARN: u32 = 3;
pub const SINGLE_RESULT_SHARE_WARN: f64 = 0.10;
pub const CONVERSATION_SHARE_INFO: f64 = 0.60;
```

Rules, in output order; `pct(share)` is `(share * 100).round()` as an integer:

1. Warn, ToolDefinitions, when tools share >= 0.20: `"Tool definitions are {pct}% of this call ({about} tokens): {n} tools, {m} from MCP servers."` where n = sum of item counts and m = sum of counts of items whose label starts with `"MCP: "`. Leave out ", {m} from MCP servers" when m is 0.
2. Warn, ToolDefinitions, per item labelled `MCP: <server>` with tokens >= 2,000, largest first: `"MCP server `<server>` adds {about} tokens to every call ({count} tools). Turn it off in projects that do not use it."` (use the singular "tool" for 1).
3. Warn, Instructions, slice tokens >= 5,000: `"Instructions and reminders add {about} tokens to every call. Largest: {item label} ({about item})."`
4. Warn, FilesRead, per item with count >= 3, largest first: `"`{path}` is in context {count} times ({about} tokens). Each read adds the full file again."`
5. Warn, FilesRead or ToolResults, per item whose `largest_tokens / total >= 0.10`, largest first: files: `"One read of `{path}` is {pct}% of this call ({about} tokens)."`; others: `"One `{tool}` result is {pct}% of this call ({about} tokens)."`
6. Info, Conversation, share >= 0.60: `"Conversation history is {pct}% of this call. /compact or a fresh session would shrink it."`
7. Info, NotCaptured, when source is Transcript: `"The system prompt, tool definitions and instructions are not visible for this call. Run the session through the capture proxy to see them."` (no harness name, so the crate stays neutral).

- [ ] **Step 1: Write failing tests**: one test per rule with three cases (just below, at, just above the threshold), built by constructing `ContextBreakdown` values directly; plus `about_tokens` cases 0, 999, 1000, 1499, 1500, 41_234. Plus `breakdown_includes_advice`: a transcript measure gets rule 7.
- [ ] **Step 2: Run** `cargo test -p insights advice`. Expected: FAIL.
- [ ] **Step 3: Implement** and call `advise` from `breakdown`.
- [ ] **Step 4: Run** tests, fmt, clippy. Expected: PASS.
- [ ] **Step 5: Commit** "Explain the biggest context costs as plain advice".

---

### Task 6: Claude Code rules and a richer capture fixture

**Files:**
- Create: `crates/adapter-claude-code/src/context.rs`
- Modify: `crates/adapter-claude-code/src/lib.rs`, `crates/adapter-claude-code/Cargo.toml` (`insights.workspace = true`), `crates/capture/examples/make_fixture.rs`, `crates/capture/Cargo.toml` only if the example needs a new dev-dependency, regenerate `fixtures/captures/basic/calls.jsonl`, then refresh `ui/src/mock/capture-data.json` with the snapshot switch.

**Interfaces:**
- Produces: `pub fn context_rules() -> insights::ContextRules` re-exported from `adapter_claude_code`:

```rust
ContextRules {
    reminder_open: "<system-reminder>".into(),
    reminder_close: "</system-reminder>".into(),
    section_marker: Some("Contents of ".into()),
    labels: vec![
        LabelRule { needle: "MEMORY.md".into(), label: "memory".into(), with_detail: true },
        LabelRule { needle: "CLAUDE.md".into(), label: "CLAUDE.md".into(), with_detail: true },
        LabelRule { needle: "skills are available".into(), label: "skills list".into(), with_detail: false },
        LabelRule { needle: "deferred tools".into(), label: "deferred tools list".into(), with_detail: false },
    ],
    file_reads: vec![FileReadRule { tool: "Read".into(), path_field: "file_path".into() }],
}
```

- [ ] **Step 1: Test the rules** in `context.rs`: `insights::measure_request` on a small body whose first user message has `"<system-reminder>\nContents of C:\\work\\example\\CLAUDE.md (project instructions):\nBe brief.\nContents of C:\\work\\example\\memory\\MEMORY.md (memory):\n- a note\n</system-reminder>"` gives Instructions items `"CLAUDE.md: C:\\work\\example\\CLAUDE.md"` and `"memory: C:\\work\\example\\memory\\MEMORY.md"`; and a `Read` tool use plus result lands in FilesRead under its `file_path`. Run, see it fail, implement, see it pass.
- [ ] **Step 1b: Fixture history test** in `crates/adapter-claude-code/tests/context_fixture.rs`: load `fixtures/claude-code/basic/00000000-0000-4000-8000-000000000002.jsonl` with `load_session`, apply the events into a `Trace`, call `insights::measure_transcript_all(&trace, &context_rules())`, take the main run's model calls in order (walk the first root `Run` the way `trace_view::model_calls_by_run` does, or add `trace-view` as a dev-dependency of the adapter if it is not one already and does not create a cycle; `trace-view` only dev-depends on the adapter, so check `cargo tree` first and prefer the local walk), and assert that total characters never decrease from one call to the next and the last is > 0.
- [ ] **Step 2: Extend the fixture maker.** In `make_fixture.rs`:
  - Add two invented MCP tools to `base_tools()` (so all calls have them): `mcp__docs__search` and `mcp__docs__fetch`, each with a description of about 1,500 invented characters (build it with `"Search the invented documentation index. ".repeat(36)` style text) so the MCP slice is visible.
  - In `messages()`, prepend to the FIRST user message of each run a text block holding an invented reminder: main run: `<system-reminder>\nAs you answer, use this context:\nContents of C:\\work\\example\\CLAUDE.md (project instructions):\n` followed by about 2,000 characters of invented instructions, then `\nContents of C:\\work\\example\\memory\\MEMORY.md (memory):\n- Invented note one.\n- Invented note two.\n</system-reminder>`; subagent run: a short `<system-reminder>\nThe following skills are available: invented-skill-a, invented-skill-b.\n</system-reminder>`.
  - Keep the `FORBIDDEN` check passing (no `Users`, `AppData`, `Bearer`).
  - Run `cargo run -p capture --example make_fixture`. Expected: "wrote N records".
- [ ] **Step 3: Refresh mock data** in PowerShell: `$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS`. Then run `cargo test --workspace` without the variable. Review `git diff --stat`: only `calls.jsonl` and `capture-data.json` (and maybe `fixture-data.json` if unchanged it stays) should change. Any test in `src-tauri` or `capture` that pinned tool counts or sizes from the fixture is updated to the new values with a one-line reason in the test.
- [ ] **Step 4: Run** fmt, clippy, all tests. Expected: PASS.
- [ ] **Step 5: Commit** "Add the Claude Code context rules and richer invented captures".

---

### Task 7: App: measures per capture and the two context commands

**Files:**
- Create: `src-tauri/src/context.rs`
- Modify: `src-tauri/Cargo.toml` (`insights.workspace = true`), `src-tauri/src/captures.rs`, `src-tauri/src/capture_sink.rs`, `src-tauri/src/main.rs`, `src-tauri/src/sessions.rs` (snapshot test)

**Interfaces:**
- Consumes: `insights::{measure_request, measure_transcript_all, measure_transcript, breakdown, ContextMeasure, ContextBreakdown, ContextBar}`, `adapter_claude_code::context_rules`.
- Produces (Rust, serialised to the UI):

```rust
// context.rs
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionContext {
    /// Bar per model call trace id, for every model call with a known total or a capture.
    pub bars: BTreeMap<String, ContextBar>,
    /// The latest model call of the main run, in full.
    pub latest: Option<ContextBreakdown>,
    /// Its trace id.
    pub latest_trace_id: Option<String>,
}
/// What the context views need from a trace, copied under the session lock.
pub struct ContextFacts { /* transcript measures by call id, reported totals by call id, main run's last call id, model call ids */ }
impl ContextFacts { pub fn of(trace: &Trace) -> Self; pub fn of_call(trace: &Trace, trace_id: &str) -> Self; }
pub fn session_context(facts: &ContextFacts, captures: &SessionRecords) -> SessionContext;
pub fn call_context(facts: &ContextFacts, captures: &SessionRecords, trace_id: &str) -> Option<ContextBreakdown>;
```

- Tauri commands (snake_case args like the others): `session_context(project, session_id) -> SessionContext` and `call_context(project, session_id, trace_id) -> Option<ContextBreakdown>`.

Steps:

- [ ] **Step 1: Cache measures.** In `SessionRecords` add `pub measures: Arc<HashMap<String, Option<ContextMeasure>>>`. `summarise_missing` also builds the measure for each record without one, from the same `record.request.body.json()` value (`None` when the body is not JSON), via `insights::measure_request(body, &context_rules())`. Build the rules once per `summarise_missing` call. Extend the existing "summarised once" test (it counts `SUMMARISE_CALLS`) with a `MEASURE_CALLS` counter in the same style, asserting each record is measured once across two `summarise_missing` calls.
- [ ] **Step 2: Write failing tests in `context.rs`** using `crate::captures::fixture_records()` and the fixture trace (see `ui_capture_data_matches_the_backend` in `sessions.rs` for how they load):
  1. `captured_calls_use_the_capture`: for a model call with a mapped capture, the bar's source is `captured` and its slices include `tool_definitions`.
  2. `uncaptured_calls_use_the_transcript`: with an empty `SessionRecords`, every model call with usage has a bar with source `transcript`.
  3. `slices_add_up_to_the_reported_total`: for every bar with `total_is_reported`, the slice tokens sum to `total_tokens`.
  4. `latest_is_the_main_runs_last_call`: `latest_trace_id` equals the last id of the first run from `trace_view::model_calls_by_run`.
  5. `usage_arriving_later_updates_the_bar` (Review Focus 5): build facts from a trace where the call has no usage, then from the same trace with usage set; the second bar has `total_is_reported` true and the measure was not rebuilt (MEASURE_CALLS unchanged).
  6. `unknown_call_is_none`: `call_context` for `"turn:x"` is `None`.
- [ ] **Step 3: Implement** `ContextFacts::of` (one `measure_transcript_all` pass plus `Usage::context_tokens()` per model call) and `of_call` (one `measure_transcript` plus that call's total). A capture maps to a call the same way `TraceFacts::trace_id` does (`adapter_claude_code::model_call_id` of the record's `message_id`); add a small `pub(crate)` helper on `TraceFacts` or reuse `mapped_id` rather than duplicating it. The capture's measure wins over the transcript measure when both exist and the capture's measure is `Some`. A call with neither a total nor a capture gets no bar.
- [ ] **Step 4: Wire commands** in `capture_sink.rs` following `session_overview`: validate names with `session_path`, check the trace id with `sessions::check_trace_id`, get records with `cache.get_summarised(store, key)` (empty `SessionRecords` when the session has no key), copy `ContextFacts` while holding the open session's lock (or from a freshly opened `LiveSession` when it is not the open one), then build the view outside the lock. Register both in `main.rs` next to `session_captures`, using `run_blocking`, with a `tracing::info!` line like the others (no content in logs).
- [ ] **Step 5: Mock snapshot.** In `sessions.rs` tests add `ui_context_data_matches_the_backend`: `{ "session": session_context(...), "calls": { <every model call id>: call_context(...) } }` written to `ui/src/mock/context-data.json` via `check_snapshot`. Generate it with the snapshot switch (Task 6 Step 3 command), then run tests without it.
- [ ] **Step 6: Run** fmt, clippy, all tests. Expected: PASS.
- [ ] **Step 7: Commit** "Serve context breakdowns for every model call".

---

### Task 8: UI: types, API, mock and the bar on model-call boxes

**Files:**
- Modify: `ui/src/types.ts`, `ui/src/api.ts`, `ui/src/mock/mockApi.ts`, `ui/src/components/nodes.tsx`, `ui/src/layout.ts` (+ `layout.test.ts` if heights change), `ui/src/components/Diagram.tsx`, `ui/src/App.tsx`, `ui/src/app.css`
- Create: `ui/src/context.ts`, `ui/src/context.test.ts`, `ui/src/components/ContextBar.tsx`

**Interfaces:**
- Produces:

```ts
// types.ts
export type SliceKind = 'system_prompt' | 'tool_definitions' | 'instructions' | 'files_read' | 'tool_results' | 'conversation' | 'not_captured'
export type ContextSource = 'captured' | 'transcript'
export interface ContextItem { label: string; tokens: number; count: number; largest_tokens: number }
export interface ContextSlice { kind: SliceKind; label: string; tokens: number; share: number; items: ContextItem[] }
export interface Advice { level: 'warn' | 'info'; slice: SliceKind; text: string }
export interface ContextBreakdown { source: ContextSource; total_tokens: number; total_is_reported: boolean; slices: ContextSlice[]; advice: Advice[] }
export interface BarSlice { kind: SliceKind; tokens: number }
export interface ContextBar { source: ContextSource; total_tokens: number; total_is_reported: boolean; slices: BarSlice[] }
export interface SessionContext { bars: Record<string, ContextBar>; latest: ContextBreakdown | null; latest_trace_id: string | null }

// api.ts (Api interface + tauriApi)
sessionContext(project: string, sessionId: string): Promise<SessionContext>   // invoke('session_context', { project, session_id })
callContext(project: string, sessionId: string, traceId: string): Promise<ContextBreakdown | null>  // invoke('call_context', { project, session_id, trace_id })

// context.ts
export const SLICE_NAMES: Record<SliceKind, string>   // same names as SliceKind::label in Rust
export interface Segment { kind: SliceKind; tokens: number; percent: number }
/** Segments for a bar: percent of the total, in the given order; slices under 0.5% are kept (the CSS gives them a minimum width of 1px). */
export function barSegments(bar: ContextBar): Segment[]
/** One tooltip line per slice: "Tool definitions: 41,234 tokens (38%)". */
export function barTooltip(bar: ContextBar): string
```

- [ ] **Step 1: Failing tests** in `context.test.ts`: `barSegments` percentages sum to 100 (within 0.01) for a three-slice bar; an empty bar gives `[]`; a zero total gives `[]`; `barTooltip` lines use `formatTokens` style grouping and whole percentages; a `not_captured` slice is named "Not captured".
- [ ] **Step 2:** `cd ui; npm test`. Expected: FAIL.
- [ ] **Step 3: Implement** `context.ts`, types and API. Mock: `sessionContext` returns `context-data.json`'s `session` for the fixture key (and `{ bars: {}, latest: null, latest_trace_id: null }` otherwise or after captures are deleted? No: transcript bars stay after deleting captures in the real app, but the mock cannot compute them, so return the stored value regardless); `callContext` returns `calls[traceId] ?? null`; honour `wait()` and add `fail=context` to the URL flags comment and behaviour.
- [ ] **Step 4: `ContextBar` component**: props `{ bar: ContextBar; height: number; label?: string }`; renders a `div role="img"` with `aria-label` = `barTooltip`, `title` = same text, one child span per segment with `width: {percent}%`, `min-width: 1px`, background `var(--slice-<kind>)`; `not_captured` uses a hatched `repeating-linear-gradient` of `var(--slice-not_captured)`. Define seven `--slice-*` colours in `app.css` on `:root` and a dark set under the existing dark theme selector (match how `app.css` already switches themes). Pick colours distinct from the existing status colours; check each against the box background for at least 3:1 contrast and note the check in the commit message.
- [ ] **Step 5: Boxes.** `BoxData` gains `context: ContextBar | null`. The model-call box renders `<ContextBar height={6} />` as its last row when `context` is non-null. If it does not fit in the current `NODE_HEIGHT.model_call` (80), raise it by 8 and update `layout.test.ts` expectations. `App.tsx` keeps `sessionContext` state loaded with `api.sessionContext` at the same moments and with the same token/throttle as `loadCaptureOverview` (fresh load on session open, throttled on live updates); `Diagram.tsx` passes `bars[node.id] ?? null` into each box's data. A failed load leaves bars empty and logs nothing to the UI (the session card in Task 9 shows the error).
- [ ] **Step 6: Run** `cd ui; npm run lint; npm run typecheck; npm test; npm run build`. Then `npm run dev`, open `http://localhost:5173/?mock`, open the fixture session and check that model-call boxes show bars. Take a screenshot for the task report.
- [ ] **Step 7: Commit** "Show a context bar on every model call box".

---

### Task 9: UI: details panel section and session Context card

**Files:**
- Create: `ui/src/components/ContextSection.tsx`
- Modify: `ui/src/components/DetailsPanel.tsx`, `ui/src/components/Sidebar.tsx`, `ui/src/App.tsx`, `ui/src/app.css`, `ui/src/context.ts` + `context.test.ts`

**Interfaces:**
- Consumes: Task 8 types, `ContextBar`, `api.callContext`, `SessionContext` state from `App.tsx`.
- Produces:

```ts
// context.ts
/** "Estimated split of 41,234 reported tokens" or "Estimated total: about 9,800 tokens" when not reported. */
export function totalNote(b: ContextBreakdown): string
/** The three largest slices, largest first. */
export function topSlices(b: ContextBreakdown, n: number): ContextSlice[]

// ContextSection.tsx
export function ContextDetails({ breakdown }: { breakdown: ContextBreakdown })  // full breakdown: bar, legend, items, advice, note
export function ContextCard(props: { context: Loadable<SessionContext>; onSelect: (traceId: string) => void; onRetry: () => void })
```

- [ ] **Step 1: Failing tests** for `totalNote` (reported and estimated) and `topSlices` (order, n larger than slices).
- [ ] **Step 2:** `npm test`. Expected: FAIL. Implement both. Expected: PASS.
- [ ] **Step 3: Details panel.** When the selected node is a model call, `DetailsPanel` loads `api.callContext(project, sessionId, node.id)` on open and whenever `refreshKey` changes (same pattern and stale-response guard it already uses for node detail) and renders a `Section title="Context"` with `ContextDetails` above the existing sections. `ContextDetails` shows: the full-width bar (height 12), a legend table (colour swatch, slice name, tokens, share %), each slice's items in a `<details>` element (label, tokens, `×count` when count > 1), the advice list (warn items with a warning icon and text, info items plain), and `totalNote`. No context: "No token counts for this call yet." Loading and error states match the panel's existing ones.
- [ ] **Step 4: Session Context card.** In `Sidebar.tsx`, above `CaptureOverviewSection`, render `ContextCard`: heading "Context", the latest breakdown's bar (height 10), `topSlices(b, 3)` as "Tool definitions 38%" lines, the advice list, `totalNote`, and a button "Show this call" that calls `onSelect(latest_trace_id)` (App selects that node like a box click). `latest` null: "No model calls with token counts yet." Error: message plus Retry.
- [ ] **Step 5: Run** all UI checks. Then `npm run dev`, `?mock`, open the fixture session: confirm the card renders with its bar, top slices and whatever advice the fixture triggers, and the details section renders for a model call. Screenshot both for the task report. Warn advice may not trigger on the small fixture; the Rust tests cover the rules.
- [ ] **Step 6: Commit** "Explain each call's context in the details panel and the session overview".

---

### Task 10 (coordinator, not a subagent): real-app check and docs

- [ ] Run `cargo tauri dev`, start a captured Claude Code session (`$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude`), do a short task that reads a file several times, and check in the app (WebView2 CDP port 9333): bars on boxes, details panel numbers against the raw request, card advice. Open one old transcript-only session too.
- [ ] `docs/VIEW-MODEL.md`: `SessionContext`, `ContextBreakdown`, `ContextBar`, slices, advice.
- [ ] `docs/DECISIONS.md`: the `insights` crate and why the rules are data; estimating by scaling characters to the reported total; 4 characters per token fallback; fixed image weight; advice thresholds; deferring transcript attachments to 5.1b.
- [ ] `docs/HARNESS-NOTES.md`: what a real request's context is made of (slice shares from the real check) and the transcript attachment finding (`prompt_snapshot`, `instructions`, `skill_listing`, `mcp_instructions_delta`; no tool definitions).
- [ ] `README.md` feature line and milestone status; `CLAUDE.md` current milestone (M5.1 status, and the commands list if a new one exists).
- [ ] Final whole-branch review on the most capable model, one fix wave, then the report with a "Rulings I made" list.
