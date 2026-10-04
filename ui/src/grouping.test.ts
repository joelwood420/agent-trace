import assert from 'node:assert/strict'
import { test } from 'node:test'

import { groupSessions } from './grouping.ts'
import type { SessionSummary } from './types.ts'

function session(project: string, id: string, modified: number, title: string | null = null): SessionSummary {
  return {
    project,
    project_label: project.split('-').pop() ?? project,
    session_id: id,
    title,
    modified_ms: modified,
    size_bytes: 1,
    live: false,
  }
}

test('groups by project, newest group and newest session first', () => {
  const groups = groupSessions([
    session('C--work-alpha', 'a1', 10),
    session('C--work-beta', 'b1', 30),
    session('C--work-alpha', 'a2', 20),
  ])
  assert.deepEqual(
    groups.map((g) => [g.label, g.sessions.map((s) => s.session_id)]),
    [
      ['beta', ['b1']],
      ['alpha', ['a2', 'a1']],
    ],
  )
})

test('filters by title, id or project label', () => {
  const sessions = [
    session('C--work-alpha', 'a1', 10, 'Fix the parser'),
    session('C--work-beta', 'b1', 30, 'Write docs'),
  ]
  assert.deepEqual(
    groupSessions(sessions, 'PARSER').map((g) => g.label),
    ['alpha'],
  )
  assert.deepEqual(
    groupSessions(sessions, 'b1').map((g) => g.label),
    ['beta'],
  )
  assert.deepEqual(
    groupSessions(sessions, 'beta').map((g) => g.label),
    ['beta'],
  )
  assert.equal(groupSessions(sessions, 'nothing').length, 0)
})
