// TypeScript shapes of the backend command responses. They mirror the Rust
// types exactly; see docs/VIEW-MODEL.md and docs/SCHEMA.md. The UI only
// renders these and never reinterprets raw transcript data.

/** Mirrors `LiveStatus` in src-tauri/src/live.rs. */
export type LiveStatus = 'watching' | 'no_watcher' | 'deleted'

/** Mirrors `LiveMessage` in src-tauri/src/live.rs: pushed while a session is open. */
export type LiveMessage =
  | { type: 'updated'; project: string; session_id: string; view: SessionView; changed_trace_ids: string[] }
  | { type: 'status'; project: string; session_id: string; status: LiveStatus }

/** One session file, as returned by `list_sessions`. */
export interface SessionSummary {
  project: string
  project_label: string
  session_id: string
  title: string | null
  modified_ms: number
  size_bytes: number
  /** True if the file was written in the last 10 minutes. */
  live: boolean
}

/** A line the backend could not use. */
export interface SkippedLine {
  source: string
  line: number
  reason: string
}

/** The response of `load_session`. */
export interface SessionView {
  diagram: SessionDiagram
  skipped: SkippedLine[]
  /** True if the main transcript was written in the last 10 minutes. */
  live: boolean
  /** Goes up with every update of the open session; keep the higher one. */
  version: number
  /** Model call trace ids that have a captured API call. */
  captured_trace_ids: string[]
}

export interface SessionDiagram {
  run_id: string | null
  title: string | null
  harness: string | null
  started_at_ms: number | null
  ended_at_ms: number | null
  duration_ms: number | null
  markers: DiagramNode[]
  prompts: PromptDiagram[]
}

export interface PromptTotals {
  model_calls: number
  tool_calls: number
  errors: number
  max_context_tokens: number | null
  output_tokens: number | null
}

export interface PromptDiagram {
  index: number
  turn_id: string
  prompt_preview: string
  started_at_ms: number | null
  ended_at_ms: number | null
  duration_ms: number | null
  totals: PromptTotals
  root: DiagramNode
}

export type NodeKind =
  | 'prompt'
  | 'model_call'
  | 'tool_call'
  | 'parallel_group'
  | 'subagent'
  | 'summary'
  | 'marker'

export type Status = 'ok' | 'error' | 'running' | 'none'

export interface DiagramNode {
  id: string
  kind: NodeKind
  label: string
  detail_label: string | null
  started_at_ms: number | null
  duration_ms: number | null
  context_tokens: number | null
  output_tokens: number | null
  status: Status
  collapsed_by_default: boolean
  trace_ids: string[]
  children: DiagramNode[]
}

// --- node_detail ---

export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }
  | { type: 'image'; media_type?: string }
  | { type: 'other'; kind: string }

export type StopReason = 'end_turn' | 'tool_use' | 'max_tokens' | 'stop_sequence' | { other: string }

export interface Usage {
  input_tokens?: number
  output_tokens?: number
  cache_read_tokens?: number
  cache_write_tokens?: number
}

export type TraceNode =
  | { type: 'run'; harness: string; title?: string }
  | { type: 'turn'; prompt: ContentBlock[] }
  | {
      type: 'model_call'
      model?: string
      output: ContentBlock[]
      stop_reason?: StopReason
      usage?: Usage
    }
  | {
      type: 'tool_call'
      name: string
      input: unknown
      result?: { content: ContentBlock[]; is_error: boolean }
    }
  | { type: 'marker'; kind: string; summary: string }
  | { type: 'context_update'; parts?: ContextPart[]; remove?: string[] }

/** What a recorded piece of hidden context is. */
export type ContextPartKind = 'system_prompt' | 'tool_definitions' | 'instructions' | 'reminder' | { other: string }

/** One piece of hidden context with its full text. */
export interface ContextPart {
  key: string
  kind: ContextPartKind
  label: string
  text: string
}

export interface RawSource {
  source: string
  line: number
  text: string
}

/** The response of `node_detail`. */
export interface NodeDetail {
  trace_id: string
  parent_id: string | null
  kind: string
  started_at_ms: number | null
  ended_at_ms: number | null
  duration_ms: number | null
  context_tokens: number | null
  node: TraceNode
  metadata: Record<string, unknown>
  raw: RawSource[]
}

