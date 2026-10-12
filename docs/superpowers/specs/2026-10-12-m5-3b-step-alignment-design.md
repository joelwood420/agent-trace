# M5.3b: line up two runs step by step

## Goal

5.3 compares two runs by their totals. 5.3b lines the two runs up step by step, so you can see where they went different ways: "both read `orders.py`, then A ran a Grep that B skipped". It marks the first place they split and shows the steps only one run took.

## Decisions made while designing

Chosen by the project owner:

- Same step, option B: the same tool and the same target (file path, command, pattern, URL). A different read range or different whitespace still matches.
- Display, option A: two columns side by side like a diff tool, in the Compare view below the summary, with the first divergence marked. Clicking a step opens that run's diagram at the box.
- Prompt pairing, option B: pair prompts first (by prompt text, notifications by order), then line up the steps inside each pair. An unmatched prompt shows as only in A or only in B.
- Approach A: a harness-neutral `insights::align` module with a standard diff (longest common subsequence), target fields supplied by the adapter as data (`AlignRules`, like 5.2's `FlagRules`), and the result returned by `compare_runs`.
- Parallel tool calls of one model call are sorted by key, so a different order is not a divergence.
- A subagent is one step. Lining up the steps inside two matched subagents is out of scope for now.

## Scope

In: `insights::align`, `AlignRules` from the adapter, `alignment` in the `compare_runs` response, a Steps section in the Compare view, jumps to either side's diagram, a new invented fixture session that aligns well with the basic sample, mock data, docs.

Out: subagent internals, lining up model calls or text output, three or more runs, any schema change.

## Rules (`AlignRules`, from the adapter)

```
AlignRules {
  targets: Vec<TargetRule>,          // tool -> input fields that name its target
  fallback_fields: Vec<String>,      // tried in order for tools with no rule
  notification_prefixes: Vec<String> // prompts starting with one of these are notifications
}
TargetRule { tool: String, fields: Vec<String> }
```

A step's target is every listed field that is present, as text, joined with " / ". A tool with no rule uses the first present field of `fallback_fields`. No field present: the target is empty and the key is the tool name alone.

Claude Code rules: Read, Edit, MultiEdit, Write `file_path`; NotebookEdit `notebook_path`; Bash and PowerShell `command`; Grep `pattern`, `glob`; Glob `pattern` (`path` is left out because it is optional and defaults to the working folder; found in the real-app check, where one run passed it and the other did not); WebFetch `url`; WebSearch `query`; Agent and Task `subagent_type`, `description`. Fallback: `command`, `file_path`, `pattern`, `query`, `url`, `path`. Notification prefix: `<task-notification>`.

## Computation (`insights::align`)

### Steps

Each tool call of the main run is a step: `key` = tool name + target with runs of whitespace collapsed to one space and trimmed; `trace_id`; `label` = `Tool(target shortened to 40 characters)` or `Tool`; `failed` (result is an error); `running` (no result yet); `duration_ms`. Tool calls under one model call are sorted by key (stable). A tool call that starts a subagent is one step; nothing inside the subagent run is a step.

### Prompt pairing

Each main-run turn gets a prompt key: its text blocks joined and trimmed with whitespace collapsed, or `notification` if the trimmed text starts with a notification prefix. The two key lists are diffed with the same LCS as steps. Each result is a prompt pair: A turn, B turn, or both.

### Step lineup

Inside each prompt pair (a one-sided pair has all rows one-sided) the step keys are diffed with LCS. Each row is `both`, `only_a` or `only_b`. A `both` row where exactly one side failed is `differs: true`.

### Divergence

Each pair records `first_difference`: the index of its first row that is not a `both` row with `differs: false`, or null. The alignment records the overall first difference as (pair index, row index), or null if none.

### Size limit

`MAX_CELLS` = 4,000,000. If steps(A) x steps(B) in one pair exceeds it, that pair is matched in order instead (row i pairs step i of each side when keys are equal, otherwise both are emitted as one-sided rows) and the pair is marked `approximate: true`. The same limit applies to prompt pairing.

### Output

```
Alignment {
  pairs: Vec<PromptPair>,
  first_difference: Option<{ pair: usize, row: usize }>,
}
PromptPair {
  a: Option<PromptSide>, b: Option<PromptSide>,   // turn_id, index (from 0), preview
  rows: Vec<AlignRow>,
  first_difference: Option<usize>,
  approximate: bool,
}
AlignRow { kind: both | only_a | only_b, a: Option<Step>, b: Option<Step>, differs: bool }
Step { trace_id, label, failed, running, duration_ms: Option<u64> }
```

`align(trace_a, scope_a, trace_b, scope_b, rules) -> Alignment`, pure. With a prompt scope on a side, that side has only that turn; two prompt scopes give exactly one pair (paired regardless of text).

## App

`CompareView` gains `alignment: Alignment`, built in `compare_runs` from both traces with `align_rules()`. Since each side's trace is only read inside `copy_from_trace`, the app copies the per-side inputs out (steps and prompt keys) and aligns after both sides are read: `insights::align` exposes `run_steps(trace, scope, rules) -> RunSteps` (copied under the lock) and `align_steps(a: &RunSteps, b: &RunSteps) -> Alignment`. `align` is `align_steps(run_steps(a), run_steps(b))`.

## UI

- A "Steps" section in the Compare view after Problems, before Outcome.
- Header: "First difference: prompt N, step M" as a button that scrolls to that row, or "The runs took the same steps." A note per approximate pair: "Too long to line up exactly; matched in order."
- Per prompt pair: a subheading with each side's "Prompt N: preview", or "only in A/B" for an unmatched prompt. A two-column table; each cell shows the step label, a red error mark if failed, "running" if running, and its duration. A one-sided row tints the empty slot and tags "only in A/B". A `differs` row tags "outcome differs". The pair's first-difference row has a left border marker.
- Long pairs show the first 50 rows and a "Show all N steps" button.
- Clicking a step leaves the Compare view and opens that run's diagram at the box with the 5.2 jump. For side B it opens session B first, then jumps once the diagram is loaded.
- Live: refreshed with the comparison.
- Below 900px each row stacks A above B.
- Mock: compare-data.json includes alignments; a new invented fixture session (a variant of the basic sample with two steps changed) gives a pair that lines up well.

## Error handling

No target: key is the tool name. No steps: "No steps to line up." Running steps can match. Skipped lines are logged as today. A jump to a box that no longer exists does nothing, as in 5.2.

## Testing

- `insights::align`: identical runs; extra step on each side; two steps swapped; parallel calls in a different order; same target with a different range and with different whitespace; one side failed; an extra prompt; notifications paired in order; a subagent as one step; the size fallback (with a low limit passed in for the test); first difference per pair and overall; prompt scopes.
- Adapter: `align_rules()` covers Read, Edit, MultiEdit, Write, NotebookEdit, Bash, PowerShell, Grep, Glob, WebFetch, WebSearch, Agent, Task.
- Fixture: the variant session under a new `fixtures/claude-code/variant/` project, invented; existing fixture tests and mock snapshots updated for the extra session.
- App: `compare_runs` alignment for basic vs variant has the expected first difference and one-sided rows.
- UI: unit tests for row formatting, the "Show all" cut-off and the jump target; lint, typecheck, test, build.
- Real-app check: the haiku and sonnet `proxyrun` sessions (the extra Grep shows as only in A), two prompts of a long session, jumps to both sides.

## Docs

`docs/VIEW-MODEL.md` (alignment shape), `docs/DECISIONS.md` (LCS, size limit, parallel sort, subagent as one step, rules as data), `docs/HARNESS-NOTES.md` (what real lineups show), README and CLAUDE.md milestone status. No `docs/SCHEMA.md` change.
