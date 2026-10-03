// Positions the boxes of one prompt diagram. This is rendering only: what
// the boxes are, their labels and grouping all come from the Rust view model.
//
// The layout is a top-down indented tree, chosen so that prompts with dozens
// of model calls stay readable (see docs/DECISIONS.md):
//
// - Sequences (the children of a prompt, subagent or summary, and the
//   subagent under a tool call) are stacked downwards, indented under their
//   parent, in order.
// - A model call's tool call (or parallel group) sits to its right, top
//   aligned, so each step reads as one row: "model call -> what it ran".
// - A parallel group is a container box drawn around its tool calls.
//
// Pure function, no DOM, tested with `npm test`.

import type { DiagramNode, NodeKind } from './types.ts'

export const NODE_WIDTH = 288
export const GAP_Y = 14
export const GAP_X = 40
export const INDENT = 36
export const GROUP_PAD = 12
export const GROUP_HEADER = 34

/** Fixed box heights. Box text is single-line, so every box of a kind has the same size. */
export const NODE_HEIGHT: Record<NodeKind, number> = {
  prompt: 92,
  model_call: 80,
  tool_call: 64,
  parallel_group: 0, // computed from its contents
  subagent: 92,
  summary: 80,
  marker: 52,
}

/** A positioned box. `x` and `y` are its top-left corner. */
export interface LayoutBox {
  id: string
  node: DiagramNode
  x: number
  y: number
  width: number
  height: number
  /** `null` if the box cannot be expanded or collapsed, else whether it is open. */
  open: boolean | null
}

/** A line from a parent box to a child box. */
export interface LayoutEdge {
  id: string
  source: string
  target: string
  /** `down` for sequence children, `right` for a model call's tool calls. */
  sourceHandle: 'down' | 'right'
}

export interface Layout {
  boxes: LayoutBox[]
  edges: LayoutEdge[]
  width: number
  height: number
}

/**
 * Expanded or collapsed state chosen by the user, by box id. Boxes that are
 * not in the map use their `collapsed_by_default` value.
 */
export type OpenState = ReadonlyMap<string, boolean>

/** True if the user can expand and collapse this box. */
export function isCollapsible(node: DiagramNode): boolean {
  if (node.children.length === 0) return false
  return node.collapsed_by_default || node.kind === 'subagent' || node.kind === 'summary'
}

/** Whether this box currently shows its children. */
export function isOpen(node: DiagramNode, state: OpenState): boolean {
  if (!isCollapsible(node)) return true
  return state.get(node.id) ?? !node.collapsed_by_default
}

/** Ids of every collapsible box in the tree, at any depth. */
export function collapsibleIds(root: DiagramNode): string[] {
  const out: string[] = []
  const walk = (node: DiagramNode) => {
    if (isCollapsible(node)) out.push(node.id)
    node.children.forEach(walk)
  }
  walk(root)
  return out
}

interface Extent {
  width: number
  height: number
}

/** Lays out the tree under `root`, with its top-left corner at (0, 0). */
export function layoutTree(root: DiagramNode, state: OpenState): Layout {
  const boxes: LayoutBox[] = []
  const edges: LayoutEdge[] = []

  const addEdge = (source: string, target: string, sourceHandle: 'down' | 'right') => {
    edges.push({ id: `${source}->${target}`, source, target, sourceHandle })
  }

  const place = (node: DiagramNode, x: number, y: number): Extent => {
    if (node.kind === 'parallel_group') return placeGroup(node, x, y)

    const width = NODE_WIDTH
    const height = NODE_HEIGHT[node.kind]
    const open = isOpen(node, state)
    boxes.push({ id: node.id, node, x, y, width, height, open: isCollapsible(node) ? open : null })
    const children = open ? node.children : []
    if (children.length === 0) return { width, height }

    if (node.kind === 'model_call') {
      // Tool calls to the right, stacked if there is more than one box.
      const cx = x + width + GAP_X
      let cy = y
      let maxWidth = 0
      for (const child of children) {
        addEdge(node.id, child.id, 'right')
        const ext = place(child, cx, cy)
        cy += ext.height + GAP_Y
        maxWidth = Math.max(maxWidth, ext.width)
      }
      return { width: width + GAP_X + maxWidth, height: Math.max(height, cy - GAP_Y - y) }
    }

    // A sequence: children stacked below, indented.
    const cx = x + INDENT
    let cy = y + height + GAP_Y
    let maxWidth = width
    for (const child of children) {
      addEdge(node.id, child.id, 'down')
      const ext = place(child, cx, cy)
      cy += ext.height + GAP_Y
      maxWidth = Math.max(maxWidth, INDENT + ext.width)
    }
    return { width: maxWidth, height: cy - GAP_Y - y }
  }

  const placeGroup = (node: DiagramNode, x: number, y: number): Extent => {
    // Push the container first so it is drawn behind its children.
    const box: LayoutBox = { id: node.id, node, x, y, width: 0, height: 0, open: null }
    boxes.push(box)
    const cx = x + GROUP_PAD
    let cy = y + GROUP_HEADER
    let maxWidth = 0
    for (const child of node.children) {
      const ext = place(child, cx, cy)
      cy += ext.height + GAP_Y
      maxWidth = Math.max(maxWidth, ext.width)
    }
    const innerBottom = node.children.length > 0 ? cy - GAP_Y : cy
    box.width = Math.max(NODE_WIDTH, maxWidth) + 2 * GROUP_PAD
    box.height = innerBottom - y + GROUP_PAD
    return { width: box.width, height: box.height }
  }

  const ext = place(root, 0, 0)
  return { boxes, edges, width: ext.width, height: ext.height }
}

export interface Viewport {
  x: number
  y: number
  zoom: number
}

/**
 * Where to start the camera for a diagram of this size in a pane of this
 * size. Small diagrams are fitted and centred. Diagrams that would need a
 * tiny zoom to fit are shown from the top instead, at a readable zoom, so a
 * prompt with 40 model calls opens on its first steps rather than as a speck.
 */
export function initialViewport(
  width: number,
  height: number,
  paneWidth: number,
  paneHeight: number,
  padding = 24,
): Viewport {
  const fit = Math.min(
    (paneWidth - 2 * padding) / Math.max(width, 1),
    (paneHeight - 2 * padding) / Math.max(height, 1),
  )
  const minReadable = 0.7
  if (fit >= minReadable) {
    const zoom = Math.min(fit, 1)
    return {
      x: (paneWidth - width * zoom) / 2,
      y: Math.max(padding, (paneHeight - height * zoom) / 2),
      zoom,
    }
  }
  const zoom = Math.min(1, Math.max(minReadable, (paneWidth - 2 * padding) / Math.max(width, 1)))
  return { x: padding, y: padding, zoom }
}
