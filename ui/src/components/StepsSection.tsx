// The Steps section of the Compare view: each prompt of run A lined up with
// a prompt of run B, step by step. The backend built the lineup; this only
// lays it out and reports clicks on a step.

import { useEffect, useRef, useState } from 'react'

import { diffLabel, STEP_ROW_LIMIT, stepTags, visibleRows } from '../compare.ts'
import { formatDuration } from '../format.ts'
import type { AlignRow, AlignStep, Alignment, PromptPair, PromptSide } from '../types.ts'

/** How long the row the header link points at stays highlighted, in milliseconds. */
const FLASH_MS = 2000

const APPROXIMATE_NOTE = 'Too long to line up exactly; matched in order.'

export type Side = 'a' | 'b'

interface Props {
  alignment: Alignment
  /** A step was clicked: open it in that run's diagram. */
  onJump: (side: Side, traceId: string) => void
}

/** The element id of a row, or of a pair's heading when `row` is null. */
function targetId(pair: number, row: number | null): string {
  return row === null ? `steps-pair-${pair}` : `steps-row-${pair}-${row}`
}

function pairKey(pair: PromptPair | undefined): string {
  return `${pair?.a?.turn_id ?? ''}|${pair?.b?.turn_id ?? ''}`
}

function reducedMotion(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches
}

export default function StepsSection({ alignment, onJump }: Props) {
  // Pairs whose rows all show, by the turn ids of the pair.
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  // The element the header link highlighted, by id.
  const [flash, setFlash] = useState<string | null>(null)
  // An element to scroll to once it is rendered (after "Show all").
  const pendingScroll = useRef<string | null>(null)
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (flashTimer.current !== null) clearTimeout(flashTimer.current)
    },
    [],
  )

  const scrollTo = (id: string) => {
    const el = document.getElementById(id)
    if (!el) return false
    el.scrollIntoView({ block: 'center', behavior: reducedMotion() ? 'auto' : 'smooth' })
    return true
  }

  // A new lineup drops a scroll asked for on the old one.
  useEffect(() => {
    pendingScroll.current = null
  }, [alignment])

  // Runs after every render; tries the pending scroll once, found or not.
  useEffect(() => {
    const id = pendingScroll.current
    if (id === null) return
    pendingScroll.current = null
    scrollTo(id)
  })

  const showAll = (pair: number) => setExpanded((prev) => new Set(prev).add(pairKey(alignment.pairs[pair])))

  const goToDifference = () => {
    const point = alignment.first_difference
    if (point === null) return
    const pair = alignment.pairs[point.pair]
    if (!pair) return
    const hasRow = point.row < pair.rows.length
    const id = targetId(point.pair, hasRow ? point.row : null)
    if (hasRow && point.row >= STEP_ROW_LIMIT && !expanded.has(pairKey(pair))) {
      pendingScroll.current = id
      showAll(point.pair)
    } else {
      scrollTo(id)
    }
    setFlash(id)
    if (flashTimer.current !== null) clearTimeout(flashTimer.current)
    flashTimer.current = setTimeout(() => {
      flashTimer.current = null
      setFlash(null)
    }, FLASH_MS)
  }

  const label = diffLabel(alignment)

  return (
    <section className="compare-section steps" aria-label="Steps">
      <h3>Steps</h3>
      <p className="steps-header">
        {alignment.first_difference !== null && alignment.pairs.length > 0 ? (
          <button type="button" className="link-button steps-diff-link" onClick={goToDifference}>
            {label}
          </button>
        ) : (
          <span className="muted">{label}</span>
        )}
      </p>
      {alignment.pairs.map((pair, i) => (
        <PairView
          key={i}
          index={i}
          pair={pair}
          showAll={expanded.has(pairKey(pair))}
          flash={flash}
          onShowAll={() => showAll(i)}
          onJump={onJump}
        />
      ))}
    </section>
  )
}

const NO_PROMPT = 'No matching prompt'

