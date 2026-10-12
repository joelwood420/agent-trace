// Tests for the Compare view's formatting helpers. Run with `npm test`.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  chartLines,
  diffLabel,
  formatDifference,
  formatValue,
  hasPartial,
  jumpSession,
  presenceTag,
  runRef,
  sameRequest,
  scopeOptions,
  stepTags,
  visibleRows,
  visibleSections,
} from './compare.ts'
import type { AlignRow, AlignStep, Alignment, CompareRow, PromptPair } from './types.ts'

const row = (over: Partial<CompareRow>): CompareRow => ({
  label: 'Output',
  unit: 'count',
  a: 1,
  b: 1,
  a_partial: false,
  b_partial: false,
  difference: 0,
  higher: 'equal',
  presence: 'both',
  ...over,
})

test('scopeOptions starts with the whole session and numbers prompts from 1', () => {
  assert.deepEqual(scopeOptions([{ turn_id: 't1', index: 0, preview: 'Fix the parser' }]), [
    { value: '', label: 'Whole session' },
    { value: 't1', label: 'Prompt 1: Fix the parser' },
  ])
})

test('formatValue shows unknown as a question mark', () => {
  assert.equal(formatValue(null, 'tokens'), '?')
})

test('formatValue shows counts as plain numbers', () => {
  assert.equal(formatValue(12, 'count'), '12')
})

test('formatValue formats tokens and durations like the rest of the app', () => {
  assert.notEqual(formatValue(1500, 'tokens'), '1500')
  assert.notEqual(formatValue(65_000, 'ms'), '65000')
})

test('formatDifference is empty when unknown and "same" when equal', () => {
  assert.equal(formatDifference(row({ difference: null, higher: 'unknown' })), '')
  assert.equal(formatDifference(row({})), 'same')
})

test('formatDifference signs the difference', () => {
  assert.equal(formatDifference(row({ a: 5, b: 2, difference: -3, higher: 'a' })), '-3')
  assert.equal(formatDifference(row({ a: 2, b: 5, difference: 3, higher: 'b' })), '+3')
})

test('presenceTag names the only side', () => {
  assert.equal(presenceTag(row({ presence: 'only_a' })), 'only in A')
  assert.equal(presenceTag(row({ presence: 'only_b' })), 'only in B')
  assert.equal(presenceTag(row({})), null)
})

test('visibleSections drops empty sections', () => {
  const sections = [
    { title: 'Models', rows: [] },
    { title: 'Work', rows: [row({})] },
  ]
  assert.deepEqual(
    visibleSections(sections).map((s) => s.title),
    ['Work'],
  )
})

test('chartLines spreads calls over the width and scales to the largest value', () => {
  const lines = chartLines(
    [
      { index: 1, trace_id: 'a1', tokens: 100 },
      { index: 2, trace_id: 'a2', tokens: 200 },
    ],
    [
      { index: 1, trace_id: 'b1', tokens: 50 },
      { index: 2, trace_id: 'b2', tokens: null },
      { index: 3, trace_id: 'b3', tokens: 100 },
    ],
    200,
    100,
  )
  assert.equal(lines.maxTokens, 200)
  assert.equal(lines.a, '0,50 100,0')
  assert.equal(lines.b, '0,75 200,50')
})

test('chartLines is empty with no known values', () => {
  assert.deepEqual(chartLines([{ index: 1, trace_id: 'a', tokens: null }], [], 10, 10), { a: '', b: '', maxTokens: 0 })
})

test('chartLines puts a single call at the left edge', () => {
  assert.equal(chartLines([{ index: 1, trace_id: 'a', tokens: 10 }], [], 10, 10).a, '0,0')
})

const session = { project: 'p1', session_id: 's1' }

test('runRef asks for the whole session when the scope is empty', () => {
  assert.deepEqual(runRef(session, ''), { project: 'p1', session_id: 's1', prompt_trace_id: null })
})

test('runRef asks for one prompt when a turn id is given', () => {
  assert.deepEqual(runRef(session, 't1'), { project: 'p1', session_id: 's1', prompt_trace_id: 't1' })
})

const request = { swapped: false, scopes: { left: '', right: 't2' } }

test('sameRequest is true when the order and both scopes match', () => {
  assert.equal(sameRequest(request, { swapped: false, scopes: { left: '', right: 't2' } }), true)
})

test('sameRequest is false when the runs were swapped since', () => {
  assert.equal(sameRequest(request, { ...request, swapped: true }), false)
})

