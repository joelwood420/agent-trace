// Tests for the diagram layout. Run with `npm test` (Node's built-in test
// runner, which strips the TypeScript types).

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import {
  GROUP_HEADER,
  INDENT,
  NODE_WIDTH,
  collapsibleIds,
  initialViewport,
  isOpen,
  layoutTree,
  type LayoutBox,
} from './layout.ts'
import type { DiagramNode, SessionView } from './types.ts'

/** The fixture session's diagram, as the backend returns it. */
function fixtureView(): SessionView {
  const url = new URL('./mock/fixture-data.json', import.meta.url)
  const data = JSON.parse(readFileSync(url, 'utf8')) as { views: Record<string, SessionView> }
  const view = Object.values(data.views)[0]
  assert.ok(view, 'fixture has a session view')
  return view
}

function node(id: string, kind: DiagramNode['kind'], children: DiagramNode[] = []): DiagramNode {
  return {
    id,
    kind,
    label: id,
    detail_label: null,
    started_at_ms: null,
    duration_ms: null,
    context_tokens: null,
    output_tokens: null,
    status: 'ok',
    collapsed_by_default: kind === 'subagent' || kind === 'summary',
    trace_ids: [id],
    children,
  }
}

function overlaps(a: LayoutBox, b: LayoutBox): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

function contains(outer: LayoutBox, inner: LayoutBox): boolean {
  return (
    inner.x >= outer.x &&
    inner.y >= outer.y &&
    inner.x + inner.width <= outer.x + outer.width &&
    inner.y + inner.height <= outer.y + outer.height
  )
}

/** Ids of the boxes that should be visible for this open state. */
function visibleIds(root: DiagramNode, open: Map<string, boolean>): string[] {
  const out: string[] = []
  const walk = (n: DiagramNode) => {
    out.push(n.id)
    if (isOpen(n, open)) n.children.forEach(walk)
  }
  walk(root)
  return out
}

/** Every pair of boxes either does not overlap, or one is a group around the other. */
function assertNoOverlaps(boxes: LayoutBox[]) {
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i]
      const b = boxes[j]
      if (!overlaps(a, b)) continue
      const nested =
        (a.node.kind === 'parallel_group' && contains(a, b)) ||
        (b.node.kind === 'parallel_group' && contains(b, a))
      assert.ok(nested, `${a.id} overlaps ${b.id}`)
    }
  }
}

test('every fixture prompt lays out without overlaps, collapsed and expanded', () => {
  for (const prompt of fixtureView().diagram.prompts) {
    const collapsed = new Map<string, boolean>()
    const expanded = new Map(collapsibleIds(prompt.root).map((id) => [id, true]))
    for (const state of [collapsed, expanded]) {
      const layout = layoutTree(prompt.root, state)
      const ids = layout.boxes.map((b) => b.id)
      assert.deepEqual([...ids].sort(), visibleIds(prompt.root, state).sort())
      assert.equal(new Set(ids).size, ids.length, 'no duplicate boxes')
      assertNoOverlaps(layout.boxes)
      for (const box of layout.boxes) {
        assert.ok(box.x >= 0 && box.y >= 0, `${box.id} is inside the layout`)
        assert.ok(box.x + box.width <= layout.width, `${box.id} fits the width`)
        assert.ok(box.y + box.height <= layout.height, `${box.id} fits the height`)
      }
      // Every edge joins two laid out boxes.
      const known = new Set(ids)
      for (const edge of layout.edges) {
        assert.ok(known.has(edge.source) && known.has(edge.target), edge.id)
      }
    }
  }
})

