// Text and selection helpers for the capture views. They only reshape what
// the backend sends (summaries, diffs, overviews) into lines to show.

import { captureIdFor } from './captures.ts'
import { formatDuration } from './format.ts'
import type {
  CallSummary,
  CaptureOverview,
  CaptureRecord,
  DiagramNode,
  MessageSummary,
  RequestDiff,
} from './types.ts'

/** JSON on one line; falls back to `String` for values JSON cannot write. */
export function compactJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return String(value)
  }
}

/** Settings that are shown first, in this order. */
const FIRST_SETTINGS = ['model', 'max_tokens', 'thinking']

/**
 * The request settings as `key: value` rows: model, max_tokens and thinking
 * first, then the rest in request order. A string model is shown as is,
 * every other value as compact JSON.
 */
export function settingRows(settings: [string, unknown][]): { key: string; value: string }[] {
  const first = FIRST_SETTINGS.flatMap((key) => settings.filter(([k]) => k === key))
  const rest = settings.filter(([k]) => !FIRST_SETTINGS.includes(k))
  return [...first, ...rest].map(([key, value]) => ({
    key,
    value: key === 'model' && typeof value === 'string' ? value : compactJson(value),
  }))
}

/** What changed since the previous call, as short lines. */
export function diffLines(diff: RequestDiff): { kept: number; historyRemoved: boolean; changes: string[] } {
  const changes: string[] = []
  if (diff.system_changed) changes.push('System prompt changed')
  if (diff.tools_changed || diff.tools_added.length > 0 || diff.tools_removed.length > 0) {
    const names = [...diff.tools_added.map((n) => `+${n}`), ...diff.tools_removed.map((n) => `-${n}`)]
    changes.push(names.length > 0 ? `Tools changed: ${names.join(' ')}` : 'Tools changed')
  }
  for (const s of diff.settings_changed) {
    changes.push(`${s.key}: ${compactJson(s.before)} -> ${compactJson(s.after)}`)
  }
  for (const b of diff.betas_added) changes.push(`Beta added: ${b}`)
  for (const b of diff.betas_removed) changes.push(`Beta removed: ${b}`)
  return { kept: diff.shared_prefix, historyRemoved: diff.removed.length > 0, changes }
}

/** `assistant: text, tool_use (190 chars)`. */
export function messageLine(m: MessageSummary): string {
  const types = m.block_types.length > 0 ? m.block_types.join(', ') : 'no blocks'
  return `${m.role}: ${types} (${m.chars} ${m.chars === 1 ? 'char' : 'chars'})`
}

/** `System prompt (3 blocks, 340 chars)`. */
export function systemHeading(blocks: { text: string; cached: boolean }[]): string {
  const chars = blocks.reduce((sum, b) => sum + b.text.length, 0)
  const blockWord = blocks.length === 1 ? 'block' : 'blocks'
  const charWord = chars === 1 ? 'char' : 'chars'
  return `System prompt (${blocks.length} ${blockWord}, ${chars} ${charWord})`
}

/** The calls that belong to no model call box, in the backend's order. */
export function otherCalls(overview: CaptureOverview): CallSummary[] {
  return overview.other_call_ids.flatMap((id) => overview.calls.filter((c) => c.capture_id === id))
}

/** `test-small-model - 200 - 450 ms`, or the error instead of the status. */
export function callLine(call: CallSummary): string {
  const status = call.error ?? (call.status === null ? null : String(call.status))
  return [call.model ?? 'unknown model', status, formatDuration(call.duration_ms)].filter(Boolean).join(' - ')
}

/** The capture of a model call box, if one of its trace ids was captured. */
export function captureIdForNode(overview: CaptureOverview | null, node: DiagramNode): string | null {
  if (node.kind !== 'model_call') return null
  for (const id of node.trace_ids) {
    const found = captureIdFor(overview, id)
    if (found !== null) return found
  }
  return null
}

/** True for a model call box with a captured API call. */
export function isCaptured(node: DiagramNode, capturedIds: ReadonlySet<string>): boolean {
  return node.kind === 'model_call' && node.trace_ids.some((id) => capturedIds.has(id))
}

/** The request body to copy: formatted JSON, text as is, or nothing. */
export function requestJsonText(record: CaptureRecord): string {
  const body = record.request.body
  switch (body.kind) {
    case 'json':
      return JSON.stringify(body.value, null, 2) ?? ''
    case 'text':
      return body.value
    case 'empty':
      return ''
  }
}

/** True if the session load that started an action is still the current one. */
export function sameLoad(startToken: number, currentToken: number): boolean {
  return startToken === currentToken
}

/**
 * How long to wait before a throttled action may run again: 0 if it may run
 * now (never ran, or the interval has passed), else the rest of the interval.
 */
export function throttleDelay(lastRunMs: number | null, nowMs: number, intervalMs: number): number {
  if (lastRunMs === null) return 0
  return Math.max(0, lastRunMs + intervalMs - nowMs)
}

/** The capture overview as the app holds it (same shape as `Loadable`). */
export type OverviewState =
  | { status: 'loading' }
  | { status: 'ready'; value: CaptureOverview }
  | { status: 'error'; message: string }

/** What the details panel shows for a box's captured API call. */
export type CaptureState =
  | { kind: 'capture'; captureId: string }
  | { kind: 'loading' }
  | { kind: 'error'; message: string }

/**
 * The captured call of a box with the API tag: its id once the overview is
 * loaded, or a loading or error state meanwhile. Null for boxes without the
 * tag, or when the loaded overview no longer has the call.
 */
export function captureState(
  node: DiagramNode,
  capturedIds: ReadonlySet<string>,
  overview: OverviewState,
): CaptureState | null {
  if (!isCaptured(node, capturedIds)) return null
  if (overview.status === 'loading') return { kind: 'loading' }
  if (overview.status === 'error') return { kind: 'error', message: overview.message }
  const captureId = captureIdForNode(overview.value, node)
  return captureId === null ? null : { kind: 'capture', captureId }
}
