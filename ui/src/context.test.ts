import assert from 'node:assert/strict'
import { test } from 'node:test'

import { barSegments, barTooltip, SLICE_NAMES, topSlices, totalNote } from './context.ts'
import type { ContextBar, ContextBreakdown, ContextSlice } from './types.ts'

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

function slice(kind: ContextSlice['kind'], tokens: number): ContextSlice {
  return { kind, label: SLICE_NAMES[kind], tokens, share: 0, items: [] }
}

function breakdown(slices: ContextSlice[], total: number, reported: boolean): ContextBreakdown {
  return { source: 'captured', total_tokens: total, total_is_reported: reported, slices, advice: [] }
}

test('totalNote says the split of a reported total', () => {
  assert.equal(totalNote(breakdown([], 41234, true)), 'Estimated split of 41,234 reported tokens')
})

test('totalNote says the total is an estimate when not reported', () => {
  assert.equal(totalNote(breakdown([], 9800, false)), 'Estimated total: about 9,800 tokens')
})

test('topSlices gives the largest first and at most n', () => {
  const b = breakdown([slice('system_prompt', 10), slice('conversation', 30), slice('files_read', 20), slice('instructions', 5)], 65, true)
  assert.deepEqual(
    topSlices(b, 3).map((s) => s.kind),
    ['conversation', 'files_read', 'system_prompt'],
  )
})

test('topSlices with n larger than the slices gives all of them', () => {
  const b = breakdown([slice('system_prompt', 10), slice('conversation', 30)], 40, true)
  assert.equal(topSlices(b, 3).length, 2)
  assert.deepEqual(b.slices.map((s) => s.kind), ['system_prompt', 'conversation'])
})
