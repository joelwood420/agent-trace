// The backend commands. In the app they go through Tauri's `invoke`, and live
// updates arrive over Tauri channels.
// In development only, opening the UI in a normal browser with `?mock` in the
// URL replays responses for the sanitised fixture instead (see mock/).

import { Channel, invoke } from '@tauri-apps/api/core'

import type {
  CaptureDetail,
  CaptureOverview,
  CaptureStatus,
  ContextBreakdown,
  LiveMessage,
  NodeDetail,
  SessionContext,
  SessionSummary,
  SessionView,
} from './types.ts'

export interface Api {
  listSessions(): Promise<SessionSummary[]>
  /** Loads a session and keeps it live: later changes arrive through `onMessage`. */
  loadSession(project: string, sessionId: string, onMessage: (message: LiveMessage) => void): Promise<SessionView>
  nodeDetail(project: string, sessionId: string, traceId: string): Promise<NodeDetail | null>
  /** Calls `onChange` whenever the session list may have changed. */
  watchSessions(onChange: () => void): Promise<void>
  captureStatus(): Promise<CaptureStatus>
  sessionCaptures(project: string, sessionId: string): Promise<CaptureOverview>
  captureDetail(project: string, sessionId: string, captureId: string): Promise<CaptureDetail | null>
  deleteCaptures(project: string, sessionId: string): Promise<void>
  sessionContext(project: string, sessionId: string): Promise<SessionContext>
  callContext(project: string, sessionId: string, traceId: string): Promise<ContextBreakdown | null>
}

const tauriApi: Api = {
  listSessions: () => invoke<SessionSummary[]>('list_sessions'),
  loadSession: (project, sessionId, onMessage) => {
    const channel = new Channel<LiveMessage>()
    channel.onmessage = onMessage
    return invoke<SessionView>('load_session', { project, session_id: sessionId, on_update: channel })
  },
  watchSessions: (onChange) => {
    const channel = new Channel<Record<string, never>>()
    channel.onmessage = () => onChange()
    return invoke<void>('watch_sessions', { on_change: channel })
  },
  nodeDetail: (project, sessionId, traceId) =>
    invoke<NodeDetail | null>('node_detail', {
      project,
      session_id: sessionId,
      trace_id: traceId,
    }),
  captureStatus: () => invoke<CaptureStatus>('capture_status'),
  sessionCaptures: (project, sessionId) =>
    invoke<CaptureOverview>('session_captures', { project, session_id: sessionId }),
  captureDetail: (project, sessionId, captureId) =>
    invoke<CaptureDetail | null>('capture_detail', {
      project,
      session_id: sessionId,
      capture_id: captureId,
    }),
  deleteCaptures: (project, sessionId) =>
    invoke<void>('delete_captures', { project, session_id: sessionId }),
  sessionContext: (project, sessionId) => invoke<SessionContext>('session_context', { project, session_id: sessionId }),
  callContext: (project, sessionId, traceId) =>
    invoke<ContextBreakdown | null>('call_context', { project, session_id: sessionId, trace_id: traceId }),
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
