// Tests for the live-update logic. Run with `npm test`.

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import {
  findNode,
  isForSession,
  isNewer,
  keepPromptIndex,
  needsDetailRefetch,
  newPromptIndexes,
  statusMessage,
} from './live.ts'
import type { DiagramNode, LiveMessage, SessionSummary, SessionView } from './types.ts'

function steps(): LiveMessage[] {
  const url = new URL('./mock/live-steps.json', import.meta.url)
  return JSON.parse(readFileSync(url, 'utf8')) as LiveMessage[]
}

function viewAt(i: number): SessionView {
  const step = steps()[i]
  assert.ok(step && step.type === 'updated')
  return step.view
}

const session: SessionSummary = {
  project: 'basic',
  project_label: 'basic',
  session_id: '00000000-0000-4000-8000-000000000002',
  title: null,
  modified_ms: 0,
  size_bytes: 0,
  live: true,
}

test('isNewer_rejects_older_versions', () => {
  const v1 = viewAt(0)
  const v2 = viewAt(1)
  assert.equal(isNewer(null, v1), true)
  assert.equal(isNewer(v1, v2), true)
  assert.equal(isNewer(v2, v1), false)
  assert.equal(isNewer(v2, v2), false)
})

test('isForSession matches project and session id', () => {
  const msg: LiveMessage = { type: 'status', project: 'basic', session_id: session.session_id, status: 'watching' }
  assert.equal(isForSession(msg, session), true)
  assert.equal(isForSession({ ...msg, session_id: 'other' }, session), false)
  assert.equal(isForSession({ ...msg, project: 'other' }, session), false)
  assert.equal(isForSession(msg, null), false)
})

test('findNode finds prompt descendants and markers, and returns null when gone', () => {
  const last = viewAt(steps().length - 1).diagram
  const prompt = last.prompts[0]
  assert.ok(prompt)
  const deep = (function deepest(n: DiagramNode): DiagramNode {
    const child = n.children[n.children.length - 1]
    return child ? deepest(child) : n
  })(prompt.root)
  assert.equal(findNode(last, deep.id)?.id, deep.id)
  const marker = last.markers[0]
  if (marker) assert.equal(findNode(last, marker.id)?.id, marker.id)
  assert.equal(findNode(last, 'no-such-id'), null)
})

test('needsDetailRefetch only when one of the node trace ids changed', () => {
  const node = { trace_ids: ['tool:a', 'tool:b'] } as DiagramNode
  assert.equal(needsDetailRefetch(node, ['tool:b']), true)
  assert.equal(needsDetailRefetch(node, ['tool:c']), false)
  assert.equal(needsDetailRefetch(node, []), false)
})

test('newPromptIndexes lists prompts added since the previous diagram', () => {
  const all = steps().map((s) => (s.type === 'updated' ? s.view.diagram : null))
  const first = all[0]
  const last = all[all.length - 1]
  assert.ok(first && last)
  assert.deepEqual(newPromptIndexes(null, last), [])
  const added = newPromptIndexes(first, last)
  assert.deepEqual(
    added,
    last.prompts.slice(first.prompts.length).map((p) => p.index),
  )
  assert.ok(added.length > 0, 'the replay adds prompts')
})

test('statusMessage explains problems and is silent when watching', () => {
  assert.equal(statusMessage(null), null)
  assert.equal(statusMessage('watching'), null)
  assert.match(statusMessage('no_watcher') ?? '', /Refresh/)
  assert.match(statusMessage('deleted') ?? '', /no longer exists/)
})

test('live steps grow and keep increasing versions', () => {
  const versions = steps().map((s) => (s.type === 'updated' ? s.view.version : -1))
  assert.deepEqual(versions, [...versions].sort((a, b) => a - b))
  assert.equal(new Set(versions).size, versions.length)
})

test('keepPromptIndex keeps the prompt while it exists', () => {
  assert.equal(keepPromptIndex(2, 5), 2)
  assert.equal(keepPromptIndex(null, 3), 0)
  assert.equal(keepPromptIndex(4, 3), 0)
  assert.equal(keepPromptIndex(1, 0), null)
})
