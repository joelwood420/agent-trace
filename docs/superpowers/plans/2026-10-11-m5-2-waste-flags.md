# M5.2 Waste and Problem Flags Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flag repeated reads, failed tools retried the same way, loops, very large tool results and compactions (plus re-reads of files a compaction dropped), and show them as badges on diagram boxes, in the details panel and in a session overview card.

**Architecture:** A small additive `trace-core` change (`Marker.compaction`, `ContextPartKind::File`). The Claude Code adapter fills those from real compaction lines and supplies `FlagRules` as data. A new `insights::flags` module computes `Vec<Flag>` from a `Trace`, harness-neutral. The app serves them through a new `session_flags` command. The UI only maps flag trace ids to boxes.

**Tech Stack:** Rust stable (trace-core, insights, adapter-claude-code, src-tauri), React + TypeScript + React Flow (ui), node test runner for UI tests.

**Spec:** `docs/superpowers/specs/2026-10-11-m5-2-waste-flags-design.md`. Read it before starting any task.

## Global Constraints

- Every command must work in Windows PowerShell 5.1. No bash-only syntax in docs.
- No `unwrap()` or `expect()` outside tests. Return `Result` or skip and log.
- Unknown or malformed transcript fields are skipped and logged with `tracing`, never a panic.
- `trace-core` and `insights` know nothing about Claude Code. Claude Code names (`Read`, `file_path`, `compactMetadata`) appear only in `crates/adapter-claude-code`.
- Doc comments on all public types in `trace-core`.
- No em dashes anywhere (code, comments, docs, commit messages).
- Fixtures are invented: no real paths, prompts, file contents, usernames.
- No new dependencies.
- Commit in small steps with messages written for a public reader, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- Before reporting a task done run: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and for UI tasks `cd ui; npm run lint; npm run typecheck; npm test; npm run build`.
- Thresholds: `REPEAT_READ_WARN = 3`, `LOOP_WARN = 3`, `LARGE_RESULT_TOKENS = 10_000`.

## Review Focus

1. A live session: a compaction marker is re-sent with more paths as `file` lines arrive. The same marker id must replace, not duplicate, and flags recomputed later must reflect the final lists. (Task 4 test: re-sent marker has one id and full lists.)
2. A subagent that reads the same file as the main agent: counts must stay per run, never summed across runs. (Task 2 test `runs_are_checked_separately`.)
3. A trace id that sits in both a summary box and its child box: the jump must open the summary and select the deepest box, and the badge must not count the same flag twice on screen. (Task 6 tests on `badgeCount` and `revealPath`.)
4. Tool calls still running (no result): never a failure, no size, but still count as reads and loop steps. (Task 2 and Task 3 tests with `result: None`.)
5. A compaction marker with no numbers and no files (older or odd transcripts): the flag must still appear with a plain sentence, not empty text. (Task 3 test `compaction_without_details`.)

---

### Task 1: Schema additions in `trace-core`

