import { useCallback, useEffect, useRef, useState } from 'react'

import { errorMessage, getApi, insideTauri, mockMode, type Api } from './api.ts'
import DetailsPanel from './components/DetailsPanel.tsx'
import Diagram from './components/Diagram.tsx'
import { SessionList, SessionPanel, type Loadable } from './components/Sidebar.tsx'
import { formatDuration, formatTokens, plural } from './format.ts'
import { collapsibleIds } from './layout.ts'
import type { DiagramNode, SessionSummary, SessionView } from './types.ts'

const NOT_IN_APP =
  'This page is the Snitchcraft user interface and needs the desktop app to read sessions. ' +
  'Start it with "cargo tauri dev". During development you can add ?mock to the URL to view the sample session instead.'

export default function App() {
  const [api, setApi] = useState<Api | null>(null)
  const [sessions, setSessions] = useState<Loadable<SessionSummary[]>>(() =>
    insideTauri() || mockMode() ? { status: 'loading' } : { status: 'error', message: 'not running inside the Snitchcraft app' },
  )
  const [listedAt, setListedAt] = useState(0)
  const [session, setSession] = useState<SessionSummary | null>(null)
  const [view, setView] = useState<Loadable<SessionView>>({ status: 'loading' })
  const [promptIndex, setPromptIndex] = useState<number | null>(null)
  const [openState, setOpenState] = useState<ReadonlyMap<string, boolean>>(new Map())
  const [selected, setSelected] = useState<DiagramNode | null>(null)
  // Ignores responses to requests that were overtaken by a newer one.
  const loadToken = useRef(0)

  const listSessions = useCallback((which: Api) => {
    which
      .listSessions()
      .then((value) => {
        setSessions({ status: 'ready', value })
        setListedAt(Date.now())
      })
      .catch((err: unknown) => setSessions({ status: 'error', message: errorMessage(err) }))
  }, [])

  useEffect(() => {
    if (!insideTauri() && !mockMode()) return
    getApi()
      .then((chosen) => {
        setApi(chosen)
        listSessions(chosen)
      })
      .catch((err: unknown) => setSessions({ status: 'error', message: errorMessage(err) }))
  }, [listSessions])

  const refreshSessions = () => {
    if (!api) return
    setSessions({ status: 'loading' })
    listSessions(api)
  }

  const openSession = (s: SessionSummary, keepPrompt = false) => {
    if (!api) return
    const token = ++loadToken.current
    setSession(s)
    setView({ status: 'loading' })
    setSelected(null)
    if (!keepPrompt) {
      setPromptIndex(null)
      setOpenState(new Map())
    }
    api
      .loadSession(s.project, s.session_id)
      .then((value) => {
        if (token !== loadToken.current) return
        setView({ status: 'ready', value })
        setPromptIndex((current) => {
          const count = value.diagram.prompts.length
          if (count === 0) return null
          return current !== null && current < count ? current : 0
        })
      })
      .catch((err: unknown) => {
        if (token !== loadToken.current) return
        setView({ status: 'error', message: errorMessage(err) })
      })
  }

  const closeSession = () => {
    loadToken.current++
    setSession(null)
    setSelected(null)
    setPromptIndex(null)
  }

  const selectPrompt = (index: number) => {
    setPromptIndex(index)
    setSelected(null)
  }

  const onToggle = useCallback((id: string, open: boolean) => {
    setOpenState((prev) => new Map(prev).set(id, open))
  }, [])

  const prompt =
    view.status === 'ready' && promptIndex !== null ? (view.value.diagram.prompts[promptIndex] ?? null) : null

  const setAll = (open: boolean) => {
    if (!prompt) return
    setOpenState((prev) => {
      const next = new Map(prev)
      for (const id of collapsibleIds(prompt.root)) {
        if (open) next.set(id, true)
        else next.delete(id)
      }
      return next
    })
  }

  return (
    <div className={`app${selected ? ' app-with-details' : ''}`}>
      <header className="app-header">
        <h1>Snitchcraft</h1>
        <p className="tagline">snitches get traces</p>
        {mockMode() && <span className="mock-badge">Mock data from the sample fixture</span>}
      </header>

      <aside className="sidebar">
        {session === null ? (
          <SessionList sessions={sessions} now={listedAt} onOpen={(s) => openSession(s)} onRetry={refreshSessions} />
        ) : (
          <SessionPanel
            session={session}
            view={view}
            selectedPrompt={promptIndex}
            selectedNodeId={selected?.id ?? null}
            onBack={closeSession}
            onReload={() => openSession(session, true)}
            onSelectPrompt={selectPrompt}
            onSelectMarker={setSelected}
          />
        )}
      </aside>

      <main className="main">
        {prompt && session ? (
          <>
            <div className="toolbar">
              <div className="toolbar-title">
                <span className="tag">Prompt {prompt.index + 1}</span>
                <span className="toolbar-preview" title={prompt.prompt_preview}>
                  {prompt.prompt_preview}
                </span>
              </div>
              <div className="toolbar-meta muted small">
                {[
                  formatDuration(prompt.duration_ms),
                  plural(prompt.totals.model_calls, 'model call'),
                  plural(prompt.totals.tool_calls, 'tool call'),
                  prompt.totals.errors > 0 ? plural(prompt.totals.errors, 'error') : null,
                  prompt.totals.max_context_tokens !== null
                    ? `max ${formatTokens(prompt.totals.max_context_tokens)} ctx`
                    : null,
                  prompt.totals.output_tokens !== null ? `${formatTokens(prompt.totals.output_tokens)} out` : null,
                ]
                  .filter(Boolean)
                  .join(' - ')}
              </div>
              <div className="toolbar-actions">
                <button type="button" onClick={() => setAll(true)}>
                  Expand all
                </button>
                <button type="button" onClick={() => setAll(false)}>
                  Collapse all
                </button>
              </div>
            </div>
            <Diagram
              key={`${session.project}/${session.session_id}/${prompt.turn_id}`}
              prompt={prompt}
              openState={openState}
              selectedId={selected?.id ?? null}
              onToggle={onToggle}
              onSelect={setSelected}
            />
          </>
        ) : (
          <div className="empty">
            {session === null && !insideTauri() && !mockMode() && (
              <p>{NOT_IN_APP}</p>
            )}
            {session === null && sessions.status !== 'error' && <p>Choose a session on the left.</p>}
            {session !== null && view.status === 'loading' && <p className="loading">Loading session...</p>}
            {session !== null && view.status === 'error' && <p>The session could not be loaded. See the message on the left.</p>}
            {session !== null && view.status === 'ready' && view.value.diagram.prompts.length === 0 && (
              <p>This session has no prompts to draw.</p>
            )}
            {session !== null && view.status === 'ready' && view.value.diagram.prompts.length > 0 && (
              <p>Choose a prompt on the left.</p>
            )}
          </div>
        )}
      </main>

      {selected && session && api && (
        <DetailsPanel
          api={api}
          project={session.project}
          sessionId={session.session_id}
          node={selected}
          onClose={() => setSelected(null)}
        />
      )}
    </div>
  )
}
