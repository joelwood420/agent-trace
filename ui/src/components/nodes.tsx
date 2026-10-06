// One React Flow node component per diagram box kind. They only draw what
// the Rust view model says; sizes come from layout.ts so the layout is exact.

import { Handle, Position, type Node, type NodeProps } from '@xyflow/react'
import type { KeyboardEvent, ReactNode } from 'react'

import { formatDuration, formatTokens } from '../format.ts'
import type { DiagramNode, Status } from '../types.ts'

export interface BoxData extends Record<string, unknown> {
  node: DiagramNode
  /** `null` if the box cannot be expanded or collapsed, else whether it is open. */
  open: boolean | null
  selected: boolean
  /** True for a model call box with a captured raw API request. */
  captured: boolean
  onToggle: (id: string, open: boolean) => void
  onSelect: (node: DiagramNode) => void
}

export type BoxNode = Node<BoxData>

/**
 * Makes a box reachable with Tab and selectable with Enter or Space. Mouse
 * clicks are handled by React Flow's `onNodeClick`.
 */
function keyboardProps(data: BoxData) {
  return {
    role: 'button',
    tabIndex: 0,
    'aria-pressed': data.selected,
    'aria-label': [data.node.label, data.node.detail_label, data.node.status === 'none' ? null : `status ${data.node.status}`]
      .filter(Boolean)
      .join(', '),
    onKeyDown: (e: KeyboardEvent) => {
      if (e.target !== e.currentTarget) return
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        data.onSelect(data.node)
      }
    },
  }
}

const STATUS_TEXT: Record<Status, { icon: string; text: string } | null> = {
  ok: { icon: '✓', text: 'ok' },
  error: { icon: '✕', text: 'error' },
  running: { icon: '◌', text: 'running' },
  none: null,
}

export function StatusChip({ status }: { status: Status }) {
  const info = STATUS_TEXT[status]
  if (!info) return null
  return (
    <span className={`status status-${status}`} title={`Status: ${info.text}`}>
      <span aria-hidden="true">{info.icon}</span> {info.text}
    </span>
  )
}

/** Duration and token numbers, each only when known. */
function Metrics({ node }: { node: DiagramNode }) {
  const parts: ReactNode[] = []
  const duration = formatDuration(node.duration_ms)
  if (duration) parts.push(<span key="d" title="Duration">{duration}</span>)
  const ctx = formatTokens(node.context_tokens)
  if (ctx) {
    parts.push(
      <span key="c" title={`Context: ${node.context_tokens} tokens`}>
        {ctx} ctx
      </span>,
    )
  }
  const out = formatTokens(node.output_tokens)
  if (out) {
    parts.push(
      <span key="o" title={`Output: ${node.output_tokens} tokens`}>
        {out} out
      </span>,
    )
  }
  return <span className="metrics">{parts}</span>
}

function Toggle({ data }: { data: BoxData }) {
  if (data.open === null) return null
  const count = data.node.children.length
  return (
    <button
      type="button"
      className="toggle nodrag nopan"
      aria-expanded={data.open}
      title={data.open ? 'Hide what is inside' : 'Show what is inside'}
      onClick={(e) => {
        e.stopPropagation()
        data.onToggle(data.node.id, !data.open)
      }}
    >
      {data.open ? `▾ Hide ${count}` : `▸ Show ${count}`}
    </button>
  )
}

/** Connection points used by layout.ts edges. Invisible, not connectable. */
function Handles() {
  return (
    <>
      <Handle type="target" position={Position.Left} id="in" className="handle handle-in" isConnectable={false} />
      <Handle type="source" position={Position.Bottom} id="down" className="handle handle-down" isConnectable={false} />
      <Handle type="source" position={Position.Right} id="right" className="handle handle-right" isConnectable={false} />
    </>
  )
}

interface ShellProps {
  data: BoxData
  tag: string | null
  /** Put the label on its own line under the tag (prompt and subagent). */
  labelLine?: boolean
  showMetrics?: boolean
}

function Shell({ data, tag, labelLine = false, showMetrics = true }: ShellProps) {
  const { node } = data
  const classes = ['box', `box-${node.kind}`, `box-status-${node.status}`]
  if (data.selected) classes.push('box-selected')
  const label = (
    <span className="box-label" title={node.label}>
      {node.label}
    </span>
  )
  return (
    <div className={classes.join(' ')} {...keyboardProps(data)}>
      <Handles />
      <div className="box-row">
        {tag && <span className="tag">{tag}</span>}
        {!labelLine && label}
        <span className="spacer" />
        {data.captured && (
          <span className="tag tag-api" title="Raw API request captured">
            API
          </span>
        )}
        <StatusChip status={node.status} />
      </div>
      {labelLine && <div className="box-row">{label}</div>}
      {node.detail_label && (
        <div className="box-row box-detail" title={node.detail_label}>
          {node.detail_label}
        </div>
      )}
      {(showMetrics || data.open !== null) && (
        <div className="box-row box-footer">
          {showMetrics && <Metrics node={node} />}
          <span className="spacer" />
          <Toggle data={data} />
        </div>
      )}
    </div>
  )
}

export function PromptNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag="Prompt" labelLine />
}

export function ModelCallNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag={null} />
}

export function ToolCallNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag="Tool" />
}

export function SubagentNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag="Subagent" labelLine />
}

export function SummaryNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag="Repeated" />
}

export function MarkerNode({ data }: NodeProps<BoxNode>) {
  return <Shell data={data} tag="Marker" showMetrics={false} />
}

/** The container drawn around tool calls requested together. */
export function ParallelGroupNode({ data }: NodeProps<BoxNode>) {
  const { node } = data
  const classes = ['group', `box-status-${node.status}`]
  if (data.selected) classes.push('box-selected')
  const duration = formatDuration(node.duration_ms)
  return (
    <div className={classes.join(' ')} {...keyboardProps(data)}>
      <Handles />
      <div className="group-header">
        <span className="tag">Parallel</span>
        <span className="box-label" title={node.label}>
          {node.label}
        </span>
        {node.detail_label && (
          <span className="group-detail" title={node.detail_label}>
            {node.detail_label}
          </span>
        )}
        <span className="spacer" />
        {duration && <span className="metrics">{duration}</span>}
        <StatusChip status={node.status} />
      </div>
    </div>
  )
}