**Files:**
- Modify: `crates/trace-core/src/event.rs` (Marker at about line 136, ContextPartKind at about line 179)
- Modify: `crates/trace-core/src/lib.rs` (re-export `Compaction`)
- Modify every `Marker { .. }` construction: `crates/adapter-claude-code/src/parser.rs:721`, `crates/insights/src/transcript.rs:275` and `:424`, `crates/trace-core/tests/example_trace.rs:141`, `crates/trace-view/tests/common/mod.rs:180` (add `compaction: None`)
- Modify every exhaustive `match` on `ContextPartKind` (for example `crates/adapter-claude-code/tests/basic_session.rs:350`) to handle `File`
- Modify: `docs/SCHEMA.md` (marker table, context part kind sentence), `docs/DECISIONS.md`
- Test: `crates/trace-core/src/event.rs` tests module (or `crates/trace-core/tests/`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Marker { pub kind: String, pub summary: String,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub compaction: Option<Compaction> }
  #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
  pub struct Compaction {
      #[serde(default, skip_serializing_if = "Option::is_none")] pub tokens_before: Option<u64>,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub tokens_after: Option<u64>,
      #[serde(default, skip_serializing_if = "Vec::is_empty")] pub files_restored: Vec<String>,
      #[serde(default, skip_serializing_if = "Vec::is_empty")] pub files_referenced: Vec<String>,
  }
  ContextPartKind::File   // serialised "file"
  ```

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn marker_compaction_round_trips() {
    let m = Marker {
        kind: "compaction".into(),
        summary: "Context compacted".into(),
        compaction: Some(Compaction {
            tokens_before: Some(380_000),
            tokens_after: Some(19_000),
            files_restored: vec!["src/a.rs".into()],
            files_referenced: vec!["src/b.rs".into(), "notes.txt".into()],
        }),
    };
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["compaction"]["tokens_before"], 380_000);
    assert_eq!(json["compaction"]["files_referenced"][1], "notes.txt");
    let back: Marker = serde_json::from_value(json).unwrap();
    assert_eq!(back, m);
}

#[test]
fn marker_without_compaction_reads_and_writes_as_before() {
    let old = serde_json::json!({"kind": "hook", "summary": "Stop hooks ran (1)"});
    let m: Marker = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(m.compaction, None);
    assert_eq!(serde_json::to_value(&m).unwrap(), old);
}

#[test]
fn empty_compaction_details_serialise_as_an_empty_object() {
    let c = Compaction::default();
    assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({}));
}

#[test]
fn file_part_kind_is_a_plain_string() {
    assert_eq!(serde_json::to_value(ContextPartKind::File).unwrap(), "file");
    let k: ContextPartKind = serde_json::from_value(serde_json::json!("file")).unwrap();
    assert_eq!(k, ContextPartKind::File);
}
```

- [ ] **Step 2: Run** `cargo test -p trace-core`. Expected: compile errors (no `Compaction`, no `File`).
- [ ] **Step 3: Implement.** Add the struct and field with doc comments (`Compaction`: "What a compaction kept and dropped, when the harness reports it. Only markers of kind `compaction` carry it."; each field documented: context size in tokens just before / just after; paths re-attached with their content; paths named without their content). Add `File` with doc "A file's content that the harness put into the context without a tool call, for example re-attached after a compaction. The label is the file path." Add `"file"` to the hand-written `Serialize` and `Deserialize` impls. Re-export `Compaction` from `lib.rs` next to `Marker`. Add `compaction: None` at every `Marker` construction and `File` arms to exhaustive matches (in `basic_session.rs` map it to `"file"`).
- [ ] **Step 4: Run** `cargo test --workspace` and clippy. Expected: all pass.
- [ ] **Step 5: Docs.** In `docs/SCHEMA.md` `marker` table add row: `compaction` | object | no | Only for kind `"compaction"`: `{ "tokens_before"?: int, "tokens_after"?: int, "files_restored"?: [string], "files_referenced"?: [string] }`. Context size before and after, files put back with their content, and files only named. Leave out what is unknown. Add `"file"` (a file's content placed in context without a tool call; the label is its path) to the kind list. In `docs/DECISIONS.md` add an entry "M5.2: compaction details and file parts in the schema" with the reasons from the spec (harness-neutral flags; the owner's harness will compact too; additive, old traces read unchanged).
- [ ] **Step 6: Commit** `git commit -m "Add compaction details to markers and a file context part kind"`

---

### Task 2: `insights` flags, part 1 (types, rules, file parts, repeated reads, large results)

**Files:**
- Create: `crates/insights/src/flags.rs`
- Modify: `crates/insights/src/rules.rs` (add `FlagRules`, `FileEditRule`)
- Modify: `crates/insights/src/lib.rs` (module and re-exports)
- Modify: `crates/insights/src/transcript.rs` (`RunState::apply`: `File` parts go to `SliceKind::FilesRead`; make `block_chars` `pub(crate)`)
- Modify: `crates/insights/src/advice.rs` (make `plural` `pub(crate)`)

**Interfaces:**
- Consumes: `trace_core::{Trace, Node, ToolCall, Marker, Compaction, ContextPartKind::File}`, `insights::advice::{AdviceLevel, about_tokens}`, `crate::breakdown::CHARS_PER_TOKEN` (f64, 4.0), `crate::rules::FileReadRule { tool, path_field }`.
- Produces:
  ```rust
  // rules.rs
  #[derive(Debug, Clone, PartialEq, Default)]
  pub struct FlagRules { pub file_reads: Vec<FileReadRule>, pub range_fields: Vec<String>, pub file_edits: Vec<FileEditRule> }
  #[derive(Debug, Clone, PartialEq)]
  pub struct FileEditRule { pub tool: String, pub path_fields: Vec<String> } // first present string field wins
  // flags.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
  pub enum FlagKind { RepeatedRead, FailedRetry, Loop, LargeResult, Compaction, RereadAfterCompaction }
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct Flag { pub kind: FlagKind, pub level: AdviceLevel, pub text: String, pub trace_ids: Vec<String>, pub run_id: String }
  pub const REPEAT_READ_WARN: usize = 3;
  pub const LOOP_WARN: usize = 3;
  pub const LARGE_RESULT_TOKENS: u64 = 10_000;
  pub fn session_flags(trace: &Trace, rules: &FlagRules) -> Vec<Flag>;
  ```
  lib.rs: `pub mod flags; pub use flags::{Flag, FlagKind, session_flags, REPEAT_READ_WARN, LOOP_WARN, LARGE_RESULT_TOKENS}; pub use rules::{FlagRules, FileEditRule};`

**Design of `flags.rs`** (Task 3 adds rules to the same walk):

1. `order`: one pre-order walk over the whole trace from `trace.roots()` assigning an index to every node id (`HashMap<String, usize>`). Used to sort flags by their first trace id.
2. For each run (roots that are `Node::Run`, then nested runs found under tool calls, the same queue pattern as `measure_transcript_all`), walk its subtree depth first without entering nested `Run` nodes (push them to the queue), producing `Vec<Step>`:
   ```rust
   enum Step<'a> {
       Tool { id: &'a str, turn: Option<&'a str>, call: &'a ToolCall },
       Compaction { id: &'a str, details: Option<&'a Compaction> }, // Marker with kind == "compaction"
   }
   ```
   `turn` is the id of the nearest `Turn` ancestor inside this run (track it while walking).
3. Helper functions:
   ```rust
   fn read_path<'a>(call: &'a ToolCall, rules: &FlagRules) -> Option<&'a str>   // file_reads rule for call.name, input[path_field] as str
   fn edit_path<'a>(call: &'a ToolCall, rules: &FlagRules) -> Option<&'a str>   // file_edits rule, first path_fields entry that is a string
   fn range_key(call: &ToolCall, rules: &FlagRules) -> String                   // compact JSON of an object holding only the range fields present, "" if none
   fn failed(call: &ToolCall) -> bool { call.result.as_ref().is_some_and(|r| r.is_error) }
   fn result_tokens(call: &ToolCall) -> Option<u64>                             // sum of transcript::block_chars over result content, divided by CHARS_PER_TOKEN, rounded down
   ```
4. Rules in this task:
   - `repeated_read`: `HashMap<(String path, String range), Vec<&str id>>` of open series. For each `Tool` step in order: if `edit_path` is `Some(p)`, close every series whose path is `p`. Else if `read_path` is `Some(p)` and the call has not failed, push its id to the series for `(p, range_key)`. A `Compaction` step closes every series. At the end close all. Closing a series of length `>= REPEAT_READ_WARN` emits `Flag { kind: RepeatedRead, level: Warn, text: format!("`{p}` read {n} times with no edit in between."), trace_ids: series, run_id }`. Close series in a stable order (sort by first id's order index) so output is deterministic.
   - `large_result`: for each `Tool` step with `result_tokens >= Some(LARGE_RESULT_TOKENS)`: `Flag { kind: LargeResult, level: Warn, text: format!("`{name}` returned {} tokens.", about_tokens(t)), trace_ids: vec![id] }`.
5. Sort all flags by `order[trace_ids[0]]`, then by kind order, then text.

- [ ] **Step 1: Write failing tests** in `flags.rs` `#[cfg(test)] mod tests`, using a builder like `transcript.rs` tests (`add`, `run()`, `call()`), with rules:

```rust
fn rules() -> FlagRules {
    FlagRules {
        file_reads: vec![FileReadRule { tool: "ReadFile".into(), path_field: "path".into() }],
        range_fields: vec!["from".into()],
        file_edits: vec![FileEditRule { tool: "EditFile".into(), path_fields: vec!["path".into()] }],
    }
}
fn tool(name: &str, input: Value, result: Option<(&str, bool)>) -> Node {
    Node::ToolCall(ToolCall {
        name: name.into(),
        input,
        result: result.map(|(t, e)| ToolResult { content: vec![ContentBlock::Text { text: t.into() }], is_error: e }),
    })
}
```

Tests (each builds R > T1 > M1..Mn with one tool call under each model call unless stated):
- `three_reads_of_one_file_are_flagged`: 3 x `ReadFile {"path":"a.rs"}` ok. Expect one `RepeatedRead`, `trace_ids` = the 3 tool ids, text "`a.rs` read 3 times with no edit in between.", level Warn, run_id "R".
- `two_reads_are_not_flagged`.
- `an_edit_between_reads_restarts_the_count`: read, read, `EditFile a.rs`, read, read: no flag.
- `an_edit_of_another_file_does_not`: read a, read a, edit b.rs, read a: one flag with 3 ids.
- `different_ranges_are_different_reads`: `{"path":"a.rs","from":1}` x2 and `{"path":"a.rs","from":50}` x2: no flag; `{"path":"a.rs","from":1}` x3: flag.
- `failed_reads_do_not_count`: 3 reads, the second with `is_error: true`: no flag.
- `running_reads_count`: 3 reads, the last with `result: None`: one flag.
- `a_compaction_restarts_the_count`: read, read, Marker compaction, read: no `RepeatedRead`.
- `runs_are_checked_separately`: main run reads a.rs twice; one main tool call has a child `Run` "S" whose model call reads a.rs twice: no flag.
- `large_result_is_flagged`: result text of `"x".repeat(40_000)`: one `LargeResult` with text "`ReadFile` returned about 10k tokens." Also a 39_996-char result: no flag.
- `pending_result_has_no_size`: `result: None`: no `LargeResult`.
- `flags_are_in_session_order`: a large result in M1 and a repeated read starting at M2: `LargeResult` first.
- In `transcript.rs` tests: `file_parts_count_as_files_read`: a `ContextUpdate` with `ContextPart { key: "file:a.rs", kind: File, label: "a.rs", text: 10 chars }` before a model call; that call's measure has `FilesRead` item "a.rs" with 10 chars, and a later `ReadFile a.rs` with 40 chars merges into the same item (50 chars).

- [ ] **Step 2: Run** `cargo test -p insights`. Expected: compile failure.
- [ ] **Step 3: Implement** the rules types, `flags.rs` as designed, and in `RunState::apply` map `ContextPartKind::File => SliceKind::FilesRead` (label stays the path). Check that `ContextMeasure::add_part` with `FilesRead` and the same label merges with `add(FilesRead, path, ..)`; if it does not, merge by label in the snapshot.
- [ ] **Step 4: Run** tests and clippy. Expected: pass.
- [ ] **Step 5: Commit** `git commit -m "Flag repeated file reads and very large tool results"`

---

### Task 3: `insights` flags, part 2 (failed retries, loops, compaction, re-reads after compaction)

**Files:**
- Modify: `crates/insights/src/flags.rs`

**Interfaces:**
- Consumes: everything from Task 2 (`Step`, helpers, `Flag`, `FlagKind`, constants).
- Produces: no new public names; `session_flags` now returns all six kinds.

**Rules:**

- `failed_retry`: key `(name, input: &Value)`; compare inputs with `Value` equality (keep a `Vec<(key, series)>` and search linearly, since `Value` is not `Hash`). Walk `Tool` steps in order, skipping calls with no result. If an open series has an equal key, push the id; if this call succeeded, close that series. Else if this call failed, open a series with its id. At the end close all. Closing a series of length `>= 2` emits:
  - last call failed: level Warn, text `"`{name}` failed and was retried {r} time(s) with the same input; it failed again."` when `r == 1`, `"...; it failed every time."` when `r >= 2` (r = len - 1, with `plural(r, "time")` from advice).
  - last call succeeded: level Info, text `"`{name}` failed and was retried {plural(r,"time")} with the same input; the last try worked."`
- `loop`: per turn. Skip steps where `read_path` or `edit_path` is `Some` (reads are covered by `repeated_read`; edits are not loop candidates). Any edit step closes every open loop series. A change of `turn` closes every open series. A `Compaction` step closes every series. Series key `(name, input)` with `Value` equality. Closing a series with `len >= LOOP_WARN` and no failed call emits level Warn, text `"The same `{name}` call ran {n} times in this prompt with no file edit in between."`.
- `compaction` and `reread_after_compaction`: keep `in_context: Vec<String>` (distinct paths read by a non-failed read since the last compaction, plus that compaction's `files_restored`) and `dropped: Vec<String>`.
  - On a non-failed read of `p`: if `p` is in `dropped`, emit `RereadAfterCompaction` (Warn, text "`{p}` was read again after compaction dropped it.", `trace_ids: [id]`) and remove `p` from `dropped`. Add `p` to `in_context` if missing.
  - On a `Compaction` step with details `d` (use `Compaction::default()` when `None`): `restored = d.files_restored`, `referenced = d.files_referenced`. `not_mentioned = in_context` paths in neither list. `dropped = (dropped ∪ in_context) − restored`. `in_context = restored`. Emit `Compaction` (Info, `trace_ids: [marker id]`) with text joined by single spaces from these sentences, leaving out the ones that do not apply:
    1. both numbers: `"Compaction dropped {about_tokens(before - after, saturating)} tokens (kept {about_tokens(after)})."`; only before: `"Compaction ran at {about_tokens(before)} tokens."`; otherwise `"Context was compacted."`
    2. restored and referenced both non-empty: `"Re-attached {plural(r,"file")} in full and {f} by name only."`; only restored: `"Re-attached {plural(r,"file")} in full."`; only referenced: `"Re-attached {plural(f,"file")} by name only."`
    3. `not_mentioned` non-empty: `"{plural(n,"file")} read earlier {was|were} not re-attached."`
  - Note `about_tokens` already returns "about 9k" for >= 1000, so the sentence reads "dropped about 361k tokens (kept about 19k)".

- [ ] **Step 1: Write failing tests** (same builders as Task 2; a compaction marker is `Node::Marker(Marker { kind: "compaction".into(), summary: String::new(), compaction: Some(..) })`):
- `failed_then_same_failure_is_a_warn`: `Bash {"command":"make"}` fails twice. One `FailedRetry`, Warn, 2 ids, text ends "it failed again."
- `failed_three_times_says_every_time`: 3 failures: text "`Bash` failed and was retried 2 times with the same input; it failed every time."
- `retry_that_works_is_info`: fail, ok: Info, text ends "the last try worked."; a following identical ok call is not added (series closed).
- `different_input_is_not_a_retry`: fail `make`, ok `make test`: none.
- `inputs_compare_as_json_values`: `{"a":1,"b":2}` fails then `{"b":2,"a":1}` fails: flagged (build the second with `serde_json::from_str` of the reordered text).
- `pending_calls_are_ignored_by_retries`: fail, `None`: no flag.
- `same_call_three_times_is_a_loop`: 3 x ok `Bash {"command":"ls"}` in one turn: one `Loop` with 3 ids.
- `loop_needs_one_turn`: 2 in T1 and 1 in T2: none.
- `an_edit_breaks_a_loop`: ls, ls, `EditFile x`, ls: none.
- `a_loop_with_a_failure_is_not_a_loop`: ok, fail, ok of the same call: no `Loop` (and one `FailedRetry`, Info).
- `reads_are_not_loops`: 3 identical `ReadFile`: only `RepeatedRead`, no `Loop`.
- `compaction_flag_text`: reads of `a.rs`, `b.rs`, `c.rs`, then a compaction `{before: 380_389, after: 19_037, restored: ["a.rs"], referenced: ["b.rs"]}`. Text exactly "Compaction dropped about 361k tokens (kept about 19k). Re-attached 1 file in full and 1 by name only. 1 file read earlier was not re-attached." Level Info, `trace_ids` = [marker id].
- `compaction_without_details`: a compaction marker with `compaction: None` and no earlier reads: text "Context was compacted."
- `reread_after_compaction_is_flagged_once`: same setup; then read `b.rs`, read `c.rs`, read `a.rs`, read `b.rs`. Exactly two `RereadAfterCompaction` flags (the first `b.rs` and the `c.rs` reads), none for `a.rs` (restored) or the second `b.rs`.
- `dropped_files_carry_over_a_second_compaction`: read a, compaction (restored none), compaction (restored none), read a: one `RereadAfterCompaction`.

- [ ] **Step 2: Run** `cargo test -p insights`. Expected: the new tests fail.
- [ ] **Step 3: Implement** the four rules in the same per-run pass.
- [ ] **Step 4: Run** tests and clippy. Expected: pass.
- [ ] **Step 5: Commit** `git commit -m "Flag failed retries, loops, compactions and re-reads after compaction"`

---

### Task 4: Adapter: compaction details, file attachments, flag rules, compaction fixture

**Files:**
- Modify: `crates/adapter-claude-code/src/parser.rs` (`on_system` compact_boundary arm, `on_attachment`, `context_from_attachment`, `drop_reminders`, `marker`; new parser fields)
- Modify: `crates/adapter-claude-code/src/context_parts.rs` (`file` and `invoked_skills` mapping, `FILE_PREFIX`)
- Modify: `crates/adapter-claude-code/src/context.rs` (add `pub fn flag_rules() -> FlagRules`)
- Modify: `crates/adapter-claude-code/src/lib.rs` (re-export `flag_rules`)
- Create: `fixtures/claude-code/compaction/00000000-0000-4000-8000-000000000003.jsonl`
- Create: `crates/adapter-claude-code/tests/compaction_session.rs`

**Interfaces:**
- Consumes: `trace_core::{Compaction, ContextPartKind::File}` (Task 1), `insights::{FlagRules, FileEditRule, FileReadRule}` (Task 2).
- Produces: `adapter_claude_code::flag_rules() -> insights::FlagRules`; fixture path `fixtures/claude-code/compaction/00000000-0000-4000-8000-000000000003.jsonl` (project folder `compaction`, session id `00000000-0000-4000-8000-000000000003`).

**Behaviour:**
- `flag_rules()`: `file_reads: [Read/file_path]`, `range_fields: ["offset", "limit"]`, `file_edits: [Edit: [file_path], Write: [file_path], NotebookEdit: [notebook_path, file_path]]`. Doc comment says these are Claude Code's built-in tool names.
- `compact_boundary`: build `Compaction { tokens_before: compactMetadata.preTokens as u64, tokens_after: postTokens, .. }` and pass it to the marker (give `marker` a `compaction: Option<Compaction>` parameter, or add a `compaction_marker` fn). Store `self.open_compaction = Some((marker event id, Compaction))`. Also `drop_reminders` must now also remove file part keys (`file:` prefix). Track them in `self.reminder_keys` too (rename the field to `dropped_at_compaction` or keep a second `file_keys` vec; either way both are removed).
- `open_compaction` is cleared when a new prompt (turn) or a model call starts (find where turns and model calls are created: `new_model_call` caller and the user-prompt handler).
- `file` attachment: path = `content.file.filePath`, else `filename`; text = `content.file.content`. Part `{ key: "file:<path>", kind: File, label: <path>, text }`. No path or no text: skip and log at debug. If `open_compaction` is set, push the path to `files_restored` (if not there) and re-emit the marker event (same id, updated `Node::Marker`; take the stored node from `self.nodes`, change `compaction`, append the line's raw to `raw`, `emit`).
- `compact_file_reference`: no context part. Path = `filename`, else `displayPath`. If `open_compaction` is set, push to `files_referenced` and re-emit the marker. Otherwise ignore.
- `invoked_skills`: one reminder part labelled `invoked skills`, text = each skill's `content` joined with `"\n\n"` (skip non-string). No skills: no part.
- Every Claude Code name stays in this crate.

**Fixture** (invented; model each line on the existing `fixtures/claude-code/basic/*.jsonl` lines for field names such as `uuid`, `parentUuid`, `sessionId`, `timestamp`, `cwd` using an invented `C:\\work\\demo` path, `message`):
1. A user prompt "Tidy up the calculator module."
2. Assistant `tool_use` `Read {"file_path":"C:\\work\\demo\\src\\calc.py"}` and its `tool_result` (short invented text). Repeat the same Read twice more (three reads, no edit: a `repeated_read`).
3. Assistant `Read` of `C:\\work\\demo\\src\\main.py` and `C:\\work\\demo\\NOTES.txt` (ok results).
4. Assistant `Bash {"command":"python -m pytest"}` with `is_error: true` result, then the same call again failing (a `failed_retry`).
5. Assistant `Bash {"command":"git status"}` three times in a row, ok (a `loop`).
6. Assistant `WebFetch {"url":"https://example.com/doc"}` with a result of 40,100 characters of repeated invented text (a `large_result`).
7. A second prompt `/compact` in the real shape: `system` `compact_boundary` with `compactMetadata` `{ "trigger": "manual", "preTokens": 120500, "postTokens": 8200, "cumulativeDroppedTokens": 112300, "durationMs": 20000, "preservedSegment": {...invented uuids}, "preservedMessages": {"anchorUuid": "...", "uuids": [], "allUuids": []} }`, then the `isCompactSummary` user line, the three local command lines, then attachments: `compact_file_reference` for `NOTES.txt`, `file` for `calc.py` with content, `compact_file_reference` for `main.py`, `invoked_skills` with one invented skill.
8. A third prompt; assistant reads `C:\\work\\demo\\src\\main.py` (a `reread_after_compaction`) and finishes with `end_turn`.

**Tests** (`tests/compaction_session.rs`, reading the fixture through the same helper `basic_session.rs` uses):
- `compaction_marker_has_details`: exactly one marker of kind `compaction`; `tokens_before == Some(120500)`, `tokens_after == Some(8200)`, `files_restored == [calc.py path]`, `files_referenced == [NOTES.txt path, main.py path]`.
- `re_sent_marker_keeps_one_id`: parse the fixture as a stream of events (whatever `basic_session.rs` uses for live parsing, for example feeding lines one by one); the compaction marker id appears more than once in the event stream, the final trace holds one marker node, and its lists are complete.
- `restored_file_is_a_file_part`: a `context_update` holds a `File` part with key `file:<calc.py path>`, label the path and the invented content.
- `file_parts_are_removed_at_the_next_compaction`: unit test in `parser.rs` with two boundaries: the second boundary's removal update lists the `file:` key.
- `invoked_skills_is_a_reminder`: a reminder part labelled `invoked skills` with the skill content.
- `fixture_flags`: `insights::session_flags(&trace, &flag_rules())` returns exactly the kinds `[RepeatedRead, FailedRetry, Loop, LargeResult, Compaction, RereadAfterCompaction]` in that order. The `Compaction` text is "Compaction dropped about 112k tokens (kept about 8k). Re-attached 1 file in full and 2 by name only." (all three earlier-read files are mentioned, so no third sentence).
- `print_tree` still runs on the fixture with `invalid=0 skipped=0`: run `cargo run -p adapter-claude-code --example print_tree -- fixtures\claude-code\compaction\00000000-0000-4000-8000-000000000003.jsonl --stats`. Any line it skips: fix the fixture or the parser.

- [ ] **Step 1:** Write the fixture.
- [ ] **Step 2:** Write the failing tests. Run `cargo test -p adapter-claude-code`. Expected: fail.
- [ ] **Step 3:** Implement the behaviour above.
- [ ] **Step 4:** Run all tests, clippy, `print_tree --stats`. Check that the existing basic fixture tests and other tests that count sessions or projects under `fixtures/claude-code` (search `src-tauri` for `list_sessions(&root)` assertions) still pass; update counts where a new project folder changes them.
- [ ] **Step 5: Commit** in two steps: `"Read compaction details and re-attached files from Claude Code transcripts"` and `"Add an invented compaction fixture that triggers every flag"`.

---

### Task 5: App command `session_flags` and mock data

**Files:**
- Create: `src-tauri/src/flags.rs` (view type and function)
- Modify: `src-tauri/src/main.rs` (`mod flags;`, command, `generate_handler!`)
- Modify: `src-tauri/src/capture_sink.rs` (add `session_flags_view` next to `session_context_view`)
- Modify: `src-tauri/src/sessions.rs` tests (mock snapshot for flags; include the compaction session in `fixture-data.json`)
- Create (generated): `ui/src/mock/flags-data.json`
- Modify: `docs/VIEW-MODEL.md` (new "Flags (M5.2)" section)

**Interfaces:**
- Consumes: `insights::{session_flags, Flag}`, `adapter_claude_code::flag_rules`, existing `copy_from_trace(root, shared, project, session_id, f)` and `session_path` in `capture_sink.rs`.
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Serialize)]
  pub struct SessionFlags { pub flags: Vec<insights::Flag> }
  pub fn session_flags(trace: &Trace) -> SessionFlags          // src-tauri/src/flags.rs
  pub fn session_flags_view(root: &Path, shared: &Shared, project: &str, session_id: &str) -> Result<SessionFlags, SessionError>
  #[tauri::command(rename_all = "snake_case")] async fn session_flags(paths, shared, project: String, session_id: String) -> Result<SessionFlags, String>
  ```
  JSON: `{ "flags": [ { "kind": "repeated_read", "level": "warn", "text": "...", "trace_ids": ["tool:..."], "run_id": "..." } ] }`.

- [ ] **Step 1: Write failing tests** in `src-tauri/src/flags.rs`:
  - `fixture_session_has_every_flag_kind`: load the compaction fixture with `load_trace(&fixture_root(), "compaction", "00000000-0000-4000-8000-000000000003")`, call `session_flags`, assert the six kinds.
  - `basic_fixture_flags_are_stable`: same for the basic fixture, asserting only that every `trace_ids` entry exists in the trace (`trace.get(id).is_some()`).
  - In `sessions.rs` tests add `ui_flags_data_matches_the_backend`: `{ "<project>/<session>": SessionFlags }` for both fixture sessions, checked with `check_snapshot(&mock_file("flags-data.json"), ..)`. Extend `ui_mock_data_matches_the_commands` so `views` and `details` also include the compaction session.
- [ ] **Step 2: Run** `cargo test -p snitchcraft`. Expected: fail.
- [ ] **Step 3: Implement.** The command mirrors `session_context` (use `run_blocking`, `tracing::info!(flags = view.flags.len(), "loaded session flags")`). Register it in `generate_handler!`. No new Tauri permission is needed (commands are app-internal); confirm by checking `src-tauri/capabilities/` for how existing commands are allowed and follow the same pattern if they are listed.
- [ ] **Step 4: Regenerate mock data**: `$env:SNITCHCRAFT_UPDATE_SNAPSHOTS='1'; cargo test -p snitchcraft; Remove-Item Env:SNITCHCRAFT_UPDATE_SNAPSHOTS`, then `cargo test -p snitchcraft` again without the variable. Review the diff of every file in `ui/src/mock/`: only additions for the compaction session and the new flags file (plus the known key-order noise in `crates/trace-view/tests/snapshots/example-trace.diagram.json`; revert that file if it is only key order). Check the new mock JSON for anything not invented.
- [ ] **Step 5: Docs.** `docs/VIEW-MODEL.md`: section "Flags (M5.2)" with the `SessionFlags` and `Flag` field tables, the kind list, and the rule that the UI places a flag on every box whose `trace_ids` include one of the flag's `trace_ids`.
- [ ] **Step 6: Commit** `"Serve waste and problem flags to the UI"`.

---

### Task 6: UI: badges, details section, overview card, jump with reveal

**Files:**
- Create: `ui/src/flags.ts`, `ui/src/flags.test.ts`, `ui/src/components/FlagsCard.tsx`
- Modify: `ui/src/types.ts` (`Flag`, `FlagKind`, `SessionFlags`; add `'file'` to `ContextPartKind`; `compaction?` on the marker `TraceNode`)
- Modify: `ui/src/api.ts` (`sessionFlags`), `ui/src/mock/mockApi.ts` (serve `flags-data.json`, and the compaction session from `fixture-data.json`; add `fail === 'flags'`)
- Modify: `ui/src/App.tsx` (load flags with the context in `loadCaptures`; reveal on jump; pass flags down)
- Modify: `ui/src/components/Diagram.tsx`, `ui/src/components/nodes.tsx` (badge), `ui/src/components/DetailsPanel.tsx` (Flags section), `ui/src/components/Sidebar.tsx` (FlagsCard in the session panel), `ui/src/app.css`

**Interfaces:**
- Consumes: `session_flags` command JSON from Task 5; existing `DiagramNode { id, kind, trace_ids, children }`, `openState: Map<string, boolean>`, `collapsed_by_default`.
- Produces (in `flags.ts`):
  ```ts
  export type FlagIndex = Map<string, Flag[]>                       // trace id -> flags involving it
  export function indexFlags(flags: Flag[]): FlagIndex
  export function isOpen(node: DiagramNode, openState: Map<string, boolean>): boolean   // openState.get(id) ?? !collapsed_by_default
  export function boxFlags(node: DiagramNode, index: FlagIndex, open: boolean): Flag[] // distinct flags for the badge
  export function revealPath(root: DiagramNode, traceId: string): DiagramNode[] | null  // root..deepest box holding traceId
  export function flagsForNode(node: DiagramNode, index: FlagIndex): Flag[]           // distinct flags touching node.trace_ids
  ```

**Rules for `boxFlags`:**
- A box with no children, or open: distinct flags over its own `trace_ids`, minus flags already shown by a visible descendant (any flag whose trace ids intersect the `trace_ids` of a child subtree that will be drawn). Simplest correct form: own flags minus the union of `boxFlags` of open-visible children computed recursively. In practice: for an open box, a flag counts only if none of its trace ids is in any child's subtree.
- A collapsed box: distinct flags over every trace id in its whole subtree.
- Distinct means by object identity of the `Flag` (each flag is one object in the fetched array).

**Rules for `revealPath`:** depth-first; return the path to the deepest box whose `trace_ids` include `traceId` (a summary box and its child both match: return the path through the summary to the child). `null` if none.

- [ ] **Step 1: Write failing tests** in `flags.test.ts` (node test runner, same style as `live.test.ts`):
  - `indexFlags maps every trace id`: two flags sharing one id: that id maps to both.
  - `leaf box shows its own flags`.
  - `open box does not repeat a child's flag`: a model call box (trace id m1) open with child tool box (t1); flag on t1: model box shows 0, tool box shows 1.
  - `collapsed box sums its subtree once`: summary box with `trace_ids: [t1, t2, t3]` and children for each; one flag over [t1, t2, t3] and one over [t2]: collapsed summary shows 2 (not 4).
  - `open summary shows nothing of its own`: same, open: 0 on the summary.
  - `revealPath goes through a summary to the deepest box`.
  - `revealPath returns null for an unknown id`.
  - `flagsForNode lists the flags touching a box`.
  Run `cd ui; npm test`: fail.
- [ ] **Step 2: Implement `flags.ts`.** Run tests: pass.
- [ ] **Step 3: Types and API.** `types.ts`: `export type FlagKind = 'repeated_read' | 'failed_retry' | 'loop' | 'large_result' | 'compaction' | 'reread_after_compaction'`; `export interface Flag { kind: FlagKind; level: 'warn' | 'info'; text: string; trace_ids: string[]; run_id: string }`; `export interface SessionFlags { flags: Flag[] }`. `api.ts`: `sessionFlags(project, sessionId): Promise<SessionFlags>` invoking `'session_flags'` with `{ project, session_id }`. `mockApi.ts`: import `flags-data.json`, return the entry for `${project}/${sessionId}` or `{ flags: [] }`; make the compaction session loadable from `fixture-data.json` the same way the basic one is.
- [ ] **Step 4: App wiring.** In `App.tsx` add `const [sessionFlags, setSessionFlags] = useState<Loadable<SessionFlags>>({ status: 'loading' })`, fetched inside `loadCaptures` next to `sessionContext` with the same token guard, the same `fresh` loading behaviour and the same live throttle; reset in `closeSession`. `flagIndex = useMemo(() => indexFlags(...))`. Change `selectTrace` to use `revealPath`: set the prompt index, set `openState` to `true` for every box on the path except the last, then `selectNode(last)`. This also fixes "Show this call" for calls inside collapsed boxes.
- [ ] **Step 5: Badge.** Pass `flagIndex` and `openState` into `Diagram`, compute `boxFlags(node, flagIndex, isOpen(node, openState))` per box into `BoxData.flags: Flag[]`. In `nodes.tsx` `Shell` and `ParallelGroupNode` header, render when `flags.length > 0`: `<span className={`flag-badge flag-${anyWarn ? 'warn' : 'info'}`} title={flags.map(f => f.text).join('\n')} aria-label={`${flags.length} flag(s)`}>⚑ {flags.length}</span>`. CSS: small pill, warn uses the existing error/warn colour tokens, info a muted one; both themes.
- [ ] **Step 6: Details panel.** `DetailsPanel` gets `flags: Flag[]` (from `flagsForNode(selected, flagIndex)`); when non-empty, render a "Flags" section at the top: one line per flag, warn/info styling, text only.
- [ ] **Step 7: Overview card.** `FlagsCard.tsx`: props `flags: Loadable<SessionFlags>`, `onSelect(traceId: string)`. Loading: "Looking for problems...". Error: the message. Empty: "No problems found." Otherwise group by kind in the kind order above with headings ("Repeated reads", "Failed retries", "Loops", "Large tool results", "Compactions", "Re-reads after compaction") and counts; each flag a button with its text that calls `onSelect(flag.trace_ids[0])`. Place it in `Sidebar.tsx` `SessionPanel` directly above the Context card; wire `onSelect` to `onSelectTrace`.
- [ ] **Step 8: Checks.** `cd ui; npm run lint; npm run typecheck; npm test; npm run build`. Then `npm run dev` and open `http://localhost:5173/?mock`: open the compaction sample session, confirm badges on the expected boxes, the card lists six kinds, clicking each entry selects its box (opening the summary or group around it), the details panel shows the flag text. Also `?mock&fail=flags` (check how `fail` is read in `mockApi.ts`) shows the error in the card without breaking the rest.
- [ ] **Step 9: Commit** in two steps: `"Add flag helpers for badges and jumps"` and `"Show flags on boxes, in the details panel and in a session card"`.

---

### Task 7 (coordinator): real-app check and docs

Done by the coordinator, not a subagent.

- [ ] Run `cargo tauri dev`, open the real compacted session and one long session over WebView2 CDP (see memory `real-app-check`). Check badges, the card, jumps into collapsed boxes and subagents, and that each flag's text matches the boxes it marks. Note timings of `session_flags` on the long session from the log.
- [ ] `docs/HARNESS-NOTES.md`: what a real compaction records (fields of `compactMetadata`, summary line, local command lines), what it re-attaches (`file`, `compact_file_reference`, `invoked_skills`, re-sent hidden context deltas), that a few messages are kept word for word (`preservedMessages`) while the 5.1 transcript estimate resets all history, and that `file` attachments have only been seen after compactions.
- [ ] README milestone status (5.2 done) and `CLAUDE.md` current milestone (5.2 complete, with the spec path; mention the new fixture project and the `flags-data.json` mock file in the snapshot refresh command list).
- [ ] Final whole-branch review on the most capable model, one fix wave, all checks green, then report to the user.
