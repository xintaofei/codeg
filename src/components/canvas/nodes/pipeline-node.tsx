"use client"

import { memo, useEffect, useState } from "react"
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  Controls,
  type Node,
  type NodeProps,
  type Edge,
} from "@xyflow/react"
import type { NodeTypes } from "@xyflow/react"
import { useTranslations } from "next-intl"
import { AlertCircle, Plus, Trash2 } from "lucide-react"
import { pipelineGet, pipelineSave } from "@/lib/api"
import {
  addStep,
  moveStep,
  createDefaultStep,
  removeStep,
  removeLoop,
  validateGraph,
} from "@/lib/pipeline-graph-edit"
import type {
  LoopBack,
  Pipeline,
  PipelineGraph,
  PipelineStep,
} from "@/lib/types"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import type { PipelineNodeData } from "../canvas-model"
import { PipelineStepInspector } from "../pipeline-step-inspector"

type PipelineFlowNode = Node<PipelineNodeData, "pipeline">

const STEP_NODE_HEIGHT = 60
const STEP_GAP = 20

/** Inner node representing a pipeline step. */
function StepNodeComponent({
  data,
  selected,
}: {
  data: {
    step: PipelineStep
    isLoopSource: boolean
    onSelect: () => void
  }
  selected: boolean
}) {
  const { step, isLoopSource, onSelect } = data
  return (
    <div
      onClick={onSelect}
      className={cn(
        "nodrag nowheel relative rounded-lg border px-3 py-2 text-sm font-medium cursor-pointer transition-all",
        "bg-card border-foreground/15 hover:border-foreground/30",
        selected && "border-primary ring-2 ring-primary/25",
        isLoopSource && "ring-2 ring-orange-500/30"
      )}
    >
      {step.label}
    </div>
  )
}

const InnerNodeTypes = {
  step: StepNodeComponent,
} as unknown as NodeTypes

