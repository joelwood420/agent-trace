// The Compare view: two runs side by side, as sections of rows the backend
// built, a chart of context per call and the two final messages. The UI only
// formats what `compare_runs` sends; it does no maths on trace data.

import { Fragment, useCallback, useEffect, useRef, useState } from 'react'

import { errorMessage, type Api } from '../api.ts'
import { throttleDelay } from '../captureView.ts'
import {
  chartLines,
  formatDifference,
  formatValue,
  presenceTag,
  runRef,
  sameRequest,
  scopeOptions,
  type CompareRequest,
  visibleSections,
} from '../compare.ts'
import { formatRelative, formatTimestamp, shortId } from '../format.ts'
import type {
  CompareRow,
  CompareSection,
  CompareSide,
  CompareView as CompareResult,
  ContextSource,
  RunSummary,
  SessionSummary,
} from '../types.ts'
import type { Loadable } from './Sidebar.tsx'

/** Live updates refetch the comparison at most this often. */
const REFRESH_MS = 2000

const CHART_WIDTH = 600
const CHART_HEIGHT = 120

const PARTIAL_NOTE = 'Some calls did not report this; the total counts the ones that did'
const SCOPE_MISSING = 'That prompt is no longer in the session; showing the whole session.'

type Slot = 'left' | 'right'

interface Props {
  api: Api
  left: SessionSummary
  right: SessionSummary
  /** Changes when either session changed on disk; the comparison then refetches. */
  refreshKey: string
  /** When the session list was read, for relative times. */
  now: number
  onBack: () => void
}

