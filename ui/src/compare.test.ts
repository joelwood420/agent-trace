// Tests for the Compare view's formatting helpers. Run with `npm test`.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  chartLines,
  formatDifference,
  formatValue,
  presenceTag,
  runRef,
  sameRequest,
  scopeOptions,
  visibleSections,
} from './compare.ts'
import type { CompareRow } from './types.ts'

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
