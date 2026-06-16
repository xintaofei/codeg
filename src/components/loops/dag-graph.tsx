"use client"

import { useMemo, useState } from "react"
import { useTranslations } from "next-intl"

import { buildDag, foldReviews, type DagCluster } from "@/lib/loop-dag"
import type {
  LoopArtifactRow,
  LoopArtifactStatus,
  LoopLinkRow,
  LoopReviewVerdict,
} from "@/lib/types"
import { cn } from "@/lib/utils"

// Layout geometry (px). Read stages occupy fixed columns (x encodes the pipeline
// stage); task clusters fold in their reviews and stack as parallel lanes, with a
// depends_on chain running rightward across columns.
const COL_W = 208
const NODE_W = 176
const HEADER_H = 58
const ROW_PITCH = HEADER_H + 18
const PAD = 8
const LANE_GAP = 22
const REVIEW_H = 22
const REVIEW_PAD = 6
const REVIEW_DIVIDER = 1 // border-t between the task header and its reviews

/** Superseded / cancelled nodes are history; when revealed they render dimmed. */
const isDead = (s: LoopArtifactStatus): boolean =>
  s === "superseded" || s === "cancelled"

const STATUS_DOT: Record<LoopArtifactStatus, string> = {
  pending: "bg-muted-foreground/40",
  in_progress: "bg-sky-500",
  awaiting_approval: "bg-amber-500",
  done: "bg-emerald-500",
  blocked: "bg-destructive",
  superseded: "bg-muted-foreground/30",
  cancelled: "bg-muted-foreground/30",
}

/** Height of a cluster's folded reviews block (0 when the task has no reviews). */
function reviewsBlockHeight(reviews: LoopArtifactRow[]): number {
  const { latest, olderCount } = foldReviews(reviews)
  const rows = latest.length + (olderCount > 0 ? 1 : 0)
  return rows === 0 ? 0 : REVIEW_DIVIDER + REVIEW_PAD * 2 + rows * REVIEW_H
}

const clusterHeight = (c: DagCluster) =>
  HEADER_H + reviewsBlockHeight(c.reviews)

