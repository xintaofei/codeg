"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"

import { buildDag, type DagNode } from "@/lib/loop-dag"
import type {
  LoopArtifactRow,
  LoopArtifactStatus,
  LoopLinkRow,
} from "@/lib/types"
import { cn } from "@/lib/utils"

// Layout geometry (px). Columns are kind-fixed so x encodes the pipeline stage;
// rows pack each column top-to-bottom.
const COL_W = 196
const NODE_W = 168
const NODE_H = 58
const ROW_PITCH = NODE_H + 18
const PAD = 8

const nodeXY = (n: DagNode) => ({
  x: PAD + n.col * COL_W,
  y: PAD + n.row * ROW_PITCH,
})

const STATUS_DOT: Record<LoopArtifactStatus, string> = {
  pending: "bg-muted-foreground/40",
  in_progress: "bg-sky-500",
  awaiting_approval: "bg-amber-500",
  done: "bg-emerald-500",
  blocked: "bg-destructive",
  superseded: "bg-muted-foreground/30",
  cancelled: "bg-muted-foreground/30",
}

/**
 * Self-drawn layered DAG: an SVG layer renders provenance edges (derives_from
 * solid, skips_to dashed) behind absolutely-positioned HTML node cards. Nodes
 * with a live iteration (or `in_progress` status) pulse as "executing now".
 * Clicking a node opens its artifact drawer.
 */
export function DagGraph({
  artifacts,
  links,
  executingIds,
  onSelect,
}: {
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
  executingIds: Set<number>
  onSelect: (artifactId: number) => void
}) {
  const tKind = useTranslations("Loops.artifactKind")
  const tStatus = useTranslations("Loops.artifactStatus")
  const tDetail = useTranslations("Loops.issueDetail")

  const layout = useMemo(() => buildDag(artifacts, links), [artifacts, links])
  const box = useMemo(() => {
    const m = new Map<number, { x: number; y: number }>()
    for (const n of layout.nodes) m.set(n.artifact.id, nodeXY(n))
    return m
  }, [layout])

  if (layout.nodes.length === 0) return null

  const width = PAD * 2 + Math.max(layout.colCount - 1, 0) * COL_W + NODE_W
  const height = PAD * 2 + Math.max(layout.rowCount - 1, 0) * ROW_PITCH + NODE_H

  return (
    <div className="relative" style={{ width, height }}>
      <svg
        className="pointer-events-none absolute inset-0 text-muted-foreground"
        width={width}
        height={height}
        aria-hidden
      >
        {layout.edges.map((e) => {
          const a = box.get(e.from)
          const b = box.get(e.to)
          if (!a || !b) return null
          return (
            <path
              key={e.id}
              d={edgePath(a, b)}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeDasharray={e.dashed ? "4 4" : undefined}
              className={e.dashed ? "opacity-50" : "opacity-30"}
            />
          )
        })}
      </svg>

      {layout.nodes.map((n) => (
        <NodeCard
          key={n.artifact.id}
          node={n}
          executing={executingIds.has(n.artifact.id)}
          kindLabel={tKind(n.artifact.kind)}
          statusLabel={tStatus(n.artifact.status)}
          executingLabel={tDetail("executingNow")}
          onSelect={onSelect}
        />
      ))}
    </div>
  )
}

function NodeCard({
  node,
  executing,
  kindLabel,
  statusLabel,
  executingLabel,
  onSelect,
}: {
  node: DagNode
  executing: boolean
  kindLabel: string
  statusLabel: string
  executingLabel: string
  onSelect: (artifactId: number) => void
}) {
  const { artifact } = node
  const { x, y } = nodeXY(node)
  return (
    <button
      type="button"
      onClick={() => onSelect(artifact.id)}
      style={{ left: x, top: y, width: NODE_W, height: NODE_H }}
      aria-label={`${kindLabel}: ${artifact.title}`}
      className={cn(
        "absolute flex flex-col justify-center gap-1 rounded-lg border bg-card px-3 py-2 text-left shadow-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
        executing && "ring-2 ring-sky-500/50"
      )}
    >
      <div className="flex items-center gap-1.5">
        <span
          title={executing ? executingLabel : statusLabel}
          className={cn(
            "h-2 w-2 shrink-0 rounded-full",
            executing ? "animate-pulse bg-sky-500" : STATUS_DOT[artifact.status]
          )}
        />
        <span className="text-[0.625rem] uppercase tracking-wide text-muted-foreground">
          {kindLabel}
        </span>
      </div>
      <span className="truncate text-sm font-medium">{artifact.title}</span>
    </button>
  )
}

/**
 * Horizontal S-curve connecting the two node boxes on the sides that face each
 * other, so an edge never cuts through a node body. Edges run from a dependent
 * (right column) back to its source (left column).
 */
function edgePath(
  a: { x: number; y: number },
  b: { x: number; y: number }
): string {
  const acy = a.y + NODE_H / 2
  const bcy = b.y + NODE_H / 2
  const aRightOfB = a.x >= b.x
  const x1 = aRightOfB ? a.x : a.x + NODE_W
  const x2 = aRightOfB ? b.x + NODE_W : b.x
  const mx = (x1 + x2) / 2
  return `M ${x1} ${acy} C ${mx} ${acy}, ${mx} ${bcy}, ${x2} ${bcy}`
}