export default function CompareView({ api, left, right, refreshKey, now, onBack }: Props) {
  const [swapped, setSwapped] = useState(false)
  // Turn ids per session, '' for the whole session.
  const [scopes, setScopes] = useState<{ left: string; right: string }>({ left: '', right: '' })
  const [result, setResult] = useState<Loadable<CompareResult>>({ status: 'loading' })
  // The last comparison that loaded and what it answers, so the header keeps
  // each session's prompts after an error or a swap, and a stale table hides.
  const [lastGood, setLastGood] = useState<{ value: CompareResult; request: CompareRequest } | null>(null)
  const [scopeMissing, setScopeMissing] = useState(false)
  // Ignores responses overtaken by a newer request.
  const token = useRef(0)
  // When the comparison was last requested, and a pending throttled refetch.
  const lastRun = useRef<number | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const slotA: Slot = swapped ? 'right' : 'left'
  const slotB: Slot = swapped ? 'left' : 'right'
  const sessionA = swapped ? right : left
  const sessionB = swapped ? left : right
  const scopeA = scopes[slotA]
  const scopeB = scopes[slotB]

  // Plain strings, so a fresh copy of a session summary (the list refreshes
  // as sessions grow) does not count as a new request.
  const { project: projectA, session_id: idA } = sessionA
  const { project: projectB, session_id: idB } = sessionB

  const load = useCallback(() => {
    const mine = ++token.current
    lastRun.current = Date.now()
    api
      .compareRuns(
        runRef({ project: projectA, session_id: idA }, scopeA),
        runRef({ project: projectB, session_id: idB }, scopeB),
      )
      .then((value) => {
        if (mine !== token.current) return
        // A prompt that is gone was answered with the whole session.
        const answeredA = value.a.scope_missing ? '' : scopeA
        const answeredB = value.b.scope_missing ? '' : scopeB
        const answered = swapped ? { left: answeredB, right: answeredA } : { left: answeredA, right: answeredB }
        setResult({ status: 'ready', value })
        setLastGood({ value, request: { swapped, scopes: answered } })
        if (value.a.scope_missing || value.b.scope_missing) {
          setScopeMissing(true)
          setScopes(answered)
        }
      })
      .catch((err: unknown) => {
        if (mine !== token.current) return
        setResult({ status: 'error', message: errorMessage(err) })
      })
  }, [api, projectA, idA, projectB, idB, scopeA, scopeB, swapped])

  // The latest `load`, for a throttled refetch that fires later.
  const loadRef = useRef(load)
  useEffect(() => {
    loadRef.current = load
  }, [load])

  const cancelTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
  }, [])

  useEffect(() => cancelTimer, [cancelTimer])

  // The first load, and every swap or scope change, fetch at once.
  useEffect(() => {
    cancelTimer()
    load()
  }, [load, cancelTimer])

  // A change on disk refetches, at most once every REFRESH_MS.
  const seenKey = useRef(refreshKey)
  useEffect(() => {
    if (seenKey.current === refreshKey) return
    seenKey.current = refreshKey
    if (timer.current !== null) return
    const delay = throttleDelay(lastRun.current, Date.now(), REFRESH_MS)
    if (delay === 0) {
      loadRef.current()
      return
    }
    timer.current = setTimeout(() => {
      timer.current = null
      loadRef.current()
    }, delay)
  }, [refreshKey])

  const setScope = (slot: Slot, value: string) => {
    setScopeMissing(false)
    setScopes((current) => ({ ...current, [slot]: value }))
  }

  const swap = () => {
    setScopeMissing(false)
    setSwapped((s) => !s)
  }

  // What the last result says about a session, whichever side it was on then.
  const known = (slot: Slot): { side: CompareSide; summary: RunSummary } | null => {
    if (!lastGood) return null
    const wasA = (slot === 'left') !== lastGood.request.swapped
    const { value } = lastGood
    return wasA ? { side: value.a, summary: value.comparison.a } : { side: value.b, summary: value.comparison.b }
  }

  // False while a swap or scope change the user asked for is still loading.
  const current = lastGood !== null && sameRequest(lastGood.request, { swapped, scopes })

  return (
    <div className="compare">
      <div className="toolbar">
        <div className="toolbar-title">
          <span className="toolbar-preview">Compare</span>
        </div>
        <div className="toolbar-actions">
          <button type="button" onClick={onBack}>
            Back to diagram
          </button>
          <button type="button" onClick={swap} title="Swap runs A and B">
            Swap
          </button>
        </div>
      </div>

      <div className="compare-body">
        <div className="compare-header">
          <RunHeader
            letter="A"
            session={sessionA}
            now={now}
            scope={scopeA}
            prompts={known(slotA)?.side.prompts ?? []}
            summary={known(slotA)?.summary ?? null}
            onScope={(value) => setScope(slotA, value)}
          />
          <RunHeader
            letter="B"
            session={sessionB}
            now={now}
            scope={scopeB}
            prompts={known(slotB)?.side.prompts ?? []}
            summary={known(slotB)?.summary ?? null}
            onScope={(value) => setScope(slotB, value)}
          />
        </div>

        {result.status === 'loading' && <p className="muted loading">Comparing the two runs...</p>}
        {result.status === 'error' && (
          <div className="notice notice-error" role="alert">
            <p>Could not compare the runs: {result.message}</p>
            <button type="button" onClick={load}>
              Try again
            </button>
          </div>
        )}
        {scopeMissing && (
          <div className="notice notice-warning" role="status">
            {SCOPE_MISSING}
          </div>
        )}
        {result.status === 'ready' && current && <CompareBody view={result.value} />}
        {result.status === 'ready' && !current && <p className="muted loading">Comparing the two runs...</p>}
      </div>
    </div>
  )
}

function sourceNote(source: ContextSource | null): string {
  if (source === 'captured') return 'Setup from a captured API call'
  if (source === 'transcript')
    return 'Setup estimated from the transcript. A transcript does not record the system prompt or tool definitions; capture the session to see them.'
  return 'No model call to read the setup from'
}

interface RunHeaderProps {
  letter: 'A' | 'B'
  session: SessionSummary
  now: number
  scope: string
  prompts: CompareSide['prompts']
  summary: RunSummary | null
  onScope: (value: string) => void
}

function RunHeader({ letter, session, now, scope, prompts, summary, onScope }: RunHeaderProps) {
  const options = scopeOptions(prompts)
  // Keep the chosen prompt selectable while its list is not loaded yet.
  const known = options.some((o) => o.value === scope)
  return (
    <section className={`compare-run compare-run-${letter.toLowerCase()}`} aria-label={`Run ${letter}`}>
      <div className="compare-run-title">
        <span className="tag">{letter}</span>
        <span className="compare-run-name" title={session.title ?? session.session_id}>
          {session.title ?? `Session ${shortId(session.session_id)}`}
        </span>
      </div>
      <p className="muted small">
        <span title={session.project}>{session.project_label}</span> -{' '}
        <span title={formatTimestamp(session.modified_ms) ?? undefined}>{formatRelative(session.modified_ms, now)}</span>
      </p>
      <select
        className="compare-scope"
        aria-label={`Scope of run ${letter}`}
        value={scope}
        onChange={(e) => onScope(e.target.value)}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
        {!known && <option value={scope}>Prompt</option>}
      </select>
      {summary && <p className="muted small">{sourceNote(summary.first_call_source)}</p>}
    </section>
  )
}

