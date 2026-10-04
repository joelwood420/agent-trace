// Pure logic for applying live updates from the backend. Kept free of React
// so it can be unit tested.

import type { DiagramNode, LiveMessage, LiveStatus, SessionDiagram, SessionSummary, SessionView } from './types.ts'

/** True if `incoming` should replace `current` (it has a higher version). */
export function isNewer(current: SessionView | null, incoming: SessionView): boolean {
  return current === null || incoming.version > current.version
}

/** True if a live message belongs to the session being shown. */
export function isForSession(message: LiveMessage, session: SessionSummary | null): boolean {
  return session !== null && message.project === session.project && message.session_id === session.session_id
}

/** The box with this diagram id in the new diagram, or null if it is gone. */
export function findNode(diagram: SessionDiagram, id: string): DiagramNode | null {
  const search = (nodes: readonly DiagramNode[]): DiagramNode | null => {
    for (const node of nodes) {
      if (node.id === id) return node
      const found = search(node.children)
      if (found) return found
    }
    return null
  }
  return search(diagram.markers) ?? search(diagram.prompts.map((p) => p.root))
}

/** True if any trace node behind this box changed in the update. */
export function needsDetailRefetch(node: DiagramNode, changed: readonly string[]): boolean {
  return node.trace_ids.some((id) => changed.includes(id))
}

/** Indexes of prompts that are in `next` but were not in `previous`. */
export function newPromptIndexes(previous: SessionDiagram | null, next: SessionDiagram): number[] {
  if (previous === null) return []
  const known = new Set(previous.prompts.map((p) => p.turn_id))
  return next.prompts.filter((p) => !known.has(p.turn_id)).map((p) => p.index)
}

/** A message for the header, or null when there is nothing to say. */
export function statusMessage(status: LiveStatus | null): string | null {
  switch (status) {
    case 'no_watcher':
      return 'File watcher unavailable: this session still updates, but the session list needs Refresh.'
    case 'deleted':
      return 'The transcript file no longer exists. Showing the last version read.'
    default:
      return null
  }
}
