// Tests for the capture view helpers. Run with `npm test`.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import {
  callLine,
  capturedCountChanged,
  captureIdForNode,
  compactJson,
  diffLines,
  isCaptured,
  messageLine,
  otherCalls,
  requestJsonText,
  settingRows,
  systemHeading,
} from './captureView.ts'
import type { CaptureDetail, CaptureOverview, DiagramNode, RequestDiff, SessionView } from './types.ts'

const url = new URL('./mock/capture-data.json', import.meta.url)
const data = JSON.parse(readFileSync(url, 'utf8')) as {
  overview: CaptureOverview
  details: Record<string, CaptureDetail>
}

function emptyDiff(): RequestDiff {
  return {
    shared_prefix: 0,
    removed: [],
    added: [],
    system_changed: false,
    tools_changed: false,
    tools_added: [],
    tools_removed: [],
    settings_changed: [],
    betas_added: [],
    betas_removed: [],
  }
}

function node(kind: DiagramNode['kind'], traceIds: string[]): DiagramNode {
  return {
    id: 'box',
    kind,
    label: 'x',
    detail_label: null,
    started_at_ms: null,
    duration_ms: null,
    context_tokens: null,
    output_tokens: null,
    status: 'ok',
    collapsed_by_default: false,
    trace_ids: traceIds,
    children: [],
  }
}

test('compactJson writes JSON on one line and survives odd values', () => {
  assert.equal(compactJson({ type: 'enabled', budget_tokens: 10 }), '{"type":"enabled","budget_tokens":10}')
  assert.equal(compactJson('text'), '"text"')
  assert.equal(compactJson(undefined), 'undefined')
})

test('settingRows puts model, max_tokens and thinking first, others as compact JSON', () => {
  const rows = settingRows([
    ['stream', true],
    ['thinking', { type: 'enabled' }],
    ['max_tokens', 32000],
    ['model', 'claude-test'],
    ['metadata', { user: 'x' }],
  ])
  assert.deepEqual(rows, [
    { key: 'model', value: 'claude-test' },
    { key: 'max_tokens', value: '32000' },
    { key: 'thinking', value: '{"type":"enabled"}' },
    { key: 'stream', value: 'true' },
    { key: 'metadata', value: '{"user":"x"}' },
  ])
})

test('settingRows reads the fixture summary', () => {
  const summary = data.details['fixture-001'].summary
  assert.ok(summary)
  assert.deepEqual(
    settingRows(summary.settings).map((r) => r.key),
    ['model', 'max_tokens', 'stream'],
  )
})

test('diffLines names a tool that was added', () => {
  const diff = data.details['fixture-005'].diff
  assert.ok(diff)
  const lines = diffLines(diff)
  assert.equal(lines.kept, 5)
  assert.equal(lines.historyRemoved, false)
  assert.deepEqual(lines.changes, ['Tools changed: +WebFetch'])
})

test('diffLines lists system, tool, setting and beta changes', () => {
  const diff = emptyDiff()
  diff.system_changed = true
  diff.tools_changed = true
  diff.tools_added = ['A', 'B']
  diff.tools_removed = ['C']
  diff.settings_changed = [
    { key: 'max_tokens', before: 100, after: 200 },
    { key: 'thinking', before: null, after: { type: 'enabled' } },
  ]
  diff.betas_added = ['beta-x']
  diff.betas_removed = ['beta-y']
  assert.deepEqual(diffLines(diff).changes, [
    'System prompt changed',
    'Tools changed: +A +B -C',
    'max_tokens: 100 -> 200',
    'thinking: null -> {"type":"enabled"}',
    'Beta added: beta-x',
    'Beta removed: beta-y',
  ])
})

test('diffLines says tools changed without names when only definitions changed', () => {
  const diff = emptyDiff()
  diff.tools_changed = true
  assert.deepEqual(diffLines(diff).changes, ['Tools changed'])
})

test('diffLines flags removed history', () => {
  const diff = emptyDiff()
  diff.removed = [{ index: 1, role: 'user', block_types: ['text'], chars: 5, preview: 'hello' }]
  assert.equal(diffLines(diff).historyRemoved, true)
  assert.equal(diffLines(emptyDiff()).historyRemoved, false)
})

test('messageLine describes a message summary', () => {
  assert.equal(
    messageLine({ index: 3, role: 'assistant', block_types: ['text', 'tool_use'], chars: 190, preview: 'Hi' }),
    'assistant: text, tool_use (190 chars)',
  )
  assert.equal(messageLine({ index: 0, role: 'user', block_types: [], chars: 0, preview: '' }), 'user: no blocks (0 chars)')
})

test('systemHeading counts blocks and characters', () => {
  assert.equal(
    systemHeading([
      { text: 'abc', cached: false },
      { text: 'de', cached: true },
    ]),
    'System prompt (2 blocks, 5 chars)',
  )
  assert.equal(systemHeading([{ text: 'a', cached: false }]), 'System prompt (1 block, 1 char)')
})

test('otherCalls returns the calls listed as other calls, in order', () => {
  const calls = otherCalls(data.overview)
  assert.deepEqual(
    calls.map((c) => c.capture_id),
    ['fixture-002'],
  )
  assert.equal(calls[0].model, 'test-small-model')
})

test('callLine shows model, status and duration', () => {
  const call = otherCalls(data.overview)[0]
  assert.equal(callLine(call), 'test-small-model - 200 - 450 ms')
  assert.equal(callLine({ ...call, model: null, status: null, error: 'connection reset', duration_ms: null }), 'unknown model - connection reset')
})

test('captureIdForNode finds the capture of a model call box only', () => {
  const call = data.overview.calls[0]
  const traceId = call.trace_id ?? ''
  assert.equal(captureIdForNode(data.overview, node('model_call', [traceId])), call.capture_id)
  assert.equal(captureIdForNode(data.overview, node('tool_call', [traceId])), null)
  assert.equal(captureIdForNode(data.overview, node('model_call', ['model:none'])), null)
  assert.equal(captureIdForNode(null, node('model_call', [traceId])), null)
})

test('isCaptured checks model call boxes against the captured ids', () => {
  const ids = new Set(['model:a'])
  assert.equal(isCaptured(node('model_call', ['model:a']), ids), true)
  assert.equal(isCaptured(node('model_call', ['model:b']), ids), false)
  assert.equal(isCaptured(node('tool_call', ['model:a']), ids), false)
})

test('capturedCountChanged compares the number of captured trace ids', () => {
  const view = (ids: string[]) => ({ captured_trace_ids: ids }) as unknown as SessionView
  assert.equal(capturedCountChanged(null, view([])), false)
  assert.equal(capturedCountChanged(null, view(['a'])), true)
  assert.equal(capturedCountChanged(view(['a']), view(['b'])), false)
  assert.equal(capturedCountChanged(view(['a']), view(['a', 'b'])), true)
})

test('requestJsonText formats a JSON body and returns text bodies as they are', () => {
  const record = structuredClone(data.details['fixture-001'].record)
  const text = requestJsonText(record)
  assert.ok(text.startsWith('{\n'))
  assert.ok(Array.isArray((JSON.parse(text) as { messages: unknown }).messages))
  record.request.body = { kind: 'text', value: 'not json' }
  assert.equal(requestJsonText(record), 'not json')
  record.request.body = { kind: 'empty' }
  assert.equal(requestJsonText(record), '')
})