/**
 * Self-drawn DAG: an SVG layer renders provenance edges (derivation solid,
 * skips_to dashed, dependency subtle) behind absolutely-positioned HTML cards.
 * Read stages are fixed columns; each task is a *cluster* that folds in its own
 * reviews (latest attempt expanded, older attempts collapsed to a count), and
 * parallel task chains stack as lanes. Clicking any node opens its drawer.
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
  const tVerdict = useTranslations("Loops.reviewVerdict")
  const tDetail = useTranslations("Loops.issueDetail")

  // Dead nodes (superseded / cancelled) are hidden by default so the graph shows
  // the live plan; the toggle reveals them (dimmed) for audit.
  const [showSuperseded, setShowSuperseded] = useState(false)
  const layout = useMemo(
    () => buildDag(artifacts, links, { includeSuperseded: showSuperseded }),
    [artifacts, links, showSuperseded]
  )

  const geom = useMemo(() => {
    const stageLayout = layout.stageNodes.map((node) => ({
      node,
      x: PAD + node.col * COL_W,
      y: PAD + node.row * ROW_PITCH,
    }))

    // Lane bands: each lane is as tall as its tallest cluster; lanes stack with a
    // fixed gap so variable-height clusters never overlap the lane below.
    const laneHeight: number[] = Array(layout.laneCount).fill(0)
    for (const c of layout.clusters) {
      laneHeight[c.lane] = Math.max(laneHeight[c.lane], clusterHeight(c))
    }
    const laneY: number[] = []
    let acc = PAD
    for (let i = 0; i < layout.laneCount; i += 1) {
      laneY[i] = acc
      acc += laneHeight[i] + LANE_GAP
    }
    const clusterBandHeight = layout.laneCount ? acc - LANE_GAP - PAD : 0

    const clusterLayout = layout.clusters.map((cluster) => ({
      cluster,
      x: PAD + cluster.col * COL_W,
      y: laneY[cluster.lane],
      height: clusterHeight(cluster),
      fold: foldReviews(cluster.reviews),
    }))

    const stageBandHeight = layout.stageRowCount
      ? (layout.stageRowCount - 1) * ROW_PITCH + HEADER_H
      : 0
    const resultY = PAD + Math.max(0, (clusterBandHeight - HEADER_H) / 2)
    const resultLayout = layout.result
      ? {
          artifact: layout.result.artifact,
          x: PAD + layout.result.col * COL_W,
          y: resultY,
        }
      : null

    // Edge endpoints connect to each artifact's header rect (top of a cluster).
    const boxOf = new Map<number, { x: number; y: number }>()
    for (const s of stageLayout)
      boxOf.set(s.node.artifact.id, { x: s.x, y: s.y })
    for (const c of clusterLayout) {
      boxOf.set(c.cluster.task.id, { x: c.x, y: c.y })
    }
    if (resultLayout) {
      boxOf.set(resultLayout.artifact.id, {
        x: resultLayout.x,
        y: resultLayout.y,
      })
    }

    const contentHeight = Math.max(
      stageBandHeight,
      clusterBandHeight,
      resultLayout ? HEADER_H : 0
    )
    return {
      stageLayout,
      clusterLayout,
      resultLayout,
      boxOf,
      width: PAD * 2 + Math.max(layout.colCount - 1, 0) * COL_W + NODE_W,
      height: PAD * 2 + contentHeight,
    }
  }, [layout])

  const canvasEmpty =
    geom.stageLayout.length === 0 &&
    geom.clusterLayout.length === 0 &&
    !geom.resultLayout
  if (canvasEmpty && layout.supersededCount === 0) {
    return null
  }

  return (
    <div className="flex flex-col gap-2">
      {layout.supersededCount > 0 && (
        <button
          type="button"
          onClick={() => setShowSuperseded((v) => !v)}
          aria-pressed={showSuperseded}
          className="self-start rounded-md border px-2 py-1 text-xs text-muted-foreground outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
        >
          {showSuperseded
            ? tDetail("hideSuperseded")
            : tDetail("showSuperseded", { count: layout.supersededCount })}
        </button>
      )}
      <div
        className="relative"
        style={{ width: geom.width, height: geom.height }}
      >
        <svg
          className="pointer-events-none absolute inset-0 text-muted-foreground"
          width={geom.width}
          height={geom.height}
          aria-hidden
        >
          {layout.edges.map((e) => {
            const a = geom.boxOf.get(e.from)
            const b = geom.boxOf.get(e.to)
            if (!a || !b) return null
            return (
              <path
                key={e.id}
                d={edgePath(a, b)}
                fill="none"
                stroke="currentColor"
                strokeWidth={1.5}
                strokeDasharray={e.dashed ? "4 4" : undefined}
                className={
                  e.dashed
                    ? "opacity-50"
                    : e.kind === "depends_on"
                      ? "opacity-40"
                      : "opacity-25"
                }
              />
            )
          })}
        </svg>

        {geom.stageLayout.map(({ node, x, y }) => (
          <NodeCard
            key={node.artifact.id}
            artifact={node.artifact}
            x={x}
            y={y}
            executing={executingIds.has(node.artifact.id)}
            dimmed={isDead(node.artifact.status)}
            kindLabel={tKind(node.artifact.kind)}
            statusLabel={tStatus(node.artifact.status)}
            executingLabel={tDetail("executingNow")}
            onSelect={onSelect}
          />
        ))}

        {geom.resultLayout && (
          <NodeCard
            artifact={geom.resultLayout.artifact}
            x={geom.resultLayout.x}
            y={geom.resultLayout.y}
            executing={executingIds.has(geom.resultLayout.artifact.id)}
            dimmed={isDead(geom.resultLayout.artifact.status)}
            kindLabel={tKind(geom.resultLayout.artifact.kind)}
            statusLabel={tStatus(geom.resultLayout.artifact.status)}
            executingLabel={tDetail("executingNow")}
            onSelect={onSelect}
          />
        )}

        {geom.clusterLayout.map(({ cluster, x, y, height, fold }) => (
          <ClusterCard
            key={cluster.task.id}
            cluster={cluster}
            fold={fold}
            x={x}
            y={y}
            height={height}
            dimmed={isDead(cluster.task.status)}
            executingIds={executingIds}
            kindLabel={tKind(cluster.task.kind)}
            reviewKindLabel={tKind("review")}
            statusLabelOf={(s) => tStatus(s)}
            verdictLabelOf={(v) => tVerdict(v)}
            executingLabel={tDetail("executingNow")}
            olderLabelOf={(count) => tDetail("reviewsOlder", { count })}
            onSelect={onSelect}
          />
        ))}
      </div>
    </div>
  )
}

function StatusDot({
  status,
  executing,
  title,
}: {
  status: LoopArtifactStatus
  executing: boolean
  title: string
}) {
  return (
    <span
      title={title}
      className={cn(
        "h-2 w-2 shrink-0 rounded-full",
        executing ? "animate-pulse bg-sky-500" : STATUS_DOT[status]
      )}
    />
  )
}

/** A read-stage (issue/requirement/design) or result node. */
function NodeCard({
  artifact,
  x,
  y,
  executing,
  dimmed,
  kindLabel,
  statusLabel,
  executingLabel,
  onSelect,
}: {
  artifact: LoopArtifactRow
  x: number
  y: number
  executing: boolean
  dimmed: boolean
  kindLabel: string
  statusLabel: string
  executingLabel: string
  onSelect: (artifactId: number) => void
}) {
  return (
    <button
      type="button"
      onClick={() => onSelect(artifact.id)}
      style={{ left: x, top: y, width: NODE_W, height: HEADER_H }}
      aria-label={`${kindLabel}: ${artifact.title}`}
      className={cn(
        "absolute flex flex-col justify-center gap-1 rounded-lg border bg-card px-3 py-2 text-left shadow-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
        executing && "ring-2 ring-sky-500/50",
        dimmed && "opacity-50"
      )}
    >
      <div className="flex items-center gap-1.5">
        <StatusDot
          status={artifact.status}
          executing={executing}
          title={executing ? executingLabel : statusLabel}
        />
        <span className="text-[0.625rem] uppercase tracking-wide text-muted-foreground">
          {kindLabel}
        </span>
      </div>
      <span className="truncate text-sm font-medium">{artifact.title}</span>
    </button>
  )
}

