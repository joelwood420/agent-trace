// The three backend commands. In the app they go through Tauri's `invoke`.
// In development only, opening the UI in a normal browser with `?mock` in the
// URL replays responses for the sanitised fixture instead (see mock/).

import { invoke } from '@tauri-apps/api/core'

import type { NodeDetail, SessionSummary, SessionView } from './types.ts'

export interface Api {
  listSessions(): Promise<SessionSummary[]>
  loadSession(project: string, sessionId: string): Promise<SessionView>
  nodeDetail(project: string, sessionId: string, traceId: string): Promise<NodeDetail | null>
}

const tauriApi: Api = {
  listSessions: () => invoke<SessionSummary[]>('list_sessions'),
  loadSession: (project, sessionId) =>
    invoke<SessionView>('load_session', { project, session_id: sessionId }),
  nodeDetail: (project, sessionId, traceId) =>
    invoke<NodeDetail | null>('node_detail', {
      project,
      session_id: sessionId,
      trace_id: traceId,
    }),
}

/** True when running inside the Tauri app window. */
export function insideTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

/** True when the dev-only mock mode is on. Always false in production builds. */
export function mockMode(): boolean {
  return import.meta.env.DEV && new URLSearchParams(window.location.search).has('mock')
}

let chosen: Promise<Api> | null = null

/** The API to use: the mock in dev mock mode, else the Tauri commands. */
export function getApi(): Promise<Api> {
  if (chosen) return chosen
  if (import.meta.env.DEV && mockMode()) {
    // `import.meta.env.DEV` is replaced with `false` in production builds, so
    // this import and the fixture data are left out of the bundle.
    chosen = import('./mock/mockApi.ts').then((m) =>
      m.createMockApi(new URLSearchParams(window.location.search)),
    )
  } else {
    chosen = Promise.resolve(tauriApi)
  }
  return chosen
}

/** A readable message from whatever a failed command threw. */
export function errorMessage(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error) return err.message
  return String(err)
}