/** One side's half of a pair heading. */
function sideTitle(letter: 'A' | 'B', side: PromptSide | null): string {
  if (side === null) return `${letter}: ${NO_PROMPT}`
  return `${letter}: Prompt ${side.index + 1}: ${side.preview}`
}

interface PairProps {
  index: number
  pair: PromptPair
  showAll: boolean
  flash: string | null
  onShowAll: () => void
  onJump: (side: Side, traceId: string) => void
}

function PairView({ index, pair, showAll, flash, onShowAll, onJump }: PairProps) {
  const rows = visibleRows(pair.rows, showAll)
  const headingId = targetId(index, null)
  const halves = [
    { letter: 'A', side: pair.a },
    { letter: 'B', side: pair.b },
  ] as const
  return (
    <div className="steps-pair">
      <h4 id={headingId} className={`steps-pair-title${flash === headingId ? ' steps-flash' : ''}`}>
        {halves.map(({ letter, side }) => {
          const title = sideTitle(letter, side)
          return (
            <span key={letter} className={`steps-pair-side${side === null ? ' muted' : ''}`} title={title}>
              {title}
            </span>
          )
        })}
      </h4>
      {pair.approximate && <p className="muted small steps-note">{APPROXIMATE_NOTE}</p>}
      {pair.rows.length > 0 && (
        <div className="compare-table-wrap">
          <table className="compare-table steps-table">
            <thead>
              <tr>
                <th scope="col">A</th>
                <th scope="col">B</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, r) => {
                const id = targetId(index, r)
                const classes = [
                  pair.first_difference === r ? 'steps-first-difference' : '',
                  flash === id ? 'steps-flash' : '',
                ]
                  .filter(Boolean)
                  .join(' ')
                return <RowView key={r} id={id} className={classes || undefined} row={row} onJump={onJump} />
              })}
            </tbody>
          </table>
        </div>
      )}
      {rows.length < pair.rows.length && (
        <button type="button" className="link-button steps-show-all" onClick={onShowAll}>
          Show all {pair.rows.length} rows
        </button>
      )}
    </div>
  )
}

interface RowProps {
  id: string
  className: string | undefined
  row: AlignRow
  onJump: (side: Side, traceId: string) => void
}

function RowView({ id, className, row, onJump }: RowProps) {
  const marker = className?.includes('steps-first-difference') === true
  const tags = stepTags(row)
  // The tags go in the empty slot of a one-sided row, else in B's cell.
  const tagsIn: Side = row.a === null ? 'a' : 'b'
  return (
    <tr id={id} className={className}>
      <StepCell side="a" step={row.a} tags={tagsIn === 'a' ? tags : []} marker={marker} onJump={onJump} />
      <StepCell side="b" step={row.b} tags={tagsIn === 'b' ? tags : []} onJump={onJump} />
    </tr>
  )
}

interface CellProps {
  side: Side
  step: AlignStep | null
  tags: string[]
  /** Adds hidden text naming the first-difference row. */
  marker?: boolean
  onJump: (side: Side, traceId: string) => void
}

function StepCell({ side, step, tags, marker, onJump }: CellProps) {
  const letter = side === 'a' ? 'A' : 'B'
  const duration = step ? formatDuration(step.duration_ms) : null
  return (
    <td className={step ? 'steps-cell' : 'steps-cell steps-empty'} data-side={letter}>
      {marker === true && <span className="visually-hidden">First difference</span>}
      {step && (
        <>
          <button
            type="button"
            className="link-button steps-step"
            title={`Open this step in run ${letter}'s diagram`}
            onClick={() => onJump(side, step.trace_id)}
          >
            {step.label}
          </button>
          {step.failed && (
            <span className="steps-error" title="Failed">
              <span aria-hidden="true">{'✕'}</span>
              <span className="visually-hidden">failed</span>
            </span>
          )}
          {step.running && <span className="status status-running">running</span>}
          {duration && <span className="muted steps-duration">{duration}</span>}
        </>
      )}
      {tags.map((t) => (
        <span key={t} className="tag tag-small steps-tag">
          {t}
        </span>
      ))}
    </td>
  )
}
