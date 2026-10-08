// What fills a model call's context: the full breakdown for the details panel
// and the short card for the session sidebar. Numbers come from Rust.

import { useEffect, useState } from 'react'

import { errorMessage, type Api } from '../api.ts'
import { fromTranscriptTag, hiddenContextRows, SLICE_NAMES, topSlices, totalNote } from '../context.ts'
import type { Advice, ContextBar as Bar, ContextBreakdown, HiddenPart, SessionContext } from '../types.ts'
import ContextBar from './ContextBar.tsx'
import type { Loadable } from './Sidebar.tsx'

/** The bar of a breakdown, built from its slices (the same shape the bars use). */
function barOf(b: ContextBreakdown): Bar {
  return {
    source: b.source,
    total_tokens: b.total_tokens,
    total_is_reported: b.total_is_reported,
    slices: b.slices.map((s) => ({ kind: s.kind, tokens: s.tokens })),
  }
}

function percent(share: number): string {
  return `${Math.round(share * 100)}%`
}

function AdviceList({ advice }: { advice: Advice[] }) {
  if (advice.length === 0) return null
  return (
    <ul className="context-advice">
      {advice.map((a, i) => (
        <li key={i} className={a.level === 'warn' ? 'context-advice-warn' : undefined}>
          {a.level === 'warn' && (
            <span aria-hidden="true" title="Warning">
              {'⚠'}{' '}
            </span>
          )}
          {a.text}
        </li>
      ))}
    </ul>
  )
}

/** The full breakdown: bar, legend, items, advice and the note on the total. */
export function ContextDetails({ breakdown }: { breakdown: ContextBreakdown }) {
  return (
    <div className="context-details">
      <ContextBar bar={barOf(breakdown)} height={12} />
      <table className="context-legend">
        <tbody>
          {breakdown.slices.map((s) => (
            <tr key={s.kind}>
              <td>
                <span className={`context-swatch context-slice-${s.kind}`} aria-hidden="true" />
              </td>
              <td>{SLICE_NAMES[s.kind]}</td>
              <td className="num">{s.tokens.toLocaleString('en-US')}</td>
              <td className="num">{percent(s.share)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {breakdown.slices
        .filter((s) => s.items.length > 0)
        .map((s) => (
          <details key={s.kind} className="context-items">
            <summary>
              {SLICE_NAMES[s.kind]} ({s.items.length})
            </summary>
            <ul>
              {s.items.map((item, i) => (
                <li key={i}>
                  <span className="context-item-label" title={item.label}>
                    {item.label}
                  </span>
                  <span className="muted">
                    {fromTranscriptTag(item) && <span className="tag tag-small">{fromTranscriptTag(item)}</span>}{' '}
                    {item.tokens.toLocaleString('en-US')}
                    {item.count > 1 ? ` ×${item.count}` : ''}
                  </span>
                </li>
              ))}
            </ul>
          </details>
        ))}
      <AdviceList advice={breakdown.advice} />
      <p className="muted small">{totalNote(breakdown)}</p>
    </div>
  )
}

interface CallProps {
  api: Api
  project: string
  sessionId: string
  traceId: string
  /** Goes up when the call changed in a live update, to refetch. */
  refreshKey: number
}

type Fetched =
  | { key: string; status: 'ready'; value: ContextBreakdown | null }
  | { key: string; status: 'error'; message: string }

/** The "Context" section of a model call in the details panel. */
export function CallContextSection({ api, project, sessionId, traceId, refreshKey }: CallProps) {
  const key = `${project}/${sessionId}/${traceId}`
  const [fetched, setFetched] = useState<Fetched | null>(null)

  useEffect(() => {
    let cancelled = false
    api
      .callContext(project, sessionId, traceId)
      .then((value) => {
        if (!cancelled) setFetched({ key, status: 'ready', value })
      })
      .catch((err: unknown) => {
        if (!cancelled) setFetched({ key, status: 'error', message: errorMessage(err) })
      })
    return () => {
      cancelled = true
    }
  }, [api, project, sessionId, traceId, refreshKey, key])

  const current = fetched?.key === key ? fetched : null
  return (
    <section className="detail-section">
      <h3>Context</h3>
      {current === null && <p className="muted small loading">Loading context...</p>}
      {current?.status === 'error' && (
        <div className="notice notice-error" role="alert">
          Could not load the context: {current.message}
        </div>
      )}
      {current?.status === 'ready' &&
        (current.value ? (
          <ContextDetails breakdown={current.value} />
        ) : (
          <p className="muted small">No token counts for this call yet.</p>
        ))}
    </section>
  )
}

interface CardProps {
  context: Loadable<SessionContext>
  onSelect: (traceId: string) => void
  /** Opens the details of a hidden context part, which has no diagram box. */
  onSelectHidden: (traceId: string, label: string) => void
  selectedHiddenId: string | null
  onRetry: () => void
}

/** The session overview card: the latest call's context in short. */
export function ContextCard({ context, onSelect, onSelectHidden, selectedHiddenId, onRetry }: CardProps) {
  return (
    <section className="context-card" aria-label="Context">
      <h3>Context</h3>
      {context.status === 'loading' && <p className="muted small loading">Measuring context...</p>}
      {context.status === 'error' && (
        <div className="notice notice-error" role="alert">
          <p>Could not measure the context: {context.message}</p>
          <button type="button" onClick={onRetry}>
            Retry
          </button>
        </div>
      )}
      {context.status === 'ready' &&
        (context.value.latest === null ? (
          <p className="muted small">No model calls with token counts yet.</p>
        ) : (
          <>
            <ContextBar bar={barOf(context.value.latest)} height={10} label="Latest model call" />
            <ul className="context-top">
              {topSlices(context.value.latest, 3).map((s) => (
                <li key={s.kind}>
                  {SLICE_NAMES[s.kind]} {percent(s.share)}
                </li>
              ))}
            </ul>
            <AdviceList advice={context.value.latest.advice} />
            <p className="muted small">{totalNote(context.value.latest)}</p>
            {context.value.latest_trace_id !== null && (
              <button type="button" onClick={() => onSelect(context.value.latest_trace_id ?? '')}>
                Show this call
              </button>
            )}
          </>
        ))}
      {context.status === 'ready' && (
        <HiddenContextList
          parts={context.value.hidden_context}
          selectedId={selectedHiddenId}
          onSelect={onSelectHidden}
        />
      )}
    </section>
  )
}

/** The hidden context Claude Code recorded, one row per part. */
function HiddenContextList({
  parts,
  selectedId,
  onSelect,
}: {
  parts: HiddenPart[]
  selectedId: string | null
  onSelect: (traceId: string, label: string) => void
}) {
  const rows = hiddenContextRows(parts)
  if (rows.length === 0) return null
  return (
    <details className="hidden-context">
      <summary>Hidden context ({rows.length})</summary>
      <ul className="item-list">
        {rows.map((r) => (
          <li key={r.traceId}>
            <button
              type="button"
              className={`item item-compact${r.traceId === selectedId ? ' item-selected' : ''}`}
              onClick={() => onSelect(r.traceId, r.label)}
            >
              <span className="item-title" title={r.label}>
                {r.label}
              </span>
              <span className="item-meta">
                {r.kindName} - {r.size}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </details>
  )
}
