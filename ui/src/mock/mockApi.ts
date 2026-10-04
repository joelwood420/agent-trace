// Dev-only mock of the backend commands, for viewing the UI in a normal
// browser. It replays the responses for the sanitised fixture session from
// fixture-data.json, which a Rust test keeps identical to what the real
// commands return. Never imported in production builds (see api.ts).
//
// URL flags (combine with `?mock`):
//   delay=<ms>   wait this long before each response (default 250)
//   fail=list|load|detail   make that command fail, to see error handling
//   live         replay the session growing (ui/src/mock/live-steps.json),
//                one step every `step` ms (default 1500)
//   fail=live    with `live`, end the replay with a "deleted" status

import type { Api } from '../api.ts'
import type { LiveMessage, LiveStatus, NodeDetail, SessionSummary, SessionView } from '../types.ts'
import data from './fixture-data.json'
import liveSteps from './live-steps.json'

interface MockData {
  sessions: SessionSummary[]
  views: Record<string, SessionView>
  details: Record<string, NodeDetail>
}

const mock = data as unknown as MockData
const steps = liveSteps as unknown as LiveMessage[]

export function createMockApi(params: URLSearchParams): Api {
  const delay = Number(params.get('delay') ?? '250')
  const fail = params.get('fail')
  const live = params.has('live')
  const parsedStep = Number(params.get('step') ?? '1500')
  const stepMs = Number.isFinite(parsedStep) && parsedStep > 0 ? parsedStep : 1500
  const wait = () => new Promise((resolve) => setTimeout(resolve, Number.isFinite(delay) ? delay : 0))

  let timer: ReturnType<typeof setInterval> | null = null
  let onListChange: (() => void) | null = null
  const stopReplay = () => {
    if (timer !== null) clearInterval(timer)
    timer = null
  }

  return {
    async listSessions() {
      await wait()
      if (fail === 'list') throw 'could not read the projects folder: mock failure'
      // Make the fixture look recent so relative times are realistic.
      const now = Date.now()
      return mock.sessions.map((s, i) => ({
        ...s,
        modified_ms: now - (i + 1) * 3_600_000,
        live: live && i === 0,
      }))
    },
    async loadSession(project, sessionId, onMessage) {
      stopReplay()
      await wait()
      if (fail === 'load') throw 'could not load session: mock failure'
      const view = mock.views[`${project}/${sessionId}`]
      if (!view) throw 'session not found'
      const status = (value: LiveStatus): LiveMessage => ({
        type: 'status',
        project,
        session_id: sessionId,
        status: value,
      })
      // Sent after the reply, like the real backend's first status message.
      setTimeout(() => onMessage(status('watching')), 0)

      const first = steps[0]
      if (!live || first?.type !== 'updated' || first.project !== project || first.session_id !== sessionId) {
        return view
      }
      let next = 1
      timer = setInterval(() => {
        const step = steps[next]
        if (step) {
          onMessage(step)
          next += 1
          if (next % 3 === 0) onListChange?.()
        }
        if (next >= steps.length) {
          stopReplay()
          if (fail === 'live') onMessage(status('deleted'))
        }
      }, stepMs)
      return first.view
    },
    async nodeDetail(_project, _sessionId, traceId) {
      await wait()
      if (fail === 'detail') throw 'could not load session: mock failure'
      return mock.details[traceId] ?? null
    },
    async watchSessions(onChange) {
      await wait()
      onListChange = onChange
    },
  }
}
