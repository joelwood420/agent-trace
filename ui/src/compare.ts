// Formatting for the Compare view. Every number, difference and "only in"
// tag comes from the backend; these helpers only turn them into text and
// chart coordinates.

import { formatDuration, formatTokens } from './format.ts'
import type {
  AlignRow,
  Alignment,
  CallPoint,
  CompareRow,
  CompareSection,
  CompareUnit,
  PromptChoice,
  RunRef,
} from './types.ts'

/** The request for one side: the whole session when `scope` is empty. */
export function runRef(session: { project: string; session_id: string }, scope: string): RunRef {
  return { project: session.project, session_id: session.session_id, prompt_trace_id: scope === '' ? null : scope }
}

/** What a comparison was asked for: the order of the two sessions and each one's scope. */
export interface CompareRequest {
  swapped: boolean
  /** Turn ids by session, '' for the whole session. */
  scopes: { left: string; right: string }
}

/**
 * True if a result asked for `shown` still answers `current`. When it does
 * not, the user changed the order or a scope and the old table is stale.
 */
export function sameRequest(shown: CompareRequest, current: CompareRequest): boolean {
  return (
    shown.swapped === current.swapped &&
    shown.scopes.left === current.scopes.left &&
    shown.scopes.right === current.scopes.right
  )
}

export function scopeOptions(prompts: PromptChoice[]): { value: string; label: string }[] {
  return [
    { value: '', label: 'Whole session' },
    ...prompts.map((p) => ({ value: p.turn_id, label: `Prompt ${p.index + 1}: ${p.preview}` })),
  ]
}

export function formatValue(value: number | null, unit: CompareUnit): string {
  if (value === null) return '?'
  if (unit === 'tokens') return formatTokens(value) ?? String(value)
  if (unit === 'ms') return formatDuration(value) ?? String(value)
  return String(value)
}

export function formatDifference(row: CompareRow): string {
  if (row.difference === null) return ''
  if (row.difference === 0) return 'same'
  const sign = row.difference > 0 ? '+' : '-'
  return `${sign}${formatValue(Math.abs(row.difference), row.unit)}`
}

export function presenceTag(row: CompareRow): string | null {
  if (row.presence === 'only_a') return 'only in A'
  if (row.presence === 'only_b') return 'only in B'
  return null
}

export function visibleSections(sections: CompareSection[]): CompareSection[] {
  return sections.filter((s) => s.rows.length > 0)
}

/** True when any row of the section has a value that counts only some calls. */
export function hasPartial(section: CompareSection): boolean {
  return section.rows.some((r) => r.a_partial || r.b_partial)
}

/** How many rows of a prompt pair show before "Show all". */
export const STEP_ROW_LIMIT = 50

/** The rows of a pair on screen: the first `limit` unless all are shown. */
export function visibleRows(rows: AlignRow[], showAll: boolean, limit = STEP_ROW_LIMIT): AlignRow[] {
  return showAll || rows.length <= limit ? rows : rows.slice(0, limit)
}

/** The tags shown in a step row. */
export function stepTags(row: AlignRow): string[] {
  const tags: string[] = []
  if (row.kind === 'only_a') tags.push('only in A')
  if (row.kind === 'only_b') tags.push('only in B')
  if (row.differs) tags.push('outcome differs')
  return tags
}

/** The header of the Steps section. */
export function diffLabel(alignment: Alignment): string {
  if (alignment.pairs.length === 0) return 'No steps to line up.'
  const point = alignment.first_difference
  if (point === null) return 'The runs took the same steps.'
  const pair = alignment.pairs[point.pair]
  const side = pair?.a ?? pair?.b ?? null
  const prompt = side === null ? point.pair + 1 : side.index + 1
  return `First difference: prompt ${prompt}, step ${point.row + 1}`
}

/** The session a step of side `side` belongs to, given the order on screen. */
export function jumpSession<T>(side: 'a' | 'b', swapped: boolean, left: T, right: T): T {
  return (side === 'a') !== swapped ? left : right
}

const round = (n: number) => Math.round(n * 10) / 10

export function chartLines(
  a: CallPoint[],
  b: CallPoint[],
  width: number,
  height: number,
): { a: string; b: string; maxTokens: number } {
  const known = [...a, ...b].flatMap((p) => (p.tokens === null ? [] : [p.tokens]))
  const maxTokens = known.length === 0 ? 0 : Math.max(...known)
  if (maxTokens === 0) return { a: '', b: '', maxTokens: 0 }
  const n = Math.max(a.length, b.length)
  const line = (points: CallPoint[]) =>
    points
      .flatMap((p) => {
        if (p.tokens === null) return []
        const x = n <= 1 ? 0 : ((p.index - 1) / (n - 1)) * width
        const y = height - (p.tokens / maxTokens) * height
        return [`${round(x)},${round(y)}`]
      })
      .join(' ')
  return { a: line(a), b: line(b), maxTokens }
}
