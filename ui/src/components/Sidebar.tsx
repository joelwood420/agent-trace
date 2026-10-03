// The left sidebar: the session list, or, once a session is open, its
// prompts, session-level markers and any skipped lines.

import { useMemo, useState } from 'react'

import { formatBytes, formatDuration, formatRelative, formatTokens, plural, shortId } from '../format.ts'
import { groupSessions } from '../grouping.ts'
import type { DiagramNode, SessionSummary, SessionView } from '../types.ts'

export type Loadable<T> =
  | { status: 'loading' }
  | { status: 'ready'; value: T }
  | { status: 'error'; message: string }

interface SessionListProps {
  sessions: Loadable<SessionSummary[]>
  /** When the list was read, for relative times. */
  now: number
  onOpen: (session: SessionSummary) => void
  onRetry: () => void
}

export function SessionList({ sessions, now, onOpen, onRetry }: SessionListProps) {
  const [filter, setFilter] = useState('')
  const groups = useMemo(
    () => (sessions.status === 'ready' ? groupSessions(sessions.value, filter) : []),
    [sessions, filter],
  )

  return (
    <nav className="sidebar-section" aria-label="Sessions">
      <div className="sidebar-heading">
        <h2>Sessions</h2>
        <button type="button" className="link-button" onClick={onRetry} title="Read the session list again">
          Refresh
        </button>
      </div>
      {sessions.status === 'loading' && <p className="muted loading">Looking for sessions...</p>}
      {sessions.status === 'error' && (
        <div className="notice notice-error" role="alert">
          <p>Could not list sessions: {sessions.message}</p>
          <button type="button" onClick={onRetry}>
            Try again
          </button>
        </div>
      )}
      {sessions.status === 'ready' && sessions.value.length === 0 && (
        <p className="muted">
          No sessions found. Claude Code sessions appear here once a transcript exists in the
          projects folder.
        </p>
      )}
      {sessions.status === 'ready' && sessions.value.length > 0 && (
        <>
          <input
            type="search"
            name="session-filter"
            className="filter"
            placeholder="Filter by title, id or project"
            aria-label="Filter sessions"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          {groups.length === 0 && <p className="muted">No session matches the filter.</p>}
          <div className="scroll">
            {groups.map((group) => (
              <section key={group.project} className="project-group">
                <h3 className="project-label" title={group.project}>
                  {group.label}
                </h3>
                <ul className="item-list">
                  {group.sessions.map((s) => (
                    <li key={s.session_id}>
                      <button type="button" className="item" onClick={() => onOpen(s)}>
                        <span className="item-title" title={s.title ?? s.session_id}>
                          {s.title ?? `Session ${shortId(s.session_id)}`}
                        </span>
                        <span className="item-meta">
                          {formatRelative(s.modified_ms, now)} - {formatBytes(s.size_bytes)}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </div>
        </>
      )}
    </nav>
  )
}

interface SessionPanelProps {
  session: SessionSummary
  view: Loadable<SessionView>
  selectedPrompt: number | null
  selectedNodeId: string | null
  onBack: () => void
  onReload: () => void
  onSelectPrompt: (index: number) => void
  onSelectMarker: (marker: DiagramNode) => void
}

export function SessionPanel(props: SessionPanelProps) {
  const { session, view } = props
  return (
    <nav className="sidebar-section" aria-label="Prompts">
      <div className="sidebar-heading">
        <button type="button" className="link-button" onClick={props.onBack}>
          {'←'} All sessions
        </button>
        <button type="button" className="link-button" onClick={props.onReload} title="Read this session again">
          Reload
        </button>
      </div>
      <h2 className="session-title" title={session.title ?? session.session_id}>
        {session.title ?? `Session ${shortId(session.session_id)}`}
      </h2>
      <p className="muted small" title={session.project}>
        {session.project_label} - <span className="mono">{shortId(session.session_id)}</span>
      </p>

      {view.status === 'loading' && <p className="muted loading">Loading session...</p>}
      {view.status === 'error' && (
        <div className="notice notice-error" role="alert">
          <p>Could not load this session: {view.message}</p>
          <button type="button" onClick={props.onReload}>
            Try again
          </button>
        </div>
      )}
      {view.status === 'ready' && <SessionContents {...props} diagramView={view.value} />}
    </nav>
  )
}

function SessionContents({
  diagramView,
  selectedPrompt,
  selectedNodeId,
  onSelectPrompt,
  onSelectMarker,
}: SessionPanelProps & { diagramView: SessionView }) {
  const { diagram, skipped } = diagramView
  const duration = formatDuration(diagram.duration_ms)
  return (
    <>
      <p className="muted small">
        {[diagram.harness, plural(diagram.prompts.length, 'prompt'), duration].filter(Boolean).join(' - ')}
      </p>

      {skipped.length > 0 && (
        <details className="notice notice-warning">
          <summary>
            {plural(skipped.length, 'line')} could not be read and {skipped.length === 1 ? 'was' : 'were'} skipped
          </summary>
          <ul className="skipped-list">
            {skipped.map((s, i) => (
              <li key={i}>
                <span className="mono">
                  {s.source}:{s.line}
                </span>{' '}
                {s.reason}
              </li>
            ))}
          </ul>
        </details>
      )}

      {diagram.markers.length > 0 && (
        <details className="session-markers">
          <summary>{plural(diagram.markers.length, 'event')} before the first prompt</summary>
          <ul className="item-list">
            {diagram.markers.map((m) => (
              <li key={m.id}>
                <button
                  type="button"
                  className={`item item-compact${m.id === selectedNodeId ? ' item-selected' : ''}`}
                  onClick={() => onSelectMarker(m)}
                >
                  <span className="item-title" title={m.label}>
                    {m.label}
                  </span>
                  {m.detail_label && <span className="item-meta">{m.detail_label}</span>}
                </button>
              </li>
            ))}
          </ul>
        </details>
      )}

      {diagram.prompts.length === 0 && <p className="muted">This session has no prompts yet.</p>}
      <ol className="item-list scroll prompt-list">
        {diagram.prompts.map((p) => {
          const t = p.totals
          const parts = [plural(t.model_calls, 'model call'), plural(t.tool_calls, 'tool')]
          const ctx = formatTokens(t.max_context_tokens)
          const out = formatTokens(t.output_tokens)
          return (
            <li key={p.turn_id}>
              <button
                type="button"
                className={`item${p.index === selectedPrompt ? ' item-selected' : ''}`}
                aria-current={p.index === selectedPrompt ? 'true' : undefined}
                onClick={() => onSelectPrompt(p.index)}
              >
                <span className="item-title" title={p.prompt_preview}>
                  <span className="prompt-number">{p.index + 1}.</span> {p.prompt_preview}
                </span>
                <span className="item-meta">
                  {formatDuration(p.duration_ms) ?? 'no timing'} - {parts.join(', ')}
                </span>
                <span className="item-meta">
                  {ctx && <span>max {ctx} ctx</span>}
                  {ctx && out && ' - '}
                  {out && <span>{out} out</span>}
                  {t.errors > 0 && (
                    <span className="badge-error" title={`${plural(t.errors, 'tool call')} failed`}>
                      <span aria-hidden="true">{'✕'}</span> {plural(t.errors, 'error')}
                    </span>
                  )}
                </span>
              </button>
            </li>
          )
        })}
      </ol>
    </>
  )
}