/** Core pipeline rendering: graph, steps in a line, loop edges. */
function PipelineNodeContent({
  pipelineId,
  initialGraph,
  onGraphChange,
}: {
  pipelineId: number | null
  initialGraph?: PipelineGraph
  onGraphChange?: (graph: PipelineGraph) => void
}) {
  const t = useTranslations("Pipeline")
  const [graph, setGraph] = useState<PipelineGraph | null>(initialGraph ?? null)
  // Name, folder and isolation of the saved row, so an edit can be written
  // back without inventing them. A card that never loaded a row edits nothing
  // persistent, which is why every write is guarded on this.
  const [saved, setSaved] = useState<Pipeline | null>(null)
  const [loading, setLoading] = useState(!initialGraph && pipelineId != null)
  const [error, setError] = useState<string | null>(null)
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null)
  const [inspectorOpen, setInspectorOpen] = useState(false)
  const [validationError, setValidationError] = useState<string | null>(null)

  useEffect(() => {
    if (!pipelineId || initialGraph) return
    // `loading` starts true for this case, so the effect only has to clear it
    // when the fetch settles: setting it here would re-render mid-effect.
    pipelineGet(pipelineId)
      .then((pipeline) => {
        setSaved(pipeline)
        setGraph(pipeline.graph)
        onGraphChange?.(pipeline.graph)
      })
      .catch((err) => {
        setError(String(err))
      })
      .finally(() => setLoading(false))
  }, [pipelineId, initialGraph, onGraphChange])

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-sm text-muted-foreground">
        Loading...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex items-center justify-center h-full text-sm text-destructive">
        {error}
      </div>
    )
  }

  if (!graph) {
    return (
      <div className="flex items-center justify-center h-full text-sm text-muted-foreground">
        No pipeline
      </div>
    )
  }

  const selectedStep =
    selectedStepId != null
      ? graph.steps.find((s) => s.id === selectedStepId)
      : null
  const selectedLoop =
    selectedStepId != null
      ? (graph.loops ?? []).find((l) => l.from_step === selectedStepId)
      : null

  const loopSources = new Set((graph.loops ?? []).map((l) => l.from_step))

  const nodes: Node[] = graph.steps.map((_step, index) => ({
    id: _step.id,
    type: "step",
    position: { x: 0, y: index * (STEP_NODE_HEIGHT + STEP_GAP) },
    data: {
      step: _step,
      isLoopSource: loopSources.has(_step.id),
      onSelect: () => {
        setSelectedStepId(_step.id)
        setInspectorOpen(true)
      },
    },
  }))

  const edges: Edge[] = []
  for (let i = 0; i < graph.steps.length - 1; i++) {
    edges.push({
      id: `edge-${i}`,
      source: graph.steps[i].id,
      target: graph.steps[i + 1].id,
      animated: false,
    })
  }

  for (const loop of graph.loops ?? []) {
    const sourceIdx = graph.steps.findIndex((s) => s.id === loop.from_step)
    const targetIdx = graph.steps.findIndex((s) => s.id === loop.to_step)
    if (sourceIdx >= 0 && targetIdx >= 0) {
      edges.push({
        id: `loop-${loop.from_step}`,
        source: loop.from_step,
        target: loop.to_step,
        label: t("canvasLoopEdge"),
        animated: true,
        style: {
          stroke: "#ef4444",
          strokeDasharray: "5,5",
        },
      })
    }
  }

  /** Write the edited graph back to its row. Local state has already moved,
   *  so a failed write has to say so rather than pass silently. */
  const commit = (next: PipelineGraph) => {
    setGraph(next)
    onGraphChange?.(next)
    if (pipelineId == null || !saved) return
    void pipelineSave(
      {
        name: saved.name,
        folder_id: saved.folder_id ?? null,
        graph: next,
        isolation: saved.isolation ?? "worktree_per_run",
      },
      pipelineId
    )
      .then((updated) => setSaved(updated))
      .catch((e) => {
        console.error("[PipelineNode] failed to save the pipeline:", e)
        setValidationError(String(e))
      })
  }

  const handleAddStep = () => {
    const newStep = createDefaultStep(`step_${Date.now()}`, "coder", "New step")
    commit(addStep(graph, newStep))
  }

  const handleMoveStep = (stepId: string, direction: "up" | "down") => {
    const moved = moveStep(graph, stepId, direction)
    if (moved === graph) {
      // Refused: either an end of the chain, or the move would turn a
      // fix-round loop forwards.
      return
    }
    setValidationError(null)
    commit(moved)
  }

  const handleDeleteStep = (stepId: string) => {
    commit(removeStep(graph, stepId))
    if (selectedStepId === stepId) {
      setSelectedStepId(null)
      setInspectorOpen(false)
    }
  }

  const handleSaveStep = (step: PipelineStep, loop?: LoopBack | null) => {
    const updated = graph.steps.map((_s) => (_s.id === step.id ? step : _s))
    let newGraph = { ...graph, steps: updated }

    if (selectedLoop && !loop) {
      newGraph = removeLoop(newGraph, selectedStepId!)
    } else if (
      loop &&
      (!selectedLoop || selectedLoop.from_step !== loop.from_step)
    ) {
      newGraph = {
        ...newGraph,
        loops: (newGraph.loops ?? []).filter(
          (l) => l.from_step !== loop.from_step
        ),
      }
      newGraph.loops!.push(loop)
    }

    const validation = validateGraph(newGraph)
    if (!validation.valid) {
      setValidationError(validation.error.message ?? "Invalid pipeline")
      return
    }

    setValidationError(null)
    commit(newGraph)
    setInspectorOpen(false)
  }

  return (
    <div className="nodrag nowheel h-full flex flex-col gap-2 bg-background p-3 rounded-lg border border-foreground/10">
      {validationError && (
        <div className="flex items-start gap-2 rounded-md bg-destructive/10 p-2 text-xs text-destructive">
          <AlertCircle className="mt-0.5 size-3 flex-shrink-0" />
          <div>{validationError}</div>
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-auto rounded-md border border-foreground/10 bg-card">
        <ReactFlowProvider>
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={InnerNodeTypes}
            fitView
            attributionPosition="bottom-left"
          >
            <Background color="#aaa" gap={16} />
            <Controls />
          </ReactFlow>
        </ReactFlowProvider>
      </div>

      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          onClick={handleAddStep}
          className="flex-1"
        >
          <Plus className="size-3" />
          Add step
        </Button>
        {selectedStep && (
          <Button
            size="sm"
            variant="outline"
            onClick={() => handleDeleteStep(selectedStep.id)}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="size-3" />
          </Button>
        )}
      </div>

      {selectedStep && (
        <PipelineStepInspector
          step={selectedStep}
          loop={selectedLoop ?? null}
          availableLoopTargets={graph.steps.filter(
            (_s, i) =>
              i < graph.steps.findIndex((x) => x.id === selectedStep.id) &&
              (selectedStep.role === "reviewer" ||
                selectedStep.role === "tests")
          )}
          open={inspectorOpen}
          onClose={() => setInspectorOpen(false)}
          onSave={handleSaveStep}
          onMoveStep={handleMoveStep}
          onDeleteStep={() => handleDeleteStep(selectedStep.id)}
        />
      )}
    </div>
  )
}

export const PipelineNode = memo(function PipelineNode({
  data,
}: NodeProps<PipelineFlowNode>) {
  const { pipelineId } = data

  return (
    <div className="nodrag nowheel h-full w-full flex flex-col bg-card rounded-xl border border-foreground/15">
      <div className="px-4 py-3 border-b border-foreground/10 text-sm font-semibold">
        {data.label}
      </div>
      <div className="flex-1 min-h-0 overflow-hidden">
        <PipelineNodeContent pipelineId={pipelineId} />
      </div>
    </div>
  )
})
