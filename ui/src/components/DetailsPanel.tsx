// The right-hand panel: full content of the selected box, fetched with
// `node_detail` for each of the box's trace ids.

import { useEffect, useState, type ReactNode } from 'react'

import { errorMessage, type Api } from '../api.ts'
import { formatDuration, formatTimestamp, formatTokens } from '../format.ts'
import type { ContentBlock, DiagramNode, NodeDetail, RawSource, StopReason, Usage } from '../types.ts'
import { StatusChip } from './nodes.tsx'

interface Props {
  api: Api
  project: string
  sessionId: string
  node: DiagramNode
  /** Goes up when the box's trace nodes changed in a live update, to refetch. */
  refreshKey: number
  onClose: () => void
}

type Fetched =
  | { key: string; status: 'ready'; details: (NodeDetail | null)[] }
  | { key: string; status: 'error'; message: string }

const KIND_NAMES: Record<DiagramNode['kind'], string> = {
  prompt: 'Prompt',
  model_call: 'Model call',
  tool_call: 'Tool call',
  parallel_group: 'Parallel tool calls',
  subagent: 'Subagent',
  summary: 'Repeated calls',
  marker: 'Marker',
}

export default function DetailsPanel({ api, project, sessionId, node, refreshKey, onClose }: Props) {
  const key = `${project}/${sessionId}/${node.id}`
  const [fetched, setFetched] = useState<Fetched | null>(null)
  // A live update hands over a new node object each time; keying the fetch on
  // its content means only a real change (or `refreshKey`) fetches again.
  const traceIdsKey = node.trace_ids.join('|')

  useEffect(() => {
    let cancelled = false
    const traceIds = traceIdsKey === '' ? [] : traceIdsKey.split('|')
    Promise.all(traceIds.map((id) => api.nodeDetail(project, sessionId, id)))
      .then((details) => {
        if (!cancelled) setFetched({ key, status: 'ready', details })
      })
      .catch((err: unknown) => {
        if (!cancelled) setFetched({ key, status: 'error', message: errorMessage(err) })
      })
    return () => {
      cancelled = true
    }
  }, [api, project, sessionId, node.id, traceIdsKey, refreshKey, key])

  const current = fetched?.key === key ? fetched : null

  return (
    <aside className="details" aria-label="Details of the selected box">
      <div className="details-header">
        <div className="details-title">
          <span className="tag">{KIND_NAMES[node.kind]}</span>
          <StatusChip status={node.status} />
          <span className="spacer" />
          <button type="button" className="icon-button" onClick={onClose} title="Close details">
            {'✕'}
            <span className="visually-hidden">Close details</span>
          </button>
        </div>
        <h2 className="details-label">{node.label}</h2>
        {node.detail_label && <p className="muted">{node.detail_label}</p>}
      </div>
      <div className="details-body">
        {current === null && <p className="muted loading">Loading details...</p>}
        {current?.status === 'error' && (
          <div className="notice notice-error" role="alert">
            Could not load the details: {current.message}
          </div>
        )}
        {current?.status === 'ready' &&
          current.details.map((detail, i) => {
            const id = node.trace_ids[i] ?? String(i)
            if (!detail) {
              return (
                <p key={id} className="muted">
                  No details found for {id}. The session may have changed since it was loaded.
                </p>
              )
            }
            if (current.details.length === 1) return <Detail key={id} detail={detail} />
            return (
              // Two items (a subagent's run and task) start open; longer
              // lists (groups, summaries) start closed so they stay scannable.
              <details key={id} className="detail-item" open={current.details.length <= 2}>
                <summary>{summaryLine(detail)}</summary>
                <Detail detail={detail} />
              </details>
            )
          })}
      </div>
    </aside>
  )
}

/** One line describing a trace node, for the list of items inside a group. */
function summaryLine(detail: NodeDetail): string {
  const n = detail.node
  const duration = formatDuration(detail.duration_ms)
  const suffix = duration ? ` (${duration})` : ''
  switch (n.type) {
    case 'tool_call':
      return `${n.name}${n.result?.is_error ? ' - error' : ''}${suffix}`
    case 'model_call':
      return `Model call${n.model ? `: ${n.model}` : ''}${suffix}`
    case 'marker':
      return `Marker: ${n.summary}`
    case 'turn':
      return 'Task given to the subagent'
    case 'run':
      return `Run${n.title ? `: ${n.title}` : ''}`
  }
}

