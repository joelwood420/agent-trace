// Draws one prompt's diagram with React Flow, using the positions from
// layout.ts. Boxes cannot be dragged or connected; this is a viewer.

import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  type Edge,
  type NodeTypes,
  type ReactFlowInstance,
} from '@xyflow/react'
import { useCallback, useMemo, useRef } from 'react'

import { isCaptured } from '../captureView.ts'
import { initialViewport, layoutTree, type OpenState } from '../layout.ts'
import type { DiagramNode, PromptDiagram } from '../types.ts'
import {
  MarkerNode,
  ModelCallNode,
  ParallelGroupNode,
  PromptNode,
  SubagentNode,
  SummaryNode,
  ToolCallNode,
  type BoxNode,
} from './nodes.tsx'

/** Diagrams with more boxes than this get a minimap. */
const MINIMAP_MIN_BOXES = 30

const nodeTypes: NodeTypes = {
  prompt: PromptNode,
  model_call: ModelCallNode,
  tool_call: ToolCallNode,
  parallel_group: ParallelGroupNode,
  subagent: SubagentNode,
  summary: SummaryNode,
  marker: MarkerNode,
}

interface Props {
  prompt: PromptDiagram
  openState: OpenState
  selectedId: string | null
  /** Model call trace ids with a captured API call. */
  capturedIds: ReadonlySet<string>
  onToggle: (id: string, open: boolean) => void
  onSelect: (node: DiagramNode) => void
}

export default function Diagram({ prompt, openState, selectedId, capturedIds, onToggle, onSelect }: Props) {
  const layout = useMemo(() => layoutTree(prompt.root, openState), [prompt.root, openState])
  const paneRef = useRef<HTMLDivElement>(null)

  const nodes: BoxNode[] = useMemo(
    () =>
      layout.boxes.map((box) => ({
        id: box.id,
        type: box.node.kind,
        position: { x: box.x, y: box.y },
        width: box.width,
        height: box.height,
        style: { width: box.width, height: box.height },
        // Group containers sit behind the boxes they hold.
        zIndex: box.node.kind === 'parallel_group' ? 0 : 1,
        draggable: false,
        connectable: false,
        data: {
          node: box.node,
          open: box.open,
          selected: box.id === selectedId,
          captured: isCaptured(box.node, capturedIds),
          onToggle,
          onSelect,
        },
      })),
    [layout, selectedId, capturedIds, onToggle, onSelect],
  )

  const edges: Edge[] = useMemo(
    () =>
      layout.edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        sourceHandle: e.sourceHandle,
        targetHandle: 'in',
        type: 'smoothstep',
        zIndex: 1,
        pathOptions: { borderRadius: 6 },
        focusable: false,
      })),
    [layout],
  )

  // Start fitted for small diagrams, or at the top at a readable zoom for
  // big ones. Expanding boxes later keeps the camera where it is.
  const onInit = useCallback(
    (instance: ReactFlowInstance<BoxNode, Edge>) => {
      const pane = paneRef.current?.getBoundingClientRect()
      if (!pane) return
      void instance.setViewport(initialViewport(layout.width, layout.height, pane.width, pane.height))
    },
    // Only the layout at mount time matters; the component is re-keyed per prompt.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )

  return (
    <div className="diagram" ref={paneRef}>
      <ReactFlow<BoxNode, Edge>
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onInit={onInit}
        onNodeClick={(_, node) => onSelect(node.data.node)}
        nodesDraggable={false}
        nodesConnectable={false}
        elementsSelectable={false}
        nodesFocusable={false}
        edgesFocusable={false}
        minZoom={0.1}
        maxZoom={2}
        colorMode="system"
      >
        <Background gap={20} size={1} />
        <Controls showInteractive={false} />
        {/* Only for big diagrams: on small ones it would just cover boxes. */}
        {layout.boxes.length > MINIMAP_MIN_BOXES && (
          <MiniMap
            pannable
            zoomable
            style={{ width: 150, height: 110 }}
            nodeClassName={(n) => `minimap-${n.type ?? 'box'}`}
          />
        )}
      </ReactFlow>
    </div>
  )
}
