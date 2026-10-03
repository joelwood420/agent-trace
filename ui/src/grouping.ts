// Groups the session list by project for the sidebar. Pure, no DOM.

import type { SessionSummary } from './types.ts'

export interface ProjectGroup {
  project: string
  label: string
  sessions: SessionSummary[]
}

/**
 * Sessions grouped by project folder. Groups are ordered by their newest
 * session, and sessions inside a group newest first. A text filter matches
 * the title, session id or project label, ignoring case.
 */
export function groupSessions(sessions: SessionSummary[], filter = ''): ProjectGroup[] {
  const needle = filter.trim().toLowerCase()
  const matches = (s: SessionSummary) =>
    needle === '' ||
    (s.title ?? '').toLowerCase().includes(needle) ||
    s.session_id.toLowerCase().includes(needle) ||
    s.project_label.toLowerCase().includes(needle)

  const groups = new Map<string, ProjectGroup>()
  const sorted = sessions.filter(matches).sort((a, b) => b.modified_ms - a.modified_ms)
  for (const s of sorted) {
    let group = groups.get(s.project)
    if (!group) {
      group = { project: s.project, label: s.project_label, sessions: [] }
      groups.set(s.project, group)
    }
    group.sessions.push(s)
  }
  // Map keeps insertion order, which is newest session first.
  return [...groups.values()]
}
