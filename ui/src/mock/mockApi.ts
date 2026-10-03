// Dev-only mock of the backend commands, for viewing the UI in a normal
// browser. It replays the responses for the sanitised fixture session from
// fixture-data.json, which a Rust test keeps identical to what the real
// commands return. Never imported in production builds (see api.ts).
//
// URL flags (combine with `?mock`):
//   delay=<ms>   wait this long before each response (default 250)
//   fail=list|load|detail   make that command fail, to see error handling

import type { Api } from '../api.ts'
import type { NodeDetail, SessionSummary, SessionView } from '../types.ts'
import data from './fixture-data.json'

interface MockData {
  sessions: SessionSummary[]
  views: Record<string, SessionView>
  details: Record<string, NodeDetail>
}

const mock = data as unknown as MockData

export function createMockApi(params: URLSearchParams): Api {
  const delay = Number(params.get('delay') ?? '250')
  const fail = params.get('fail')
  const wait = () => new Promise((resolve) => setTimeout(resolve, Number.isFinite(delay) ? delay : 0))

  return {
    async listSessions() {
      await wait()
      if (fail === 'list') throw 'could not read the projects folder: mock failure'
      // Make the fixture look recent so relative times are realistic.
      const now = Date.now()
      return mock.sessions.map((s, i) => ({ ...s, modified_ms: now - (i + 1) * 3_600_000 }))
    },
    async loadSession(project, sessionId) {
      await wait()
      if (fail === 'load') throw 'could not load session: mock failure'
      const view = mock.views[`${project}/${sessionId}`]
      if (!view) throw 'session not found'
      return view
    },
    async nodeDetail(_project, _sessionId, traceId) {
      await wait()
      if (fail === 'detail') throw 'could not load session: mock failure'
      return mock.details[traceId] ?? null
    },
  }
}