/** A task and its reviews, rendered as one bordered cluster. */
function ClusterCard({
  cluster,
  fold,
  x,
  y,
  height,
  dimmed,
  executingIds,
  kindLabel,
  reviewKindLabel,
  statusLabelOf,
  verdictLabelOf,
  executingLabel,
  olderLabelOf,
  onSelect,
}: {
  cluster: DagCluster
  fold: { latest: LoopArtifactRow[]; olderCount: number }
  x: number
  y: number
  height: number
  dimmed: boolean
  executingIds: Set<number>
  kindLabel: string
  reviewKindLabel: string
  statusLabelOf: (s: LoopArtifactStatus) => string
  verdictLabelOf: (v: LoopReviewVerdict) => string
  executingLabel: string
  olderLabelOf: (count: number) => string
  onSelect: (artifactId: number) => void
}) {
  const { task } = cluster
  const taskExecuting = executingIds.has(task.id)
  const hasReviews = fold.latest.length > 0 || fold.olderCount > 0
  return (
    <div
      style={{ left: x, top: y, width: NODE_W, height }}
      className={cn(
        "absolute flex flex-col overflow-hidden rounded-lg border bg-card shadow-sm",
        dimmed && "opacity-50"
      )}
    >
      <button
        type="button"
        onClick={() => onSelect(task.id)}
        style={{ height: HEADER_H }}
        aria-label={`${kindLabel}: ${task.title}`}
        className={cn(
          "flex flex-col justify-center gap-1 px-3 py-2 text-left outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
          taskExecuting && "ring-2 ring-inset ring-sky-500/50"
        )}
      >
        <div className="flex items-center gap-1.5">
          <StatusDot
            status={task.status}
            executing={taskExecuting}
            title={taskExecuting ? executingLabel : statusLabelOf(task.status)}
          />
          <span className="text-[0.625rem] uppercase tracking-wide text-muted-foreground">
            {kindLabel}
          </span>
        </div>
        <span className="truncate text-sm font-medium">{task.title}</span>
      </button>

      {hasReviews && (
        <div
          className="flex flex-col gap-0 border-t bg-muted/30"
          style={{ paddingTop: REVIEW_PAD, paddingBottom: REVIEW_PAD }}
        >
          {fold.latest.map((review) => {
            const executing = executingIds.has(review.id)
            // Row text keeps the artifact title so sibling reviews stay distinct;
            // the pass/fail outcome shows as a shape glyph (✓/✗) — not color alone
            // — and is named in the accessible label + tooltip.
            const verdictLabel = review.verdict
              ? verdictLabelOf(review.verdict)
              : null
            const statusLabel = executing
              ? executingLabel
              : statusLabelOf(review.status)
            return (
              <button
                key={review.id}
                type="button"
                onClick={() => onSelect(review.id)}
                style={{ height: REVIEW_H }}
                aria-label={
                  verdictLabel
                    ? `${reviewKindLabel}: ${review.title} — ${verdictLabel}`
                    : `${reviewKindLabel}: ${review.title}`
                }
                title={verdictLabel ?? statusLabel}
                className={cn(
                  "flex items-center gap-1.5 px-3 text-left outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                  // A dead review folded under a live task is dimmed on its own;
                  // when the task itself is dead the whole cluster is already dimmed.
                  isDead(review.status) && "opacity-50"
                )}
              >
                <span
                  className={cn(
                    "h-2 w-2 shrink-0 rounded-full",
                    executing
                      ? "animate-pulse bg-sky-500"
                      : STATUS_DOT[review.status]
                  )}
                />
                <span className="flex-1 truncate text-xs text-muted-foreground">
                  {review.title}
                </span>
                {review.verdict && (
                  <span
                    aria-hidden
                    className={cn(
                      "shrink-0 text-xs font-semibold leading-none",
                      review.verdict === "pass"
                        ? "text-emerald-600"
                        : "text-destructive"
                    )}
                  >
                    {review.verdict === "pass" ? "✓" : "✗"}
                  </span>
                )}
              </button>
            )
          })}
          {fold.olderCount > 0 && (
            <span
              style={{ height: REVIEW_H }}
              className="flex items-center px-3 text-[0.625rem] uppercase tracking-wide text-muted-foreground/70"
            >
              {olderLabelOf(fold.olderCount)}
            </span>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * Horizontal S-curve connecting two header rects on the sides that face each
 * other, so an edge never cuts through a node body. Edges run from a dependent
 * (right) back to its source (left).
 */
function edgePath(
  a: { x: number; y: number },
  b: { x: number; y: number }
): string {
  const acy = a.y + HEADER_H / 2
  const bcy = b.y + HEADER_H / 2
  const aRightOfB = a.x >= b.x
  const x1 = aRightOfB ? a.x : a.x + NODE_W
  const x2 = aRightOfB ? b.x + NODE_W : b.x
  const mx = (x1 + x2) / 2
  return `M ${x1} ${acy} C ${mx} ${acy}, ${mx} ${bcy}, ${x2} ${bcy}`
}
