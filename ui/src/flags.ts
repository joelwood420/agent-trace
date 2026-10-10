// Pure helpers for showing the waste and problem flags Rust computed: which
// box carries a badge, which flags a box touches, and the path of boxes to
// open when jumping to a flagged trace node. Kept free of React so they can
// be unit tested. The flags themselves are never recomputed here.

import { isOpen as layoutIsOpen, type OpenState } from './layout.ts'
import type { DiagramNode, Flag } from './types.ts'

/** Trace id to the flags that involve it. */
export type FlagIndex = Map<string, Flag[]>

/** Maps every trace id named by a flag to the flags that name it. */
export function indexFlags(flags: readonly Flag[]): FlagIndex {
  const index: FlagIndex = new Map()
  for (const flag of flags) {
    for (const id of flag.trace_ids) {
      const list = index.get(id)
      if (list) {
        if (!list.includes(flag)) list.push(flag)
      } else {
        index.set(id, [flag])
      }
    }
  }
  return index
}

/**
 * Whether this box shows its children. The same rule the layout uses, so a
 * badge always matches what is drawn: the user's choice, else the default.
 */
export function isOpen(node: DiagramNode, openState: OpenState): boolean {
  return layoutIsOpen(node, openState)
}

/** Distinct flags (by identity, in first-seen order) over these trace ids. */
function flagsOver(ids: Iterable<string>, index: FlagIndex): Flag[] {
  const out: Flag[] = []
  for (const id of ids) {
    for (const flag of index.get(id) ?? []) {
      if (!out.includes(flag)) out.push(flag)
    }
  }
  return out
}

/** Every trace id in the subtree under `node`, the node's own included. */
function subtreeIds(node: DiagramNode, out = new Set<string>()): Set<string> {
  for (const id of node.trace_ids) out.add(id)
  for (const child of node.children) subtreeIds(child, out)
  return out
}

/**
 * The flags a box's badge counts. A collapsed box stands for its whole
 * subtree, so it counts every flag found anywhere under it, once. An open
 * box (or one without children) counts only the flags that no box drawn
 * under it already shows, so the same flag is never counted twice on screen.
 */
export function boxFlags(node: DiagramNode, index: FlagIndex, open: boolean): Flag[] {
  if (!open && node.children.length > 0) return flagsOver(subtreeIds(node), index)
  const own = flagsOver(node.trace_ids, index)
  if (own.length === 0 || node.children.length === 0) return own
  const below = new Set<string>()
  for (const child of node.children) subtreeIds(child, below)
  return own.filter((flag) => !flag.trace_ids.some((id) => below.has(id)))
}

/**
 * The flags the details panel lists for a box: exactly the ones its badge
 * counts in its current open or collapsed state, so the two always agree.
 */
export function detailFlags(node: DiagramNode, index: FlagIndex, openState: OpenState): Flag[] {
  return boxFlags(node, index, isOpen(node, openState))
}

/**
 * The boxes from `root` down to the deepest box whose own trace ids include
 * `traceId`, depth first. When a summary and its child both hold the id, the
 * path runs through the summary to the child. `null` if no box holds it.
 */
export function revealPath(root: DiagramNode, traceId: string): DiagramNode[] | null {
  for (const child of root.children) {
    const below = revealPath(child, traceId)
    if (below) return [root, ...below]
  }
  return root.trace_ids.includes(traceId) ? [root] : null
}
