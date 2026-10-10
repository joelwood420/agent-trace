// The session overview card listing the waste and problem flags Rust found,
// grouped by kind. Each flag jumps to the first box it is about.

import type { Flag, FlagKind, SessionFlags } from '../types.ts'
import type { Loadable } from './Sidebar.tsx'

/** Kinds in the order the card lists them, with their headings. */
const KINDS: { kind: FlagKind; heading: string }[] = [
  { kind: 'repeated_read', heading: 'Repeated reads' },
  { kind: 'failed_retry', heading: 'Failed retries' },
  { kind: 'loop', heading: 'Loops' },
  { kind: 'large_result', heading: 'Large tool results' },
  { kind: 'compaction', heading: 'Compactions' },
  { kind: 'reread_after_compaction', heading: 'Re-reads after compaction' },
]

interface Props {
  flags: Loadable<SessionFlags>
  onSelect: (traceId: string) => void
}

export default function FlagsCard({ flags, onSelect }: Props) {
  return (
    <section className="flags-card" aria-label="Flags">
      <h3>Flags</h3>
      {flags.status === 'loading' && <p className="muted small loading">Looking for problems...</p>}
      {flags.status === 'error' && (
        <div className="notice notice-error" role="alert">
          <p>Could not look for problems: {flags.message}</p>
        </div>
      )}
      {flags.status === 'ready' && flags.value.flags.length === 0 && <p className="muted small">No problems found.</p>}
      {flags.status === 'ready' &&
        KINDS.map(({ kind, heading }) => {
          const ofKind = flags.value.flags.filter((f) => f.kind === kind)
          if (ofKind.length === 0) return null
          return (
            <div key={kind} className="flags-group">
              <h4>
                {heading} <span className="muted">({ofKind.length})</span>
              </h4>
              <ul className="item-list">
                {ofKind.map((f, i) => (
                  <li key={i}>
                    <FlagButton flag={f} onSelect={onSelect} />
                  </li>
                ))}
              </ul>
            </div>
          )
        })}
    </section>
  )
}

function FlagButton({ flag, onSelect }: { flag: Flag; onSelect: (traceId: string) => void }) {
  const first = flag.trace_ids[0]
  return (
    <button
      type="button"
      className={`item item-compact flag-item flag-item-${flag.level}`}
      disabled={first === undefined}
      onClick={() => {
        if (first !== undefined) onSelect(first)
      }}
    >
      <span className="flag-item-text">{flag.text}</span>
    </button>
  )
}
