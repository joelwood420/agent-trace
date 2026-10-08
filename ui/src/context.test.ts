import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  afterFailedLoad,
  barSegments,
  barTooltip,
  focusParts,
  fromTranscriptTag,
  hiddenContextRows,
  otherPartsLabel,
  SLICE_NAMES,
  topSlices,
  totalNote,
} from './context.ts'
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

function hidden(trace_id: string, key: string, kind: HiddenPart['kind'], label: string, chars: number, part_keys = [key]): HiddenPart {
  return { trace_id, key, kind, label, chars, count: part_keys.length, part_keys }
}

test('hiddenContextRows names kinds and formats sizes', () => {
  const parts: HiddenPart[] = [
    hidden('context:a', 'k1', 'system_prompt', 'system prompt', 27500),
    hidden('context:b', 'tools:built-in', 'tool_definitions', 'built-in', 999),
    hidden('context:c', 'k3', 'instructions', 'CLAUDE.md', 1000),
    hidden('context:d', 'k4', 'reminder', 'environment', 0),
    hidden('context:e', 'k5', { other: 'skill listing' }, 'skills', 1234),
  ]
  assert.deepEqual(hiddenContextRows(parts), [
    { key: 'k1', traceId: 'context:a', label: 'system prompt', meta: 'System prompt - 27.5k chars', count: 1, partKeys: ['k1'] },
    {
      key: 'tools:built-in',
      traceId: 'context:b',
      label: 'built-in',
      meta: 'Tool definitions - 999 chars',
      count: 1,
      partKeys: ['tools:built-in'],
    },
    { key: 'k3', traceId: 'context:c', label: 'CLAUDE.md', meta: 'Instructions - 1.0k chars', count: 1, partKeys: ['k3'] },
    { key: 'k4', traceId: 'context:d', label: 'environment', meta: 'Reminder - 0 chars', count: 1, partKeys: ['k4'] },
    { key: 'k5', traceId: 'context:e', label: 'skills', meta: 'skill listing - 1.2k chars', count: 1, partKeys: ['k5'] },
  ])
})

test('hiddenContextRows keys group rows by row key and counts their tools', () => {
  const parts: HiddenPart[] = [
    hidden('context:1', 'tools:MCP: docs', 'tool_definitions', 'MCP: docs', 12300, ['tool:mcp__docs__a', 'tool:mcp__docs__b']),
    hidden('context:1', 'reminder:1', 'reminder', 'date', 20),
  ]
  const rows = hiddenContextRows(parts)
  assert.deepEqual(
    rows.map((r) => [r.key, r.traceId, r.meta, r.count, r.partKeys]),
    [
      ['tools:MCP: docs', 'context:1', 'Tool definitions - 12.3k chars (2 tools)', 2, ['tool:mcp__docs__a', 'tool:mcp__docs__b']],
      ['reminder:1', 'context:1', 'Reminder - 20 chars', 1, ['reminder:1']],
    ],
  )
  assert.notEqual(rows[0]?.key, rows[1]?.key, 'rows sharing a trace id keep distinct keys')
})

test('focusParts puts the focused parts first and the rest apart', () => {
  const parts = [{ key: 'a' }, { key: 'b' }, { key: 'c' }, { key: 'd' }]
  assert.deepEqual(focusParts(parts, ['c', 'a', 'gone']), {
    focused: [{ key: 'a' }, { key: 'c' }],
    others: [{ key: 'b' }, { key: 'd' }],
  })
})

test('focusParts with no focus keys keeps every part as focused', () => {
  const parts = [{ key: 'a' }, { key: 'b' }]
  assert.deepEqual(focusParts(parts, []), { focused: parts, others: [] })
})

test('otherPartsLabel counts the other parts', () => {
  assert.equal(otherPartsLabel(1), '1 other part in this update')
  assert.equal(otherPartsLabel(5), '5 other parts in this update')
})

test('hiddenContextRows of nothing is empty', () => {
  assert.deepEqual(hiddenContextRows([]), [])
})

test('fromTranscriptTag only tags items measured from the transcript', () => {
  const item: ContextItem = { label: 'x', tokens: 1, count: 1, largest_tokens: 1, from_transcript: true }
  assert.equal(fromTranscriptTag(item), 'from transcript')
  assert.equal(fromTranscriptTag({ ...item, from_transcript: false }), null)
})
