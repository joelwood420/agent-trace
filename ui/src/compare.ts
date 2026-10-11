// Formatting for the Compare view. Every number, difference and "only in"
// tag comes from the backend; these helpers only turn them into text and
// chart coordinates.

import { formatDuration, formatTokens } from './format.ts'
import type { CallPoint, CompareRow, CompareSection, CompareUnit, PromptChoice } from './types.ts'

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
