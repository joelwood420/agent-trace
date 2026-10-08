// Display helpers for the context bar. The numbers come from Rust; this only
// turns them into widths and text.

import type { ContextBar, ContextBreakdown, ContextItem, ContextPartKind, ContextSlice, HiddenPart, SliceKind } from './types.ts'

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

/** One tooltip line per slice: "Tool definitions: 41,234 tokens (37%)". */
export function barTooltip(bar: ContextBar): string {
  return barSegments(bar)
    .map((s) => `${SLICE_NAMES[s.kind]}: ${s.tokens.toLocaleString('en-US')} tokens (${Math.round(s.percent)}%)`)
    .join('\n')
}

/** "Estimated split of 41,234 reported tokens" or "Estimated total: about 9,800 tokens" when not reported. */
export function totalNote(b: ContextBreakdown): string {
  const n = b.total_tokens.toLocaleString('en-US')
  return b.total_is_reported ? `Estimated split of ${n} reported tokens` : `Estimated total: about ${n} tokens`
}

/** The `n` largest slices, largest first. */
export function topSlices(b: ContextBreakdown, n: number): ContextSlice[] {
  return [...b.slices].sort((a, c) => c.tokens - a.tokens).slice(0, n)
}

/** A value that loads in the background (same shape as `Loadable` in Sidebar.tsx). */
export type LoadState<T> = { status: 'loading' } | { status: 'ready'; value: T } | { status: 'error'; message: string }

/**
 * The state after a load failed. A background reload (`fresh` false) keeps a
 * ready value, so the bars do not vanish over one failed refresh; a fresh
 * load, or one with nothing ready yet, shows the error.
 */
export function afterFailedLoad<T>(current: LoadState<T>, fresh: boolean, message: string): LoadState<T> {
  if (!fresh && current.status === 'ready') return current
  return { status: 'error', message }
}

/** Names of the part kinds; an unknown kind shows the name Rust gave it. */
export function partKindName(kind: ContextPartKind): string {
  if (typeof kind !== 'string') return kind.other
  switch (kind) {
    case 'system_prompt':
      return 'System prompt'
    case 'tool_definitions':
      return 'Tool definitions'
    case 'instructions':
      return 'Instructions'
    case 'reminder':
      return 'Reminder'
  }
}

/** "27.5k chars" from 1,000 up, else "197 chars". */
function formatChars(chars: number): string {
  return chars < 1000 ? `${chars} chars` : `${(chars / 1000).toFixed(1)}k chars`
}

/** One row of the "Hidden context" list. */
export interface HiddenRow {
  /** Unique within the list: the part key, or `tools:<group>`. */
  key: string
  traceId: string
  label: string
  /** "Reminder - 1.2k chars", plus " (3 tools)" for a group. */
  meta: string
  count: number
  partKeys: string[]
}

/** Rows for the hidden context list, in the order Rust gave them. */
export function hiddenContextRows(parts: HiddenPart[]): HiddenRow[] {
  return parts.map((p) => ({
    key: p.key,
    traceId: p.trace_id,
    label: p.label,
    meta: `${partKindName(p.kind)} - ${formatChars(p.chars)}${p.count > 1 ? ` (${p.count} tools)` : ''}`,
    count: p.count,
    partKeys: p.part_keys,
  }))
}

/**
 * Splits an update's parts into the ones a hidden context row points at
 * (first, in the update's order) and the rest. With no focus keys every
 * part counts as focused.
 */
export function focusParts<T extends { key: string }>(parts: T[], focusKeys: string[]): { focused: T[]; others: T[] } {
  if (focusKeys.length === 0) return { focused: parts, others: [] }
  const wanted = new Set(focusKeys)
  return {
    focused: parts.filter((p) => wanted.has(p.key)),
    others: parts.filter((p) => !wanted.has(p.key)),
  }
}

/** "5 other parts in this update". */
export function otherPartsLabel(n: number): string {
  return `${n} other ${n === 1 ? 'part' : 'parts'} in this update`
}

/** The small tag for an item measured from the transcript, else null. */
export function fromTranscriptTag(item: ContextItem): string | null {
  return item.from_transcript ? 'from transcript' : null
}