// ---- Captures (M4). Mirrors the capture-core and src-tauri structs. ----

export interface Header {
  name: string
  value: string
}

export type Body = { kind: 'empty' } | { kind: 'json'; value: unknown } | { kind: 'text'; value: string }

export interface CapturedRequest {
  method: string
  path: string
  headers: Header[]
  body: Body
}

export interface CapturedResponse {
  status: number
  headers: Header[]
  stream: string | null
  message: unknown
}

export interface CaptureRecord {
  id: string
  started_at_ms: number
  first_byte_at_ms: number | null
  ended_at_ms: number | null
  request: CapturedRequest
  response: CapturedResponse | null
  message_id: string | null
  error: string | null
}

export interface RequestSummary {
  model: string | null
  system_hash: string | null
  tools_hash: string | null
  tool_names: string[]
  system_chars: number
  message_count: number
  settings: [string, unknown][]
  betas: string[]
}

export interface MessageSummary {
  index: number
  role: string
  block_types: string[]
  chars: number
  preview: string
}

export interface SettingChange {
  key: string
  before: unknown
  after: unknown
}

export interface RequestDiff {
  shared_prefix: number
  removed: MessageSummary[]
  added: MessageSummary[]
  system_changed: boolean
  tools_changed: boolean
  tools_added: string[]
  tools_removed: string[]
  settings_changed: SettingChange[]
  betas_added: string[]
  betas_removed: string[]
}

export interface CallSummary {
  capture_id: string
  trace_id: string | null
  started_at_ms: number
  duration_ms: number | null
  model: string | null
  status: number | null
  error: string | null
  system_version: number | null
  tools_version: number | null
  message_count: number
}

export interface VersionSummary {
  version: number
  hash: string
  first_capture_id: string
  call_count: number
  system_chars: number | null
  tool_names: string[] | null
}

export interface CaptureOverview {
  session_key: string | null
  total_bytes: number
  calls: CallSummary[]
  system_versions: VersionSummary[]
  tool_versions: VersionSummary[]
  other_call_ids: string[]
  skipped: string[]
}

export interface CaptureDetail {
  record: CaptureRecord
  summary: RequestSummary | null
  previous_capture_id: string | null
  diff: RequestDiff | null
}

export interface CaptureStatus {
  listening: boolean
  port: number
  command: string
  error: string | null
  last_save_error: string | null
}

// --- Context breakdown (what fills each model call's context) ---

/** What a slice of the context is made of. */
export type SliceKind =
  | 'system_prompt'
  | 'tool_definitions'
  | 'instructions'
  | 'files_read'
  | 'tool_results'
  | 'conversation'
  | 'not_captured'

/** Where a breakdown was measured from. */
export type ContextSource = 'captured' | 'transcript'

/** One thing inside a slice, such as one file or one tool. */
export interface ContextItem {
  label: string
  tokens: number
  count: number
  largest_tokens: number
  /** True when measured from the transcript instead of a captured request. */
  from_transcript: boolean
}

/** One slice of a call's context, with the items it is made of. */
export interface ContextSlice {
  kind: SliceKind
  label: string
  tokens: number
  share: number
  items: ContextItem[]
}

/** A plain-language remark about the context. */
export interface Advice {
  level: 'warn' | 'info'
  slice: SliceKind
  text: string
}

/** The full breakdown of one model call's context. */
export interface ContextBreakdown {
  source: ContextSource
  total_tokens: number
  total_is_reported: boolean
  slices: ContextSlice[]
  advice: Advice[]
}

/** A slice of a bar: just its kind and size. */
export interface BarSlice {
  kind: SliceKind
  tokens: number
}

/** The short form of a breakdown, drawn on a model call box. */
export interface ContextBar {
  source: ContextSource
  total_tokens: number
  total_is_reported: boolean
  slices: BarSlice[]
}

/** One hidden context part listed in the session overview. */
export interface HiddenPart {
  trace_id: string
  key: string
  kind: ContextPartKind
  label: string
  chars: number
}

/** Bars for every model call of a session, plus the latest full breakdown. */
export interface SessionContext {
  bars: Record<string, ContextBar>
  latest: ContextBreakdown | null
  latest_trace_id: string | null
  hidden_context: HiddenPart[]
}
