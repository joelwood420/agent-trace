import { useCallback, useEffect, useRef, useState } from 'react'

import { errorMessage, getApi, insideTauri, mockMode, type Api } from './api.ts'
import DetailsPanel from './components/DetailsPanel.tsx'
import Diagram from './components/Diagram.tsx'
import { SessionList, SessionPanel, type Loadable } from './components/Sidebar.tsx'
import { formatDuration, formatTokens, plural } from './format.ts'
import { collapsibleIds } from './layout.ts'
import { findNode, isForSession, isNewer, keepPromptIndex, needsDetailRefetch, newPromptIndexes } from './live.ts'
import type { DiagramNode, LiveMessage, LiveStatus, SessionSummary, SessionView } from './types.ts'

/** How long a prompt that arrived live stays highlighted. */
const NEW_PROMPT_MS = 4000

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
  const [liveStatus, setLiveStatus] = useState<LiveStatus | null>(null)
  const [newPrompts, setNewPrompts] = useState<ReadonlySet<number>>(new Set())
  // Goes up when the selected box's trace nodes changed, so the details refetch.
  const [detailRefresh, setDetailRefresh] = useState(0)
  // Ignores responses to requests that were overtaken by a newer one.
  const loadToken = useRef(0)
  // The view on screen, read by the live handler to drop older versions.
  const viewRef = useRef<SessionView | null>(null)
  // The selected box, read by the live handler to decide on a details refetch.
  const selectedRef = useRef<DiagramNode | null>(null)
  // Timers that end the highlight of new prompts.
  const highlightTimers = useRef(new Set<ReturnType<typeof setTimeout>>())

  useEffect(() => {
    selectedRef.current = selected
  }, [selected])

  const clearHighlights = useCallback(() => {
    for (const timer of highlightTimers.current) clearTimeout(timer)
    highlightTimers.current.clear()
    setNewPrompts(new Set())
  }, [])

  useEffect(() => {
    const timers = highlightTimers.current
    return () => {
      for (const timer of timers) clearTimeout(timer)
    }
  }, [])

  const listSessions = useCallback((which: Api) => {
    which
      .listSessions()
      .then((value) => {
        setSessions({ status: 'ready', value })
        setListedAt(Date.now())
        // Keep the open session's title and live flag current.
        setSession((current) => {
          if (current === null) return null
          const fresh = value.find((s) => s.project === current.project && s.session_id === current.session_id)
          return fresh ?? current
        })
      })
      .catch((err: unknown) => setSessions({ status: 'error', message: errorMessage(err) }))
  }, [])

  useEffect(() => {
    if (!insideTauri() && !mockMode()) return
    getApi()
      .then((chosen) => {
        setApi(chosen)
        listSessions(chosen)
        chosen
          .watchSessions(() => listSessions(chosen))
          .catch((err: unknown) => console.warn('Could not watch the session list:', errorMessage(err)))
      })
      .catch((err: unknown) => setSessions({ status: 'error', message: errorMessage(err) }))
  }, [listSessions])

  /** Shows a view unless an equal or newer version is already on screen. */
  const showView = useCallback((value: SessionView) => {
    if (!isNewer(viewRef.current, value)) return false
    viewRef.current = value
    setView({ status: 'ready', value })
    setPromptIndex((current) => keepPromptIndex(current, value.diagram.prompts.length))
    return true
  }, [])

  const highlightPrompts = useCallback((indexes: number[]) => {
    if (indexes.length === 0) return
    setNewPrompts((prev) => new Set([...prev, ...indexes]))
    for (const index of indexes) {
      const timer = setTimeout(() => {
        highlightTimers.current.delete(timer)
        setNewPrompts((prev) => {
          const next = new Set(prev)
          next.delete(index)
          return next
        })
      }, NEW_PROMPT_MS)
      highlightTimers.current.add(timer)
    }
  }, [])

  /** Applies a message pushed by the backend while session `s` is open. */
  const handleLive = useCallback(
    (token: number, s: SessionSummary, message: LiveMessage) => {
      if (token !== loadToken.current || !isForSession(message, s)) return
      if (message.type === 'status') {
        setLiveStatus(message.status)
        return
      }
      const previous = viewRef.current
      const next = message.view
      if (!showView(next)) return
      highlightPrompts(newPromptIndexes(previous?.diagram ?? null, next.diagram))
      // The selected box may be gone (the details close) or may have changed.
      setSelected((sel) => (sel ? findNode(next.diagram, sel.id) : null))
      const sel = selectedRef.current
      const updated = sel ? findNode(next.diagram, sel.id) : null
      if (updated && needsDetailRefetch(updated, message.changed_trace_ids)) {
        setDetailRefresh((n) => n + 1)
      }
    },
    [showView, highlightPrompts],
  )

  const refreshSessions = () => {
    if (!api) return
    setSessions({ status: 'loading' })
    listSessions(api)
  }

  const openSession = (s: SessionSummary, keepPrompt = false) => {
    if (!api) return
    const token = ++loadToken.current
    // Every load starts again at version 1, so forget the old view.
    viewRef.current = null
    setSession(s)
    setView({ status: 'loading' })
    setSelected(null)
    setLiveStatus(null)
    clearHighlights()
    if (!keepPrompt) {
      setPromptIndex(null)
      setOpenState(new Map())
    }
    api
      .loadSession(s.project, s.session_id, (message) => handleLive(token, s, message))
      .then((value) => {
        if (token !== loadToken.current) return
        showView(value)
      })
      .catch((err: unknown) => {
        if (token !== loadToken.current) return
        setView({ status: 'error', message: errorMessage(err) })
      })
  }

  const closeSession = () => {
    loadToken.current++
    viewRef.current = null
    setSession(null)
    setSelected(null)
    setPromptIndex(null)
    setLiveStatus(null)
    clearHighlights()
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
            liveStatus={liveStatus}
            newPrompts={newPrompts}
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
          refreshKey={detailRefresh}
          onClose={() => setSelected(null)}
        />
      )}
    </div>
  )
}
