// The captured API call of a model call box (or of an "other" call picked in
// the session overview): what changed since the previous call, and the full
// request and response. Fetched with `capture_detail`.

import { useEffect, useMemo, useState, type ReactNode } from 'react'

import { errorMessage, type Api } from '../api.ts'
import { diffLines, messageLine, requestJsonText, settingRows, systemHeading } from '../captureView.ts'
import { messageList, requestBody, systemBlocks, toolList } from '../captures.ts'
import { formatDuration, formatTimestamp, plural, prettyJson } from '../format.ts'
import type { CaptureDetail, CaptureRecord, MessageSummary, RequestDiff, RequestSummary } from '../types.ts'
import CopyButton from './CopyButton.tsx'

interface Props {
  api: Api
  project: string
  sessionId: string
  captureId: string
  /** Open the full request straight away (for a call picked in the overview). */
  rawOpen?: boolean
}

type Fetched =
  | { key: string; status: 'ready'; detail: CaptureDetail | null }
  | { key: string; status: 'error'; message: string }

/** The capture sections, for the details panel. */
export function CaptureSections({ api, project, sessionId, captureId, rawOpen = false }: Props) {
  const key = `${project}/${sessionId}/${captureId}`
  const [fetched, setFetched] = useState<Fetched | null>(null)

  // The detail holds the diff, which is shown open, so it is fetched as soon
  // as the box is selected. The full request is only drawn when opened.
  useEffect(() => {
    let cancelled = false
    api
      .captureDetail(project, sessionId, captureId)
      .then((detail) => {
        if (!cancelled) setFetched({ key, status: 'ready', detail })
      })
      .catch((err: unknown) => {
        if (!cancelled) setFetched({ key, status: 'error', message: errorMessage(err) })
      })
    return () => {
      cancelled = true
    }
  }, [api, project, sessionId, captureId, key])

  const current = fetched?.key === key ? fetched : null
  return (
    <section className="detail-section capture-sections">
      <h3>Captured API call</h3>
      {current === null && <p className="muted loading">Loading the captured call...</p>}
      {current?.status === 'error' && (
        <div className="notice notice-error" role="alert">
          Could not load the captured call: {current.message}
        </div>
      )}
      {current?.status === 'ready' && current.detail === null && (
        <p className="muted">This captured call was not found. It may have been deleted.</p>
      )}
      {current?.status === 'ready' && current.detail !== null && (
        <>
          <Changes key={`changes-${key}`} detail={current.detail} />
          <Fold key={`raw-${key}`} title="Raw request" defaultOpen={rawOpen} className="fold-main">
            <RequestView record={current.detail.record} summary={current.detail.summary} />
          </Fold>
        </>
      )}
    </section>
  )
}

/** A `<details>` that only draws its contents once opened. */
function Fold({
  title,
  defaultOpen = false,
  className,
  children,
}: {
  title: ReactNode
  defaultOpen?: boolean
  className?: string
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <details
      className={`fold${className ? ` ${className}` : ''}`}
      open={open}
      onToggle={(e) => setOpen(e.currentTarget.open)}
    >
      <summary>{title}</summary>
      {open && <div className="fold-body">{children}</div>}
    </details>
  )
}

function CachedChip() {
  return (
    <span className="chip-cached" title="Has a cache_control marker">
      cached
    </span>
  )
}

function Changes({ detail }: { detail: CaptureDetail }) {
  const { diff, previous_capture_id } = detail
  return (
    <Fold title="Changes since the previous call" defaultOpen={diff !== null} className="fold-main">
      {previous_capture_id === null && (
        <p className="muted">No earlier captured call of this agent to compare with.</p>
      )}
      {previous_capture_id !== null && diff === null && (
        <p className="muted">The previous call could not be compared with this one.</p>
      )}
      {diff !== null && <DiffView diff={diff} />}
    </Fold>
  )
}

function DiffView({ diff }: { diff: RequestDiff }) {
  const lines = diffLines(diff)
  return (
    <div className="diff">
      {lines.historyRemoved && (
        <div className="notice notice-warning" role="status">
          History was removed or replaced (for example by compaction).
        </div>
      )}
      <p className="small">Kept {plural(lines.kept, 'message')}</p>
      {diff.removed.length > 0 && <MessageSummaries title={`Removed (${diff.removed.length})`} messages={diff.removed} kind="removed" />}
      {diff.added.length > 0 && <MessageSummaries title={`Added (${diff.added.length})`} messages={diff.added} kind="added" />}
      {lines.changes.length > 0 ? (
        <ul className="diff-changes">
          {lines.changes.map((c, i) => (
            <li key={i}>{c}</li>
          ))}
        </ul>
      ) : (
        <p className="muted small">System prompt, tools and settings unchanged.</p>
      )}
    </div>
  )
}

function MessageSummaries({
  title,
  messages,
  kind,
}: {
  title: string
  messages: MessageSummary[]
  kind: 'added' | 'removed'
}) {
  return (
    <div className={`diff-messages diff-${kind}`}>
      <h4>{title}</h4>
      <ul>
        {messages.map((m) => (
          <li key={m.index}>
            <span className="mono">{messageLine(m)}</span>
            {m.preview && <span className="diff-preview">{m.preview}</span>}
          </li>
        ))}
      </ul>
    </div>
  )
}