test('collapsed boxes hide their children until opened', () => {
  const prompt = fixtureView().diagram.prompts.find((p) =>
    p.root.children.some((c) => c.kind === 'summary'),
  )
  assert.ok(prompt, 'fixture has a prompt with a summary')
  const summary = prompt.root.children.find((c) => c.kind === 'summary')
  assert.ok(summary)

  const closed = layoutTree(prompt.root, new Map())
  const summaryBox = closed.boxes.find((b) => b.id === summary.id)
  assert.equal(summaryBox?.open, false)
  assert.ok(!closed.boxes.some((b) => b.id === summary.children[0]?.id))

  const opened = layoutTree(prompt.root, new Map([[summary.id, true]]))
  assert.equal(opened.boxes.find((b) => b.id === summary.id)?.open, true)
  assert.ok(opened.boxes.some((b) => b.id === summary.children[0]?.id))
  assert.ok(opened.height > closed.height, 'opening a summary makes room for it')
})

test('a parallel group is drawn around its tool calls', () => {
  const prompt = fixtureView().diagram.prompts.find((p) =>
    p.root.children.some((c) => c.children.some((g) => g.kind === 'parallel_group')),
  )
  assert.ok(prompt, 'fixture has a parallel group')
  const layout = layoutTree(prompt.root, new Map())
  const group = layout.boxes.find((b) => b.node.kind === 'parallel_group')
  assert.ok(group)
  assert.ok(group.node.children.length >= 2)
  for (const child of group.node.children) {
    const box = layout.boxes.find((b) => b.id === child.id)
    assert.ok(box, child.id)
    assert.ok(contains(group, box), `${child.id} is inside the group`)
    assert.ok(box.y >= group.y + GROUP_HEADER, 'below the group header')
  }
  // The group is pushed before its children, so it is drawn behind them.
  const groupIndex = layout.boxes.indexOf(group)
  for (const child of group.node.children) {
    assert.ok(layout.boxes.findIndex((b) => b.id === child.id) > groupIndex)
  }
})

test('sequence children are stacked downwards in order and indented', () => {
  const calls = [1, 2, 3].map((i) => node(`m${i}`, 'model_call', [node(`t${i}`, 'tool_call')]))
  const root = node('p', 'prompt', calls)
  const layout = layoutTree(root, new Map())
  const ys = calls.map((c) => layout.boxes.find((b) => b.id === c.id)?.y ?? -1)
  assert.deepEqual([...ys].sort((a, b) => a - b), ys)
  for (const c of calls) {
    const call = layout.boxes.find((b) => b.id === c.id)
    const tool = layout.boxes.find((b) => b.id === c.children[0]?.id)
    assert.ok(call && tool)
    assert.equal(call.x, INDENT)
    assert.equal(tool.y, call.y, 'tool call sits on the same row as its model call')
    assert.ok(tool.x > call.x + call.width)
  }
  assert.deepEqual(
    layout.edges.map((e) => [e.source, e.target, e.sourceHandle]),
    [
      ['p', 'm1', 'down'],
      ['m1', 't1', 'right'],
      ['p', 'm2', 'down'],
      ['m2', 't2', 'right'],
      ['p', 'm3', 'down'],
      ['m3', 't3', 'right'],
    ],
  )
})

test('a prompt with many model calls grows downwards, not sideways', () => {
  const calls = Array.from({ length: 45 }, (_, i) =>
    node(`m${i}`, 'model_call', [node(`t${i}`, 'tool_call')]),
  )
  const layout = layoutTree(node('p', 'prompt', calls), new Map())
  assert.ok(layout.width < 3 * NODE_WIDTH, `width ${layout.width}`)
  assert.ok(layout.height > 45 * 64)
  assertNoOverlaps(layout.boxes)
})

test('initial viewport fits small diagrams and shows the top of tall ones', () => {
  const small = initialViewport(600, 300, 1000, 800)
  assert.equal(small.zoom, 1)
  assert.ok(small.x > 0 && small.y > 0)

  const tall = initialViewport(700, 5000, 1000, 800)
  assert.ok(tall.zoom >= 0.7, `readable zoom, got ${tall.zoom}`)
  assert.equal(tall.y, 24, 'starts at the top')
})
