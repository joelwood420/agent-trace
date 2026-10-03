import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

/** One session file, as returned by the `list_sessions` command. */
export interface SessionSummary {
  project: string
  project_label: string
  session_id: string
  title: string | null
  modified_ms: number
  size_bytes: number
}

type State =
  | { status: 'loading' }
  | { status: 'ready'; sessions: SessionSummary[] }
  | { status: 'error'; message: string }

// Placeholder screen for M2 step 1: proves the webview can call the backend.
export default function App() {
  const [state, setState] = useState<State>({ status: 'loading' })

  useEffect(() => {
    invoke<SessionSummary[]>('list_sessions')
      .then((sessions) => setState({ status: 'ready', sessions }))
      .catch((err: unknown) => setState({ status: 'error', message: String(err) }))
  }, [])

  return (
    <main>
      <h1>looptrace</h1>
      {state.status === 'loading' && <p>Looking for sessions...</p>}
      {state.status === 'ready' && <p>Found {state.sessions.length} sessions.</p>}
      {state.status === 'error' && <p role="alert">Could not list sessions: {state.message}</p>}
    </main>
  )
}
