// Dev-only mock of the backend commands, for viewing the UI in a normal
// browser. It replays the responses for the sanitised fixture session from
// fixture-data.json, which a Rust test keeps identical to what the real
// commands return. Never imported in production builds (see api.ts).
//
// URL flags (combine with `?mock`):
//   delay=<ms>   wait this long before each response (default 250)
//   fail=list|load|detail|captures   make that command fail, to see error handling
//   live         replay the session growing (ui/src/mock/live-steps.json),
//                one step every `step` ms (default 1500)
//   fail=live    with `live`, end the replay with a "deleted" status
// Captures come from capture-data.json (fixture session only); deleting them
// empties the overview until the page is reloaded.

import type { Api } from '../api.ts'
import type {
  CaptureDetail,
  CaptureOverview,
  LiveMessage,
  LiveStatus,
  NodeDetail,
  SessionSummary,
  SessionView,
} from '../types.ts'
import captureData from './capture-data.json'
import data from './fixture-data.json'
import liveSteps from './live-steps.json'

interface MockData {
  sessions: SessionSummary[]
  views: Record<string, SessionView>
  details: Record<string, NodeDetail>
}

const mock = data as unknown as MockData
const captures = captureData as unknown as {
  overview: CaptureOverview
  details: Record<string, CaptureDetail>
}
const CAPTURE_KEY = 'basic/00000000-0000-4000-8000-000000000002'
const emptyOverview: CaptureOverview = {
  session_key: null,
  total_bytes: 0,
  calls: [],
  system_versions: [],
  tool_versions: [],
  other_call_ids: [],
  skipped: [],
}
const steps = liveSteps as unknown as LiveMessage[]

export function createMockApi(params: URLSearchParams): Api {
  const delay = Number(params.get('delay') ?? '250')
  const fail = params.get('fail')
  const live = params.has('live')
  const parsedStep = Number(params.get('step') ?? '1500')
  const stepMs = Number.isFinite(parsedStep) && parsedStep > 0 ? parsedStep : 1500
  const wait = () => new Promise((resolve) => setTimeout(resolve, Number.isFinite(delay) ? delay : 0))

  let capturesDeleted = false
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
      // An overlapping call may have started a replay while this one waited.
      stopReplay()
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
    async captureStatus() {
      await wait()
      return {
        listening: true,
        port: 47821,
        command: "$env:ANTHROPIC_BASE_URL='http://127.0.0.1:47821'; claude",
        error: null,
        last_save_error: null,
      }
    },
    async sessionCaptures(project, sessionId) {
      await wait()
      if (fail === 'captures') throw 'could not read captures: mock failure'
      if (capturesDeleted || `${project}/${sessionId}` !== CAPTURE_KEY) return emptyOverview
      return captures.overview
    },
    async captureDetail(project, sessionId, captureId) {
      await wait()
      if (capturesDeleted || `${project}/${sessionId}` !== CAPTURE_KEY) return null
      return captures.details[captureId] ?? null
    },
    async deleteCaptures() {
      await wait()
      capturesDeleted = true
    },
  }
}
