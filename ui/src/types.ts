// TypeScript shapes of the backend command responses. They mirror the Rust
// types exactly; see docs/VIEW-MODEL.md and docs/SCHEMA.md. The UI only
// renders these and never reinterprets raw transcript data.

/** One session file, as returned by `list_sessions`. */
export interface SessionSummary {
  project: string
  project_label: string
  session_id: string
  title: string | null
  modified_ms: number
  size_bytes: number
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
