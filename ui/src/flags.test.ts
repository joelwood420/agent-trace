// Tests for the flag helpers behind badges and jumps. Run with `npm test`.

import assert from 'node:assert/strict'
import { test } from 'node:test'

import { boxFlags, flagsForNode, indexFlags, isOpen, revealPath } from './flags.ts'
import type { DiagramNode, Flag, NodeKind } from './types.ts'

function box(id: string, kind: NodeKind, traceIds: string[], children: DiagramNode[] = [], collapsed = false): DiagramNode {
  return {
    id,
    kind,
    label: id,
    detail_label: null,
    started_at_ms: null,
    duration_ms: null,
    context_tokens: null,
    output_tokens: null,
    status: 'none',
    collapsed_by_default: collapsed,
    trace_ids: traceIds,
    children,
  }
}

function flag(traceIds: string[], text = 'a flag'): Flag {
  return { kind: 'repeated_read', level: 'warn', text, trace_ids: traceIds, run_id: 'run:1' }
}

/** A summary box over three model calls, each with one tool call (like the fixture). */
function summaryTree(): DiagramNode {
  const call = (n: number) => box(`model_call:m${n}`, 'model_call', [`m${n}`], [box(`tool_call:t${n}`, 'tool_call', [`t${n}`])])
  return box('summary:m1', 'summary', ['t1', 't2', 't3'], [call(1), call(2), call(3)], true)
}

test('indexFlags maps every trace id', () => {
  const a = flag(['x', 'shared'])
  const b = flag(['shared', 'y'])
  const index = indexFlags([a, b])
  assert.deepEqual(index.get('shared'), [a, b])
  assert.deepEqual(index.get('x'), [a])
  assert.deepEqual(index.get('y'), [b])
  assert.equal(index.get('z'), undefined)
})

test('leaf box shows its own flags', () => {
  const leaf = box('tool_call:t1', 'tool_call', ['t1'])
  const a = flag(['t1'])
  const b = flag(['t1', 't9'])
  const shown = boxFlags(leaf, indexFlags([a, b, flag(['t9'])]), true)
  assert.deepEqual(shown, [a, b])
})

test('open box does not repeat a child flag', () => {
  const tool = box('tool_call:t1', 'tool_call', ['t1'])
  const model = box('model_call:m1', 'model_call', ['m1'], [tool])
  const index = indexFlags([flag(['t1'])])
  assert.equal(boxFlags(model, index, true).length, 0)
  assert.equal(boxFlags(tool, index, true).length, 1)
})

test('open box keeps a flag that touches only itself', () => {
  const tool = box('tool_call:t1', 'tool_call', ['t1'])
  const model = box('model_call:m1', 'model_call', ['m1'], [tool])
  const own = flag(['m1'])
  assert.deepEqual(boxFlags(model, indexFlags([own, flag(['t1'])]), true), [own])
})

test('collapsed box sums its subtree once', () => {
  const summary = summaryTree()
  const index = indexFlags([flag(['t1', 't2', 't3']), flag(['t2'])])
  assert.equal(boxFlags(summary, index, false).length, 2)
})

test('collapsed box counts flags on trace ids only its descendants hold', () => {
  // The fixture's summary boxes list model call ids, while the flags name tool ids.
  const call = (n: number) => box(`model_call:m${n}`, 'model_call', [`m${n}`], [box(`tool_call:t${n}`, 'tool_call', [`t${n}`])])
  const summary = box('summary:m1', 'summary', ['m1', 'm2', 'm3'], [call(1), call(2), call(3)], true)
  const index = indexFlags([flag(['t1', 't2', 't3'])])
  assert.equal(boxFlags(summary, index, false).length, 1)
})

test('open summary shows nothing of its own', () => {
  const summary = summaryTree()
  const index = indexFlags([flag(['t1', 't2', 't3']), flag(['t2'])])
  assert.equal(boxFlags(summary, index, true).length, 0)
})

test('isOpen follows the user choice, then the default', () => {
  const summary = summaryTree()
  assert.equal(isOpen(summary, new Map()), false)
  assert.equal(isOpen(summary, new Map([[summary.id, true]])), true)
  const plain = box('model_call:m1', 'model_call', ['m1'], [box('tool_call:t1', 'tool_call', ['t1'])])
  assert.equal(isOpen(plain, new Map()), true)
})

test('revealPath goes through a summary to the deepest box', () => {
  const summary = summaryTree()
  const root = box('prompt:p', 'prompt', ['p'], [summary])
  const path = revealPath(root, 't2')
  assert.deepEqual(
    path?.map((n) => n.id),
    ['prompt:p', 'summary:m1', 'model_call:m2', 'tool_call:t2'],
  )
})

test('revealPath returns null for an unknown id', () => {
  const root = box('prompt:p', 'prompt', ['p'], [summaryTree()])
  assert.equal(revealPath(root, 'nope'), null)
})

test('revealPath finds the root itself', () => {
  const root = box('prompt:p', 'prompt', ['p'], [summaryTree()])
  assert.deepEqual(
    revealPath(root, 'p')?.map((n) => n.id),
    ['prompt:p'],
  )
})

test('flagsForNode lists the flags touching a box', () => {
  const group = box('parallel_group:m1', 'parallel_group', ['t1', 't2'])
  const a = flag(['t1'])
  const b = flag(['t1', 't2'])
  const c = flag(['t3'])
  assert.deepEqual(flagsForNode(group, indexFlags([a, b, c])), [a, b])
})
