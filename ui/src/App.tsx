import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { errorMessage, getApi, insideTauri, mockMode, type Api } from './api.ts'
import { captureState, sameLoad, throttleDelay } from './captureView.ts'
import { CaptureDetailsPanel } from './components/CapturePanel.tsx'
import DetailsPanel from './components/DetailsPanel.tsx'
import Diagram from './components/Diagram.tsx'
import { SessionList, SessionPanel, type Loadable } from './components/Sidebar.tsx'
import { formatDuration, formatTokens, plural } from './format.ts'
import { collapsibleIds } from './layout.ts'
import { findByTraceId, findNode, isForSession, isNewer, keepPromptIndex, needsDetailRefetch, newPromptIndexes } from './live.ts'
import type {
  CaptureOverview,
  CaptureStatus,
  DiagramNode,
  LiveMessage,
  LiveStatus,
  SessionContext,
  SessionSummary,
  SessionView,
} from './types.ts'

/** Live updates reload the capture overview at most this often. */
const CAPTURE_RELOAD_MS = 2000

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
  const [captureStatus, setCaptureStatus] = useState<Loadable<CaptureStatus>>({ status: 'loading' })
  const [captureOverview, setCaptureOverview] = useState<Loadable<CaptureOverview>>({ status: 'loading' })
  // A captured call picked in the session overview, shown instead of a box.
  const [selectedCapture, setSelectedCapture] = useState<string | null>(null)
  // What fills each model call's context: bars by trace id and the latest breakdown.
  const [sessionContext, setSessionContext] = useState<Loadable<SessionContext>>({ status: 'loading' })
  // Ignores capture overview responses overtaken by a newer request.
  const captureToken = useRef(0)
  // When the overview was last requested, and a pending throttled reload.
  const lastCaptureLoad = useRef<number | null>(null)
  const captureTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
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

  /**
   * Fetches the session list. A refresh triggered by the backend's change
   * signal (`background`) keeps the last good list if it fails; startup and
   * the Refresh button show the error.
   */
  const listSessions = useCallback((which: Api, background = false) => {
    which
      .captureStatus()
      .then((value) => setCaptureStatus({ status: 'ready', value }))
      .catch((err: unknown) => setCaptureStatus({ status: 'error', message: errorMessage(err) }))
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
      .catch((err: unknown) => {
        if (background) {
          console.warn('Could not refresh the session list:', errorMessage(err))
          return
        }
        setSessions({ status: 'error', message: errorMessage(err) })
      })
  }, [])

  useEffect(() => {
    if (!insideTauri() && !mockMode()) return
    getApi()
      .then((chosen) => {
        setApi(chosen)
        listSessions(chosen)
        chosen
          .watchSessions(() => listSessions(chosen, true))
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

  /** Loads the capture overview of session `s`; `fresh` shows loading first. */
  const loadCaptures = useCallback(
    (s: SessionSummary, fresh = false) => {
      if (!api) return
      const token = ++captureToken.current
      lastCaptureLoad.current = Date.now()
      if (fresh) setCaptureOverview({ status: 'loading' })
      // The context follows the same moments and the same guard.
      if (fresh) setSessionContext({ status: 'loading' })
      api
        .sessionContext(s.project, s.session_id)
        .then((value) => {
          if (token === captureToken.current) setSessionContext({ status: 'ready', value })
        })
        .catch((err: unknown) => {
          if (token === captureToken.current) setSessionContext({ status: 'error', message: errorMessage(err) })
        })
      api
        .sessionCaptures(s.project, s.session_id)
        .then((value) => {
          if (token === captureToken.current) setCaptureOverview({ status: 'ready', value })
        })
        .catch((err: unknown) => {
          if (token === captureToken.current) setCaptureOverview({ status: 'error', message: errorMessage(err) })
        })
    },
    [api],
  )

  const cancelCaptureReload = useCallback(() => {
    if (captureTimer.current !== null) clearTimeout(captureTimer.current)
    captureTimer.current = null
  }, [])

  useEffect(() => cancelCaptureReload, [cancelCaptureReload])

  /**
   * Reloads the overview after a live update of session `s` (opened with
   * load `token`), at most once every CAPTURE_RELOAD_MS.
   */
  const scheduleCaptureReload = useCallback(
    (token: number, s: SessionSummary) => {
      if (captureTimer.current !== null) return
      const delay = throttleDelay(lastCaptureLoad.current, Date.now(), CAPTURE_RELOAD_MS)
      if (delay === 0) {
        loadCaptures(s)
        return
      }
      captureTimer.current = setTimeout(() => {
        captureTimer.current = null
        if (sameLoad(token, loadToken.current)) loadCaptures(s)
      }, delay)
    },
    [loadCaptures],
  )

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
      // Other calls, sizes and versions can change with any update.
      scheduleCaptureReload(token, s)
      // The selected box may be gone (the details close) or may have changed.
      setSelected((sel) => (sel ? findNode(next.diagram, sel.id) : null))
      const sel = selectedRef.current
      const updated = sel ? findNode(next.diagram, sel.id) : null
      if (updated && needsDetailRefetch(updated, message.changed_trace_ids)) {
        setDetailRefresh((n) => n + 1)
      }
    },
    [showView, highlightPrompts, scheduleCaptureReload],
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
    setSelectedCapture(null)
    setLiveStatus(null)
    clearHighlights()
    captureToken.current++
    cancelCaptureReload()
    lastCaptureLoad.current = null
    setCaptureOverview({ status: 'loading' })
    setSessionContext({ status: 'loading' })
    if (!keepPrompt) {
      setPromptIndex(null)
      setOpenState(new Map())
    }
    api
      .loadSession(s.project, s.session_id, (message) => handleLive(token, s, message))
      .then((value) => {
        if (token !== loadToken.current) return
        showView(value)
        loadCaptures(s, true)
      })
      .catch((err: unknown) => {
        if (token !== loadToken.current) return
        setView({ status: 'error', message: errorMessage(err) })
        loadCaptures(s, true)
      })
  }

  const closeSession = () => {
    loadToken.current++
    viewRef.current = null
    captureToken.current++
    cancelCaptureReload()
    setSession(null)
    setSessionContext({ status: 'loading' })
    setSelected(null)
    setSelectedCapture(null)
    setPromptIndex(null)
    setLiveStatus(null)
    clearHighlights()
  }

  const selectPrompt = (index: number) => {
    setPromptIndex(index)
    setSelected(null)
  }

  const selectNode = useCallback((node: DiagramNode) => {
    setSelectedCapture(null)
    setSelected(node)
  }, [])

  /** Selects the model call box with this trace id, as a click on it would. */
  const selectTrace = (traceId: string) => {
    if (view.status !== 'ready') return
    for (const p of view.value.diagram.prompts) {
      const found = findByTraceId(p.root, traceId)
      if (found) {
        setPromptIndex(p.index)
        selectNode(found)
        return
      }
    }
  }

  const selectCapture = (captureId: string) => {
    setSelected(null)
    setSelectedCapture(captureId)
  }

  const deleteCaptures = async () => {
    if (!api || !session) return
    // The user may open another session while the delete runs.
    const token = loadToken.current
    const s = session
    await api.deleteCaptures(s.project, s.session_id)
    if (!sameLoad(token, loadToken.current)) return
    setSelectedCapture(null)
    loadCaptures(s)
  }

  const contextBars = useMemo(
    () => (sessionContext.status === 'ready' ? sessionContext.value.bars : {}),
    [sessionContext],
  )
  const viewValue = view.status === 'ready' ? view.value : null
  const capturedIds = useMemo(() => new Set(viewValue?.captured_trace_ids ?? []), [viewValue])

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
    <div className={`app${selected || selectedCapture ? ' app-with-details' : ''}`}>
      <header className="app-header">
        <h1>Snitchcraft</h1>
        <p className="tagline">snitches get traces</p>
        {mockMode() && <span className="mock-badge">Mock data from the sample fixture</span>}
      </header>

      <aside className="sidebar">
        {session === null ? (
          <SessionList
            sessions={sessions}
            now={listedAt}
            captureStatus={captureStatus}
            onOpen={(s) => openSession(s)}
            onRetry={refreshSessions}
          />
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
            onSelectMarker={selectNode}
            captureOverview={captureOverview}
            sessionContext={sessionContext}
            onSelectTrace={selectTrace}
            captureStatus={captureStatus}
            selectedCaptureId={selectedCapture}
            onSelectCapture={selectCapture}
            onDeleteCaptures={deleteCaptures}
            onReloadCaptures={() => loadCaptures(session, true)}
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
              capturedIds={capturedIds}
              contextBars={contextBars}
              onToggle={onToggle}
              onSelect={selectNode}
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
          capture={captureState(selected, capturedIds, captureOverview)}
          onClose={() => setSelected(null)}
        />
      )}
      {selectedCapture && !selected && session && api && (
        <CaptureDetailsPanel
          api={api}
          project={session.project}
          sessionId={session.session_id}
          captureId={selectedCapture}
          onClose={() => setSelectedCapture(null)}
        />
      )}
    </div>
  )
}
