import assert from 'node:assert/strict'
import { test } from 'node:test'

import { barSegments, barTooltip, SLICE_NAMES } from './context.ts'
import type { ContextBar } from './types.ts'

function bar(slices: [ContextBar['slices'][number]['kind'], number][], total: number): ContextBar {
  return {
    source: 'captured',
    total_tokens: total,
    total_is_reported: true,
    slices: slices.map(([kind, tokens]) => ({ kind, tokens })),
  }
}

test('barSegments percentages sum to 100 and keep order', () => {
  const b = bar(
    [
      ['system_prompt', 1000],
      ['tool_definitions', 6000],
      ['conversation', 3000],
    ],
    10000,
  )
  const segs = barSegments(b)
  assert.deepEqual(
    segs.map((s) => s.kind),
    ['system_prompt', 'tool_definitions', 'conversation'],
  )
  assert.ok(Math.abs(segs.reduce((n, s) => n + s.percent, 0) - 100) < 0.01)
  assert.equal(segs[1]?.percent, 60)
})

test('barSegments keeps tiny slices', () => {
  const segs = barSegments(bar([['system_prompt', 1], ['conversation', 9999]], 10000))
  assert.equal(segs.length, 2)
})

test('barSegments gives [] for an empty bar or a zero total', () => {
  assert.deepEqual(barSegments(bar([], 100)), [])
  assert.deepEqual(barSegments(bar([['conversation', 0]], 0)), [])
})

test('barTooltip has one grouped line per slice with whole percentages', () => {
  const text = barTooltip(
    bar(
      [
        ['tool_definitions', 41234],
        ['conversation', 68766],
      ],
      110000,
    ),
  )
  assert.equal(text, 'Tool definitions: 41,234 tokens (37%)\nConversation: 68,766 tokens (63%)')
})

test('a not_captured slice is named Not captured', () => {
  assert.equal(SLICE_NAMES.not_captured, 'Not captured')
  assert.match(barTooltip(bar([['not_captured', 500]], 500)), /^Not captured: 500 tokens \(100%\)$/)
})