function CompareBody({ view }: { view: CompareResult }) {
  const sections = visibleSections(view.comparison.sections)
  const hasTokens = sections.some((s) => s.title === 'Tokens')
  const { a, b } = view.comparison
  return (
    <>
      {sections.map((section) => (
        <Fragment key={section.title}>
          <SectionTable section={section} />
          {section.title === 'Tokens' && <ContextChart a={a} b={b} />}
        </Fragment>
      ))}
      {!hasTokens && <ContextChart a={a} b={b} />}
      <section className="compare-section" aria-label="Outcome">
        <h3>Outcome</h3>
        <div className="compare-outcome">
          <FinalMessage letter="A" text={a.final_message} />
          <FinalMessage letter="B" text={b.final_message} />
        </div>
      </section>
    </>
  )
}

function SectionTable({ section }: { section: CompareSection }) {
  return (
    <section className="compare-section" aria-label={section.title}>
      <h3>{section.title}</h3>
      <div className="compare-table-wrap">
        <table className="compare-table">
          <thead>
            <tr>
              <th scope="col">
                <span className="visually-hidden">Value</span>
              </th>
              <th scope="col">A</th>
              <th scope="col">B</th>
              <th scope="col">Difference</th>
            </tr>
          </thead>
          <tbody>
            {section.rows.map((row, i) => (
              <RowView key={`${row.label}-${i}`} row={row} />
            ))}
          </tbody>
        </table>
      </div>
    </section>
  )
}

function RowView({ row }: { row: CompareRow }) {
  const tag = presenceTag(row)
  return (
    <tr>
      <th scope="row">
        {row.label}
        {tag && <span className="tag tag-small compare-presence">{tag}</span>}
      </th>
      <ValueCell value={row.a} partial={row.a_partial} higher={row.higher === 'a'} row={row} />
      <ValueCell value={row.b} partial={row.b_partial} higher={row.higher === 'b'} row={row} />
      <td className="compare-difference">{formatDifference(row)}</td>
    </tr>
  )
}

interface ValueCellProps {
  value: number | null
  partial: boolean
  higher: boolean
  row: CompareRow
}

function ValueCell({ value, partial, higher, row }: ValueCellProps) {
  return (
    <td className={higher ? 'compare-higher' : undefined} title={partial ? PARTIAL_NOTE : undefined}>
      {formatValue(value, row.unit)}
      {partial && ' *'}
    </td>
  )
}

function ContextChart({ a, b }: { a: RunSummary; b: RunSummary }) {
  const lines = chartLines(a.context_per_call, b.context_per_call, CHART_WIDTH, CHART_HEIGHT)
  if (lines.maxTokens === 0) return <p className="muted small compare-chart">No context sizes reported.</p>
  return (
    <figure className="compare-chart">
      <svg
        viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
        width="100%"
        preserveAspectRatio="none"
        role="img"
        aria-label="Context per model call of runs A and B"
      >
        <polyline className="chart-a" fill="none" points={lines.a} vectorEffect="non-scaling-stroke" />
        <polyline className="chart-b" fill="none" points={lines.b} vectorEffect="non-scaling-stroke" />
      </svg>
      <figcaption className="muted small">
        <span className="chart-legend">
          <span className="chart-key chart-key-a" aria-hidden="true" /> A
        </span>
        <span className="chart-legend">
          <span className="chart-key chart-key-b" aria-hidden="true" /> B
        </span>
        <span>
          Context per model call of the main agent (largest: {formatValue(lines.maxTokens, 'tokens')})
        </span>
      </figcaption>
    </figure>
  )
}

function FinalMessage({ letter, text }: { letter: 'A' | 'B'; text: string | null }) {
  return (
    <details className="compare-final">
      <summary>Final message of {letter}</summary>
      {text === null ? <p className="muted small">No final message.</p> : <pre className="compare-final-text">{text}</pre>}
    </details>
  )
}
