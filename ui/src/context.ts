// Display helpers for the context bar. The numbers come from Rust; this only
// turns them into widths and text.

import type { ContextBar, SliceKind } from './types.ts'

/** Slice names, the same as `SliceKind::label` in Rust. */
export const SLICE_NAMES: Record<SliceKind, string> = {
  system_prompt: 'System prompt',
  tool_definitions: 'Tool definitions',
  instructions: 'Instructions and reminders',
  files_read: 'Files read',
  tool_results: 'Other tool results',
  conversation: 'Conversation',
  not_captured: 'Not captured',
}

/** One drawn part of a bar. */
export interface Segment {
  kind: SliceKind
  tokens: number
  percent: number
}

/** Segments for a bar: percent of the total, in the given order; slices under 0.5% are kept (the CSS gives them a minimum width of 1px). */
export function barSegments(bar: ContextBar): Segment[] {
  if (bar.total_tokens <= 0) return []
  return bar.slices.map((s) => ({ kind: s.kind, tokens: s.tokens, percent: (s.tokens / bar.total_tokens) * 100 }))
}

/** One tooltip line per slice: "Tool definitions: 41,234 tokens (38%)". */
export function barTooltip(bar: ContextBar): string {
  return barSegments(bar)
    .map((s) => `${SLICE_NAMES[s.kind]}: ${s.tokens.toLocaleString('en-US')} tokens (${Math.round(s.percent)}%)`)
    .join('\n')
}
