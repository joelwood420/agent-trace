// Tests for the capture display helpers. Run with `npm test`.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { captureIdFor, messageList, requestBody, systemBlocks, toolList } from './captures.ts'
import type { CaptureDetail, CaptureOverview } from './types.ts'

const url = new URL('./mock/capture-data.json', import.meta.url)
const data = JSON.parse(readFileSync(url, 'utf8')) as {
  overview: CaptureOverview
  details: Record<string, CaptureDetail>
}
const detail = data.details['fixture-001']

test('requestBody returns the JSON object with messages', () => {
  const body = requestBody(detail.record)
  assert.ok(body)
  assert.ok(Array.isArray(body.messages))
})

test('requestBody is null for a non-JSON body', () => {
  const record = structuredClone(detail.record)
  record.request.body = { kind: 'text', value: 'x' }
  assert.equal(requestBody(record), null)
  assert.deepEqual(systemBlocks(null), [])
  assert.deepEqual(toolList(null), [])
  assert.deepEqual(messageList(null), [])
})

test('systemBlocks gives 3 blocks, the last two cached', () => {
  const blocks = systemBlocks(requestBody(detail.record))
  assert.equal(blocks.length, 3)
  assert.deepEqual(
    blocks.map((b) => b.cached),
    [false, true, true],
  )
})

test('toolList names match the summary', () => {
  const tools = toolList(requestBody(detail.record))
  assert.deepEqual(
    tools.map((t) => t.name),
    detail.summary?.tool_names,
  )
})

test('messageList roles alternate starting with user', () => {
  const messages = messageList(requestBody(detail.record))
  assert.ok(messages.length > 0)
  messages.forEach((m, i) => assert.equal(m.role, i % 2 === 0 ? 'user' : 'assistant'))
})

test('messageList reads tool blocks in later requests', () => {
  const ids = data.overview.calls.map((c) => c.capture_id)
  const last = data.details[ids[ids.length - 1]]
  const types = messageList(requestBody(last.record)).flatMap((m) => m.blocks.map((b) => b.type))
  assert.ok(types.includes('tool_use'))
  assert.ok(types.includes('tool_result'))
})

test('captureIdFor finds a model call and returns null otherwise', () => {
  const call = data.overview.calls[0]
  assert.equal(captureIdFor(data.overview, call.trace_id ?? ''), call.capture_id)
  assert.equal(captureIdFor(data.overview, 'model:nope'), null)
  assert.equal(captureIdFor(null, 'x'), null)
})
