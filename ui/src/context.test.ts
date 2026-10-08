import assert from 'node:assert/strict'
import { test } from 'node:test'

import { afterFailedLoad, barSegments, barTooltip, fromTranscriptTag, hiddenContextRows, SLICE_NAMES, topSlices, totalNote } from './context.ts'
import type { ContextBar, ContextBreakdown, ContextItem, ContextSlice, HiddenPart } from './types.ts'

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

test('afterFailedLoad keeps the ready value when a background reload fails', () => {
  const ready = { status: 'ready' as const, value: 7 }
  assert.deepEqual(afterFailedLoad(ready, false, 'boom'), ready)
})

test('afterFailedLoad shows the error on a fresh load or when nothing is ready', () => {
  const ready = { status: 'ready' as const, value: 7 }
  assert.deepEqual(afterFailedLoad(ready, true, 'boom'), { status: 'error', message: 'boom' })
  assert.deepEqual(afterFailedLoad<number>({ status: 'loading' }, false, 'boom'), { status: 'error', message: 'boom' })
  assert.deepEqual(afterFailedLoad<number>({ status: 'error', message: 'old' }, false, 'boom'), {
    status: 'error',
    message: 'boom',
  })
})

test('hiddenContextRows names kinds and formats sizes', () => {
  const parts: HiddenPart[] = [
    { trace_id: 'context:a', key: 'k1', kind: 'system_prompt', label: 'system prompt', chars: 27500 },
    { trace_id: 'context:b', key: 'k2', kind: 'tool_definitions', label: 'tools', chars: 999 },
    { trace_id: 'context:c', key: 'k3', kind: 'instructions', label: 'CLAUDE.md', chars: 1000 },
    { trace_id: 'context:d', key: 'k4', kind: 'reminder', label: 'environment', chars: 0 },
    { trace_id: 'context:e', key: 'k5', kind: { other: 'skill listing' }, label: 'skills', chars: 1234 },
  ]
  assert.deepEqual(hiddenContextRows(parts), [
    { traceId: 'context:a', kindName: 'System prompt', label: 'system prompt', size: '27.5k chars' },
    { traceId: 'context:b', kindName: 'Tool definitions', label: 'tools', size: '999 chars' },
    { traceId: 'context:c', kindName: 'Instructions', label: 'CLAUDE.md', size: '1.0k chars' },
    { traceId: 'context:d', kindName: 'Reminder', label: 'environment', size: '0 chars' },
    { traceId: 'context:e', kindName: 'skill listing', label: 'skills', size: '1.2k chars' },
  ])
})

test('hiddenContextRows of nothing is empty', () => {
  assert.deepEqual(hiddenContextRows([]), [])
})

test('fromTranscriptTag only tags items measured from the transcript', () => {
  const item: ContextItem = { label: 'x', tokens: 1, count: 1, largest_tokens: 1, from_transcript: true }
  assert.equal(fromTranscriptTag(item), 'from transcript')
  assert.equal(fromTranscriptTag({ ...item, from_transcript: false }), null)
})