function RequestView({ record, summary }: { record: CaptureRecord; summary: RequestSummary | null }) {
  // Reshaped once per record, so re-renders do not walk a large body again.
  const { body, system, tools, messages } = useMemo(() => {
    const b = requestBody(record)
    return { body: b, system: systemBlocks(b), tools: toolList(b), messages: messageList(b) }
  }, [record])
  const ended = record.ended_at_ms
  return (
    <div className="request">
      <dl className="facts">
        <dt>Request</dt>
        <dd className="mono">
          {record.request.method} {record.request.path}
        </dd>
        <dt>Started</dt>
        <dd>{formatTimestamp(record.started_at_ms)}</dd>
        {ended !== null && (
          <>
            <dt>Duration</dt>
            <dd>{formatDuration(ended - record.started_at_ms)}</dd>
          </>
        )}
        {record.error && (
          <>
            <dt>Error</dt>
            <dd className="text-error">{record.error}</dd>
          </>
        )}
      </dl>
      <div className="request-actions">
        <CopyButton text={() => requestJsonText(record)} label="Copy request JSON" />
      </div>

      {summary && (
        <>
          <h4>Settings</h4>
          <dl className="facts">
            {settingRows(summary.settings).map((r) => (
              <SettingRow key={r.key} name={r.key} value={r.value} />
            ))}
          </dl>
          {summary.betas.length > 0 && (
            <>
              <h4>Betas</h4>
              <ul className="plain-list mono small">
                {summary.betas.map((b) => (
                  <li key={b}>{b}</li>
                ))}
              </ul>
            </>
          )}
        </>
      )}

      {body === null && (
        <>
          <h4>Body</h4>
          <BodyText record={record} />
        </>
      )}

      {body !== null && (
        <>
          <Fold title={systemHeading(system)}>
            {system.length === 0 && <p className="muted small">No system prompt.</p>}
            {system.map((b, i) => (
              <Fold
                key={i}
                title={
                  <>
                    Block {i + 1} ({b.text.length} chars) {b.cached && <CachedChip />}
                  </>
                }
              >
                <pre className="code">{b.text}</pre>
              </Fold>
            ))}
          </Fold>
          <Fold title={`Tools (${tools.length})`}>
            {tools.length === 0 && <p className="muted small">No tools offered.</p>}
            {tools.map((t, i) => (
              <Fold key={i} title={<span className="mono">{t.name || '(no name)'}</span>}>
                {t.description && <pre className="text-block">{t.description}</pre>}
                <pre className="code">{prettyJson(t.schema)}</pre>
              </Fold>
            ))}
          </Fold>
          <Fold title={`Messages (${messages.length})`}>
            {messages.map((m, i) => (
              <Fold
                key={i}
                title={
                  <>
                    {i + 1}. {m.role || '(no role)'} ({plural(m.blocks.length, 'block')}){' '}
                    {m.blocks.some((b) => b.cached) && <CachedChip />}
                  </>
                }
              >
                {m.blocks.map((b, j) => (
                  <div key={j} className="message-block">
                    <div className="message-block-head">
                      <span className="tag">{b.type}</span>
                      {b.cached && <CachedChip />}
                    </div>
                    <pre className="code">{b.text}</pre>
                  </div>
                ))}
              </Fold>
            ))}
          </Fold>
        </>
      )}

      <ResponseView record={record} />
    </div>
  )
}

function SettingRow({ name, value }: { name: string; value: string }) {
  return (
    <>
      <dt className="mono">{name}</dt>
      <dd className="mono">{value}</dd>
    </>
  )
}

function BodyText({ record }: { record: CaptureRecord }) {
  const body = record.request.body
  if (body.kind === 'empty') return <p className="muted small">Empty body.</p>
  return <pre className="code">{body.kind === 'text' ? body.value : prettyJson(body.value)}</pre>
}

function ResponseView({ record }: { record: CaptureRecord }) {
  const response = record.response
  return (
    <Fold title={response ? `Response (status ${response.status})` : 'Response'}>
      {response === null && <p className="muted small">No response was recorded.</p>}
      {response !== null && (
        <>
          <h4>Message</h4>
          {response.message === null || response.message === undefined ? (
            <p className="muted small">No message could be rebuilt from the response.</p>
          ) : (
            <pre className="code">{prettyJson(response.message)}</pre>
          )}
          {response.stream !== null && (
            <Fold title={`Raw stream (${response.stream.length} chars)`}>
              <pre className="code">{response.stream}</pre>
            </Fold>
          )}
        </>
      )}
    </Fold>
  )
}

/** The details panel for a capture picked in the session overview. */
export function CaptureDetailsPanel({
  api,
  project,
  sessionId,
  captureId,
  onClose,
}: Omit<Props, 'rawOpen'> & { onClose: () => void }) {
  return (
    <aside className="details" aria-label="Details of the selected API call">
      <div className="details-header">
        <div className="details-title">
          <span className="tag">API call</span>
          <span className="spacer" />
          <button type="button" className="icon-button" onClick={onClose} title="Close details">
            {'✕'}
            <span className="visually-hidden">Close details</span>
          </button>
        </div>
        <h2 className="details-label mono">{captureId}</h2>
        <p className="muted">An API call that belongs to no model call box.</p>
      </div>
      <div className="details-body">
        <CaptureSections api={api} project={project} sessionId={sessionId} captureId={captureId} rawOpen />
      </div>
    </aside>
  )
}
