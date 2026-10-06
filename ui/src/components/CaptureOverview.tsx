// Capture bits of the sidebar: the start command box above the session list,
// and the "API calls" overview of the open session.

import { useState } from 'react'

import { errorMessage } from '../api.ts'
import { callLine, otherCalls } from '../captureView.ts'
import { formatBytes, plural } from '../format.ts'
import type { CaptureOverview, CaptureStatus } from '../types.ts'
import CopyButton from './CopyButton.tsx'
import type { Loadable } from './Sidebar.tsx'

/** The command that starts a captured session, or why capture is off. */
function StartHint({ status }: { status: Loadable<CaptureStatus> }) {
  if (status.status === 'loading') return <p className="muted small loading">Checking the capture proxy...</p>
  if (status.status === 'error') {
    return (
      <p className="text-error small" role="alert">
        Could not read the capture status: {status.message}
      </p>
    )
  }
  const s = status.value
  return (
    <>
      {s.listening ? (
        <>
          <div className="command-row">
            <code className="command" title={s.command}>
              {s.command}
            </code>
            <CopyButton text={s.command} label="Copy" />
          </div>
          <p className="muted small">Start Claude Code with this command to capture its API calls.</p>
        </>
      ) : (
        <p className="text-error small" role="alert">
          Capture is off: {s.error ?? 'the proxy is not listening'}
        </p>
      )}
      {s.last_save_error && (
        <div className="notice notice-warning small" role="status">
          The last capture could not be saved: {s.last_save_error}
        </div>
      )}
    </>
  )
}

/** The compact "Capture" box at the top of the session list. */
export function CaptureStartBox({ status }: { status: Loadable<CaptureStatus> }) {
  return (
    <section className="capture-box" aria-label="Capture">
      <h3>Capture</h3>
      <StartHint status={status} />
    </section>
  )
}

interface OverviewProps {
  overview: Loadable<CaptureOverview>
  status: Loadable<CaptureStatus>
  selectedCaptureId: string | null
  onSelectCapture: (captureId: string) => void
  /** Deletes the session's captures; the caller reloads the overview. */
  onDelete: () => Promise<void>
  onRetry: () => void
}

/** The collapsible "API calls" section of the open session. */
export function CaptureOverviewSection(props: OverviewProps) {
  const { overview } = props
  const count = overview.status === 'ready' ? overview.value.calls.length : null
  return (
    <details className="capture-overview">
      <summary>API calls{count !== null ? ` (${count})` : ''}</summary>
      <div className="capture-overview-body">
        {overview.status === 'loading' && <p className="muted small loading">Loading captured calls...</p>}
        {overview.status === 'error' && (
          <div className="notice notice-error" role="alert">
            <p>Could not load the captured calls: {overview.message}</p>
            <button type="button" onClick={props.onRetry}>
              Try again
            </button>
          </div>
        )}
        {overview.status === 'ready' && <OverviewContents {...props} value={overview.value} />}
      </div>
    </details>
  )
}

function OverviewContents({
  value,
  status,
  selectedCaptureId,
  onSelectCapture,
  onDelete,
}: OverviewProps & { value: CaptureOverview }) {
  const others = otherCalls(value)
  const empty = value.calls.length === 0
  const hasData = !empty || value.skipped.length > 0 || value.total_bytes > 0
  return (
    <>
      {empty ? (
        <>
          <p className="small">No API calls captured for this session.</p>
          <StartHint status={status} />
        </>
      ) : (
        <p className="small">
          {plural(value.calls.length, 'captured call')} - {formatBytes(value.total_bytes)}
        </p>
      )}

      {value.skipped.length > 0 && (
        <details className="notice notice-warning">
          <summary>{plural(value.skipped.length, 'saved record')} could not be read</summary>
          <ul className="skipped-list">
            {value.skipped.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ul>
        </details>
      )}

      {value.system_versions.length > 0 && (
        <>
          <h4>System prompt versions</h4>
          <ul className="plain-list small">
            {value.system_versions.map((v) => (
              <li key={v.hash} title={`sha256 ${v.hash}`}>
                v{v.version}: {plural(v.call_count, 'call')}
                {v.system_chars !== null && `, ${v.system_chars.toLocaleString()} chars`}
              </li>
            ))}
          </ul>
        </>
      )}

      {value.tool_versions.length > 0 && (
        <>
          <h4>Tool set versions</h4>
          <ul className="plain-list small">
            {value.tool_versions.map((v) => (
              <li key={v.hash} title={v.tool_names ? v.tool_names.join(', ') : `sha256 ${v.hash}`}>
                v{v.version}: {plural(v.call_count, 'call')}
                {v.tool_names !== null && `, ${plural(v.tool_names.length, 'tool')}`}
              </li>
            ))}
          </ul>
        </>
      )}

      {others.length > 0 && (
        <>
          <h4>Other API calls ({others.length})</h4>
          <ul className="item-list">
            {others.map((c) => (
              <li key={c.capture_id}>
                <button
                  type="button"
                  className={`item item-compact${c.capture_id === selectedCaptureId ? ' item-selected' : ''}`}
                  onClick={() => onSelectCapture(c.capture_id)}
                >
                  <span className="item-title">{callLine(c)}</span>
                  <span className="item-meta">{new Date(c.started_at_ms).toLocaleTimeString()}</span>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}

      {hasData && <DeleteCaptures count={value.calls.length} onDelete={onDelete} />}
    </>
  )
}

/** Delete with an in-page confirmation (never a browser dialog). */
function DeleteCaptures({ count, onDelete }: { count: number; onDelete: () => Promise<void> }) {
  const [state, setState] = useState<
    { kind: 'idle' } | { kind: 'confirm' } | { kind: 'deleting' } | { kind: 'error'; message: string }
  >({ kind: 'idle' })

  const confirm = () => {
    setState({ kind: 'deleting' })
    onDelete()
      .then(() => setState({ kind: 'idle' }))
      .catch((err: unknown) => setState({ kind: 'error', message: errorMessage(err) }))
  }

  if (state.kind === 'confirm' || state.kind === 'deleting') {
    return (
      <div className="notice notice-warning" role="alertdialog" aria-label="Confirm delete">
        <p>Delete {plural(count, 'captured call')}? This cannot be undone.</p>
        <div className="button-row">
          <button type="button" className="danger-button" onClick={confirm} disabled={state.kind === 'deleting'}>
            {state.kind === 'deleting' ? 'Deleting...' : 'Delete'}
          </button>
          <button type="button" onClick={() => setState({ kind: 'idle' })} disabled={state.kind === 'deleting'}>
            Cancel
          </button>
        </div>
      </div>
    )
  }
  return (
    <div className="capture-delete">
      <button type="button" className="small-button" onClick={() => setState({ kind: 'confirm' })}>
        Delete captured calls
      </button>
      {state.kind === 'error' && (
        <p className="text-error small" role="alert">
          Could not delete: {state.message}
        </p>
      )}
    </div>
  )
}