function Detail({ detail }: { detail: NodeDetail }) {
  const n = detail.node
  return (
    <div className="detail">
      <Section title="Overview">
        <dl className="facts">
          <Fact name="Trace kind" value={detail.kind} />
          <Fact name="Trace id" value={detail.trace_id} mono />
          <Fact name="Started" value={formatTimestamp(detail.started_at_ms)} />
          <Fact name="Ended" value={formatTimestamp(detail.ended_at_ms)} />
          <Fact name="Duration" value={formatDuration(detail.duration_ms)} />
          {n.type === 'run' && <Fact name="Harness" value={n.harness} />}
          {n.type === 'run' && <Fact name="Title" value={n.title ?? null} />}
          {n.type === 'model_call' && <Fact name="Model" value={n.model ?? null} />}
          {n.type === 'model_call' && <Fact name="Stop reason" value={stopReasonText(n.stop_reason)} />}
          {n.type === 'tool_call' && <Fact name="Tool" value={n.name} />}
          {n.type === 'tool_call' && (
            <Fact
              name="Result"
              value={n.result ? (n.result.is_error ? 'error' : 'ok') : 'no result yet'}
              className={n.result?.is_error ? 'text-error' : undefined}
            />
          )}
          {n.type === 'marker' && <Fact name="Marker kind" value={n.kind} />}
        </dl>
      </Section>

      {n.type === 'model_call' && (n.usage || detail.context_tokens !== null) && (
        <Section title="Token usage">
          <UsageTable usage={n.usage} context={detail.context_tokens} />
        </Section>
      )}

      {n.type === 'turn' && (
        <Section title="Prompt">
          <Blocks blocks={n.prompt} empty="(empty prompt)" />
        </Section>
      )}
      {n.type === 'model_call' && (
        <Section title="Model output">
          <Blocks blocks={n.output} empty="(no text output; see the tool calls)" />
        </Section>
      )}
      {n.type === 'tool_call' && (
        <>
          <Section title="Input">
            <pre className="code">{prettyJson(n.input)}</pre>
          </Section>
          <Section title={n.result?.is_error ? 'Result (error)' : 'Result'}>
            {n.result ? (
              <div className={n.result.is_error ? 'result-error' : undefined}>
                <Blocks blocks={n.result.content} empty="(empty result)" />
              </div>
            ) : (
              <p className="muted">No result yet. The tool may still be running, or the session was interrupted.</p>
            )}
          </Section>
        </>
      )}
      {n.type === 'marker' && (
        <Section title="Summary">
          <p>{n.summary}</p>
        </Section>
      )}

      {Object.keys(detail.metadata).length > 0 && (
        <Section title="Metadata">
          <pre className="code">{prettyJson(detail.metadata)}</pre>
        </Section>
      )}

      <Section title={`Raw source (${detail.raw.length})`}>
        {detail.raw.length === 0 ? (
          <p className="muted">No raw source records.</p>
        ) : (
          <RawList raw={detail.raw} />
        )}
      </Section>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="detail-section">
      <h3>{title}</h3>
      {children}
    </section>
  )
}

function Fact({
  name,
  value,
  mono = false,
  className,
}: {
  name: string
  value: string | null
  mono?: boolean
  className?: string
}) {
  if (value === null) return null
  const classes = [mono ? 'mono' : '', className ?? ''].join(' ').trim()
  return (
    <>
      <dt>{name}</dt>
      <dd className={classes || undefined}>{value}</dd>
    </>
  )
}

function UsageTable({ usage, context }: { usage: Usage | undefined; context: number | null }) {
  const rows: [string, number | null | undefined][] = [
    ['Context (input + cache)', context],
    ['Input, uncached', usage?.input_tokens],
    ['Cache read', usage?.cache_read_tokens],
    ['Cache write', usage?.cache_write_tokens],
    ['Output', usage?.output_tokens],
  ]
  return (
    <dl className="facts">
      {rows.map(([name, value]) => (
        <Fact
          key={name}
          name={name}
          value={value === undefined || value === null ? 'unknown' : `${value.toLocaleString()} (${formatTokens(value)})`}
        />
      ))}
    </dl>
  )
}

function Blocks({ blocks, empty }: { blocks: ContentBlock[]; empty: string }) {
  if (blocks.length === 0) return <p className="muted">{empty}</p>
  return (
    <div className="blocks">
      {blocks.map((block, i) => {
        switch (block.type) {
          case 'text':
            return (
              <pre key={i} className="text-block">
                {block.text}
              </pre>
            )
          case 'thinking':
            return (
              <details key={i} className="thinking">
                <summary>Thinking{block.text ? '' : ' (hidden by the provider)'}</summary>
                {block.text && <pre className="text-block">{block.text}</pre>}
              </details>
            )
          case 'image':
            return (
              <p key={i} className="muted">
                [image{block.media_type ? `: ${block.media_type}` : ''}; data not kept]
              </p>
            )
          case 'other':
            return (
              <p key={i} className="muted">
                [{block.kind} content; see the raw source]
              </p>
            )
        }
        return null
      })}
    </div>
  )
}

function RawList({ raw }: { raw: RawSource[] }) {
  const [pretty, setPretty] = useState(false)
  return (
    <div className="raw">
      <label className="raw-toggle">
        <input type="checkbox" name="format-json" checked={pretty} onChange={(e) => setPretty(e.target.checked)} /> Format JSON
      </label>
      {raw.map((r, i) => (
        <details key={i} className="raw-item">
          <summary className="mono">
            {r.source}:{r.line}
          </summary>
          <pre className="code">{pretty ? prettyRaw(r.text) : r.text}</pre>
        </details>
      ))}
    </div>
  )
}

function stopReasonText(reason: StopReason | undefined): string | null {
  if (reason === undefined) return null
  if (typeof reason === 'string') return reason
  return reason.other
}

function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return String(value)
  }
}

/** The raw line pretty-printed if it is JSON, else unchanged. */
function prettyRaw(text: string): string {
  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return text
  }
}