test('sameRequest is false when a scope changed since', () => {
  assert.equal(sameRequest(request, { swapped: false, scopes: { left: 't1', right: 't2' } }), false)
  assert.equal(sameRequest(request, { swapped: false, scopes: { left: '', right: '' } }), false)
})

test('hasPartial is true when either side of any row is partial', () => {
  assert.equal(hasPartial({ title: 'Tokens', rows: [row({}), row({})] }), false)
  assert.equal(hasPartial({ title: 'Tokens', rows: [row({}), row({ b_partial: true })] }), true)
  assert.equal(hasPartial({ title: 'Tokens', rows: [row({ a_partial: true })] }), true)
})

const step = (label: string): AlignStep => ({
  trace_id: `t-${label}`,
  label,
  failed: false,
  running: false,
  duration_ms: 10,
})

const alignRow = (over: Partial<AlignRow>): AlignRow => ({
  kind: 'both',
  a: step('Read a.rs'),
  b: step('Read a.rs'),
  differs: false,
  ...over,
})

const pair = (over: Partial<PromptPair>): PromptPair => ({
  a: { turn_id: 'ta', index: 0, preview: 'Fix it' },
  b: { turn_id: 'tb', index: 0, preview: 'Fix it' },
  rows: [alignRow({})],
  first_difference: null,
  approximate: false,
  ...over,
})

test('visibleRows cuts a long pair at the limit unless all are shown', () => {
  const rows = Array.from({ length: 60 }, () => alignRow({}))
  assert.equal(visibleRows(rows, false).length, 50)
  assert.equal(visibleRows(rows, true).length, 60)
  assert.equal(visibleRows(rows, false, 10).length, 10)
  assert.equal(visibleRows(rows.slice(0, 50), false).length, 50)
  assert.equal(visibleRows([], false).length, 0)
})

test('stepTags names one-sided rows and differing outcomes', () => {
  assert.deepEqual(stepTags(alignRow({})), [])
  assert.deepEqual(stepTags(alignRow({ kind: 'only_a', b: null })), ['only in A'])
  assert.deepEqual(stepTags(alignRow({ kind: 'only_b', a: null })), ['only in B'])
  assert.deepEqual(stepTags(alignRow({ differs: true })), ['outcome differs'])
})

test('diffLabel points at the first difference, counting from 1', () => {
  const alignment: Alignment = {
    pairs: [
      pair({}),
      pair({
        a: { turn_id: 'ta2', index: 1, preview: 'More' },
        b: { turn_id: 'tb2', index: 1, preview: 'More' },
        first_difference: 2,
      }),
    ],
    first_difference: { pair: 1, row: 2 },
  }
  assert.equal(diffLabel(alignment), 'First difference: prompt 2, step 3')
})

test('diffLabel names a one-sided prompt without a step', () => {
  const extraB = pair({ a: null, b: { turn_id: 'tb2', index: 4, preview: 'Extra' }, rows: [], first_difference: 0 })
  const onlyB: Alignment = { pairs: [pair({}), extraB], first_difference: { pair: 1, row: 0 } }
  assert.equal(diffLabel(onlyB), 'First difference: prompt 5 (only in B)')
  const extraA = pair({ b: null, a: { turn_id: 'ta2', index: 2, preview: 'Extra' }, rows: [], first_difference: 0 })
  const onlyA: Alignment = { pairs: [extraA], first_difference: { pair: 0, row: 0 } }
  assert.equal(diffLabel(onlyA), 'First difference: prompt 3 (only in A)')
})

test('diffLabel names both prompt numbers when they differ', () => {
  const shifted = pair({ b: { turn_id: 'tb2', index: 3, preview: 'Fix it' }, first_difference: 1 })
  const alignment: Alignment = { pairs: [shifted], first_difference: { pair: 0, row: 1 } }
  assert.equal(diffLabel(alignment), 'First difference: prompt 1 in A, prompt 4 in B, step 2')
})

test('diffLabel says when the runs match or have no steps', () => {
  assert.equal(diffLabel({ pairs: [pair({})], first_difference: null }), 'The runs took the same steps.')
  assert.equal(diffLabel({ pairs: [], first_difference: null }), 'No steps to line up.')
})

test('jumpSession maps a side to its session, following the swap', () => {
  const left = { project: 'p', session_id: 'left' }
  const right = { project: 'p', session_id: 'right' }
  assert.equal(jumpSession('a', false, left, right), left)
  assert.equal(jumpSession('b', false, left, right), right)
  assert.equal(jumpSession('a', true, left, right), right)
  assert.equal(jumpSession('b', true, left, right), left)
})
