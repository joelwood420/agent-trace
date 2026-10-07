// A thin stacked bar showing what fills a model call's context.

import { barSegments, barTooltip } from '../context.ts'
import type { ContextBar as Bar } from '../types.ts'

interface Props {
  bar: Bar
  height: number
  label?: string
}

export default function ContextBar({ bar, height, label }: Props) {
  const segments = barSegments(bar)
  if (segments.length === 0) return null
  const text = barTooltip(bar)
  const description = label ? `${label}\n${text}` : text
  return (
    <div className="context-bar" role="img" aria-label={description} title={description} style={{ height }}>
      {segments.map((s) => (
        <span
          key={s.kind}
          className={`context-slice context-slice-${s.kind}`}
          style={{ width: `${s.percent}%`, minWidth: 1 }}
        />
      ))}
    </div>
  )
}
