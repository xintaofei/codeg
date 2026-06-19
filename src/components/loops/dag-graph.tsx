"use client"

import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { TriangleAlert } from "lucide-react"

import { AgentIcon } from "@/components/agent-icon"
import { foldReviews, placeGhosts } from "@/lib/loop-dag"
import {
  buildProcessGraph,
  type ArtifactIterationRef,
  type ArtifactNode,
  type IterationRef,
  type Phase,
  type PhaseConnector,
  type PhasePending,
  type PhaseState,
  type ProcessGraph,
} from "@/lib/loop-process-graph"
import type { AttentionKey } from "@/lib/loop-attention"
import { AGENT_LABELS } from "@/lib/types"
import type {
  AgentType,
  LoopArtifactRow,
  LoopArtifactStatus,
  LoopInboxItemRow,
  LoopIterationOutcome,
  LoopIterationRow,
  LoopLinkRow,
  LoopReviewVerdict,
  LoopStage,
} from "@/lib/types"
import { cn } from "@/lib/utils"

// ---------------------------------------------------------------------------
// Layout geometry (px). The macro layout is six phase containers laid out left
// → right; Implement alone has rich internal structure (the `depends_on` task
// forest, folded reviews, plan ghosts). Inter-phase lineage is folded to ONE
// connector per phase pair, drawn between container boundaries. Hiding dead
// nodes is a pure render-layer choice (the model is built once, toggle-free).
// ---------------------------------------------------------------------------
const PAD = 8 // canvas margin
const COL_W = 208 // Implement-internal column pitch (a depends_on step)
const NODE_W = 176 // a card's width
const HEADER_H = 58 // a card header's height
const ROW_PITCH = HEADER_H + 18 // stacked-member vertical pitch
const GHOST_GAP = ROW_PITCH - HEADER_H // gap above a column's first ghost
const LANE_GAP = 22 // gap between Implement lanes
const REVIEW_H = 22
const REVIEW_PAD = 6
const REVIEW_DIVIDER = 1 // border-t between a task header and its reviews

const PHASE_HEADER_H = 30 // a phase container's title bar
const PHASE_PAD = 12 // a phase container's inner padding
const PHASE_GAP = 72 // horizontal gap between phase containers (connector room)
const PLACEHOLDER_W = 116 // an empty phase's slim placeholder box
const PLACEHOLDER_H = 72
const SKIP_LANE_GAP = 18 // gap below the boxes before the skip-routing lane
const SKIP_LANE_H = 28 // reserved band a skip connector dips into
const SESSION_CHIP_H = 30 // a live triage/finalize session chip (Issue / Result)
const SESSION_GAP = 8 // gap above the first session chip / between chips
const SESSION_PITCH = SESSION_CHIP_H + SESSION_GAP

/** Human-readable agent name for an `AgentType`, falling back to the raw wire
 *  value for an unknown (future) agent so the label degrades instead of blanking. */
const agentName = (a: AgentType): string => AGENT_LABELS[a] ?? a

/** Superseded / cancelled nodes are history; when revealed they render dimmed. */
const isDead = (s: LoopArtifactStatus): boolean =>
  s === "superseded" || s === "cancelled"

/** Per-artifact status → dot color (a node's own lifecycle). */
const STATUS_DOT: Record<LoopArtifactStatus, string> = {
  pending: "bg-muted-foreground/40",
  in_progress: "bg-sky-500",
  awaiting_approval: "bg-amber-500",
  done: "bg-emerald-500",
  blocked: "bg-destructive",
  superseded: "bg-muted-foreground/30",
  cancelled: "bg-muted-foreground/30",
}

/** Per-phase rollup state → dot color. Distinct from {@link STATUS_DOT}: a phase
 *  has `active` (any member in flight / a pending ghost) and `empty` states that
 *  no single artifact status carries. */
const PHASE_STATE_DOT: Record<PhaseState, string> = {
  blocked: "bg-destructive",
  awaiting_approval: "bg-amber-500",
  active: "bg-sky-500",
  pending: "bg-muted-foreground/40",
  done: "bg-emerald-500",
  empty: "bg-muted-foreground/20",
}

/** A task and the reviews to render under it, decoupled from the model so the
 *  renderer no longer depends on the retired `DagCluster` shape. */
interface TaskClusterView {
  task: LoopArtifactRow
  fold: { latest: LoopArtifactRow[]; olderCount: number }
}

/**
 * The single ring a node shows, in priority order: a transient locate pulse wins
 * (so a just-located node is unmistakable), then an attention ring (amber — a
 * pending inbox card concerns it, D8), then the executing ring (sky). `inset` is
 * used inside a bordered cluster header so the ring doesn't clip.
 */
function nodeRingClass(
  opts: { pulsing: boolean; attention: boolean; executing: boolean },
  inset = false
): string {
  const i = inset ? " ring-inset" : ""
  if (opts.pulsing)
    return "ring-2 ring-sky-400 ring-offset-2 ring-offset-background animate-pulse"
  if (opts.attention) return `ring-2 ring-amber-500/70${i}`
  if (opts.executing) return `ring-2 ring-sky-500/50${i}`
  return ""
}

/** A small amber alert glyph marking a node that has pending inbox cards (D8).
 *  Decorative; the count/meaning rides on the node's title + aria-label. */
function AttentionMark() {
  return (
    <TriangleAlert
      aria-hidden
      className="h-3 w-3 shrink-0 text-amber-600 dark:text-amber-400"
    />
  )
}

/** Height of a task's folded reviews block (0 when it has none). */
function reviewsBlockHeight(reviews: LoopArtifactRow[]): number {
  const { latest, olderCount } = foldReviews(reviews)
  const rows = latest.length + (olderCount > 0 ? 1 : 0)
  return rows === 0 ? 0 : REVIEW_DIVIDER + REVIEW_PAD * 2 + rows * REVIEW_H
}

// --- geometry layout types -------------------------------------------------

interface MemberLayout {
  node: ArtifactNode
  x: number
  y: number
  height: number
  /** Folded reviews — only meaningful for Implement task members. */
  fold: { latest: LoopArtifactRow[]; olderCount: number }
}
interface PendingLayout {
  pending: PhasePending
  x: number
  y: number
}
interface SessionLayout {
  ref: IterationRef
  x: number
  y: number
}
interface PhaseBoxLayout {
  phase: Phase
  /** Solid box (has visible content) vs slim placeholder (empty phase). */
  solid: boolean
  x: number
  y: number
  width: number
  height: number
  members: MemberLayout[]
  pending: PendingLayout[]
  /** Live artifact-less session chips (Issue triage / Result finalize, P3). */
  sessions: SessionLayout[]
}
interface WorkflowEdgeLayout {
  id: string
  from: { x: number; y: number }
  to: { x: number; y: number }
}
interface ConnectorLayout {
  connector: PhaseConnector
  path: string
  dashed: boolean
  badgeX: number
  badgeY: number
  /** Count shown on the badge: total when revealing dead, else active. */
  visibleCount: number
  /** Links folded out of view (dead endpoint) under the current toggle. */
  hiddenCount: number
}
interface GraphGeom {
  width: number
  height: number
  boxes: PhaseBoxLayout[]
  workflowEdges: WorkflowEdgeLayout[]
  connectors: ConnectorLayout[]
}

interface PreLayout {
  phase: Phase
  solid: boolean
  contentW: number
  contentH: number
  memberLocal: Array<{
    node: ArtifactNode
    lx: number
    ly: number
    height: number
    fold: { latest: LoopArtifactRow[]; olderCount: number }
  }>
  pendingLocal: Array<{ pending: PhasePending; lx: number; ly: number }>
  sessionLocal: Array<{ ref: IterationRef; lx: number; ly: number }>
}

/** Visible rows of a node's reviews under the current toggle (dead hidden by
 *  default, revealed dimmed when the toggle is on). */
function visibleReviewRows(
  node: ArtifactNode,
  showSuperseded: boolean
): LoopArtifactRow[] {
  const src = showSuperseded
    ? node.reviews
    : node.reviews.filter((r) => !r.dead)
  return src.map((r) => r.artifact)
}

/**
 * Phase-internal content metrics + local placements (origin at the content
 * area's top-left). Implement lays its task forest out by col/lane (lanes
 * compacted over the visible set so hiding a dead chain leaves no gap) plus
 * plan ghosts beneath; other phases stack members then pending ghosts.
 */
function prelayoutPhase(phase: Phase, showSuperseded: boolean): PreLayout {
  const visibleMembers = showSuperseded
    ? phase.members
    : phase.members.filter((m) => !m.dead)
  // session_only gate (Codex major): a live artifact-less session keeps a phase
  // solid (not a slim placeholder) even with no members/pending — its chip needs
  // somewhere to render.
  const solid =
    visibleMembers.length > 0 ||
    phase.pending.length > 0 ||
    phase.sessionRefs.length > 0

  if (phase.kind === "implement") {
    // Compact the (possibly sparse after hiding dead) lanes to 0..n-1.
    const lanesPresent = [...new Set(visibleMembers.map((m) => m.lane))].sort(
      (a, b) => a - b
    )
    const laneIndex = new Map(lanesPresent.map((l, i) => [l, i]))
    const laneHeight: number[] = new Array(lanesPresent.length).fill(0)
    const memberLocal = visibleMembers.map((node) => {
      const reviews = visibleReviewRows(node, showSuperseded)
      const height = HEADER_H + reviewsBlockHeight(reviews)
      const li = laneIndex.get(node.lane)!
      laneHeight[li] = Math.max(laneHeight[li], height)
      return { node, reviews, height, fold: foldReviews(reviews), li }
    })
    const laneY: number[] = []
    let acc = 0
    for (let i = 0; i < lanesPresent.length; i += 1) {
      laneY[i] = acc
      acc += laneHeight[i] + LANE_GAP
    }
    const laneBandBottom = lanesPresent.length ? acc - LANE_GAP : 0

    const placedMembers = memberLocal.map((m) => ({
      node: m.node,
      lx: m.node.col * COL_W,
      ly: laneY[m.li],
      height: m.height,
      fold: m.fold,
    }))

    // Plan ghosts stack beneath the first task column (mirrors the old layout),
    // measured against that column's real-node bottom so they never overlap.
    const columnBottom = new Map<number, number>()
    for (const m of placedMembers) {
      const prev = columnBottom.get(m.node.col) ?? 0
      columnBottom.set(m.node.col, Math.max(prev, m.ly + m.height))
    }
    const ghostInputs = phase.pending.map((p, i) => ({
      iterationId: p.iterationId,
      col: 0,
      row: i,
    }))
    const ghostY = placeGhosts(ghostInputs, columnBottom, {
      pad: 0,
      rowPitch: ROW_PITCH,
      gap: GHOST_GAP,
    })
    const pendingLocal = phase.pending.map((p) => ({
      pending: p,
      lx: 0,
      ly: ghostY.get(p.iterationId) ?? 0,
    }))

    const ghostBottom = pendingLocal.reduce(
      (m, p) => Math.max(m, p.ly + HEADER_H),
      0
    )
    const maxColRight = placedMembers.reduce(
      (m, p) => Math.max(m, p.node.col * COL_W + NODE_W),
      0
    )
    const contentW = Math.max(maxColRight, pendingLocal.length ? NODE_W : 0)
    const contentH = Math.max(laneBandBottom, ghostBottom)
    return {
      phase,
      solid,
      contentW,
      contentH,
      memberLocal: placedMembers,
      pendingLocal,
      sessionLocal: [], // Implement carries no artifact-less sessions.
    }
  }

  // issue / requirement / design / result / reflect: a vertical stack of member
  // cards, then pending ghosts, then live session chips (Issue triage / Result
  // finalize — the artifact-less phase history, P3).
  const memberLocal = visibleMembers.map((node, i) => ({
    node,
    lx: 0,
    ly: i * ROW_PITCH,
    height: HEADER_H,
    fold: foldReviews([]),
  }))
  const pendingLocal = phase.pending.map((p, i) => ({
    pending: p,
    lx: 0,
    ly: (visibleMembers.length + i) * ROW_PITCH,
  }))
  const nodeRows = visibleMembers.length + phase.pending.length
  const nodeStackBottom =
    nodeRows > 0 ? (nodeRows - 1) * ROW_PITCH + HEADER_H : 0
  // Chips sit below the node stack (or at the top when the phase is session-only).
  const sessionTop = nodeRows > 0 ? nodeStackBottom + SESSION_GAP : 0
  const sessionLocal = phase.sessionRefs.map((ref, i) => ({
    ref,
    lx: 0,
    ly: sessionTop + i * SESSION_PITCH,
  }))
  const sessionBottom = sessionLocal.length
    ? sessionTop + (sessionLocal.length - 1) * SESSION_PITCH + SESSION_CHIP_H
    : 0
  const contentH = Math.max(nodeStackBottom, sessionBottom)
  const contentW = nodeRows > 0 || sessionLocal.length > 0 ? NODE_W : 0
  return {
    phase,
    solid,
    contentW,
    contentH,
    memberLocal,
    pendingLocal,
    sessionLocal,
  }
}

/** S-curve between two box edges that face each other (earlier right → later
 *  left), so a lineage connector never cuts through a box body. */
function phaseConnectorPath(
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number }
): string {
  const x1 = a.x + a.width
  const y1 = a.y + a.height / 2
  const x2 = b.x
  const y2 = b.y + b.height / 2
  const mx = (x1 + x2) / 2
  return `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}`
}

/** A skip connector dips into a reserved lane BELOW the boxes and runs across
 *  it, so it visibly routes around any phase(s) it skips rather than appearing
 *  to hang off the box in between. */
function skipConnectorPath(
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number },
  laneY: number
): string {
  const x1 = a.x + a.width
  const y1 = a.y + a.height / 2
  const x2 = b.x
  const y2 = b.y + b.height / 2
  const mx = (x1 + x2) / 2
  return `M ${x1} ${y1} C ${x1 + 40} ${y1}, ${x1 + 40} ${laneY}, ${mx} ${laneY} C ${x2 - 40} ${laneY}, ${x2 - 40} ${y2}, ${x2} ${y2}`
}

/** The model's connector counts under the toggle: total when revealing dead,
 *  else active. The connector renders iff this is > 0 (no zero-count lines). */
const connectorVisibleCount = (c: PhaseConnector, showSuperseded: boolean) =>
  showSuperseded ? c.totalCount : c.activeCount

/**
 * Lay the whole graph out for a given toggle. Pure: depends only on the model
 * and `showSuperseded`. Boxes are top-aligned left → right; members/pending get
 * absolute canvas coords; connectors get boundary paths + a midpoint badge.
 */
function layoutGraph(graph: ProcessGraph, showSuperseded: boolean): GraphGeom {
  const y0 = PAD
  const pre = graph.phases.map((p) => prelayoutPhase(p, showSuperseded))

  // Box sizes + left → right x positions.
  let x = PAD
  const boxes: PhaseBoxLayout[] = pre.map((p) => {
    const width = p.solid ? PHASE_PAD * 2 + p.contentW : PLACEHOLDER_W
    const height = p.solid
      ? PHASE_HEADER_H + PHASE_PAD * 2 + p.contentH
      : PLACEHOLDER_H
    const bx = x
    x += width + PHASE_GAP
    const contentLeft = bx + PHASE_PAD
    const contentTop = y0 + PHASE_HEADER_H + PHASE_PAD
    return {
      phase: p.phase,
      solid: p.solid,
      x: bx,
      y: y0,
      width,
      height,
      members: p.memberLocal.map((m) => ({
        node: m.node,
        x: contentLeft + m.lx,
        y: contentTop + m.ly,
        height: m.height,
        fold: m.fold,
      })),
      pending: p.pendingLocal.map((pl) => ({
        pending: pl.pending,
        x: contentLeft + pl.lx,
        y: contentTop + pl.ly,
      })),
      sessions: p.sessionLocal.map((s) => ({
        ref: s.ref,
        x: contentLeft + s.lx,
        y: contentTop + s.ly,
      })),
    }
  })

  const canvasRight = x - PHASE_GAP + PAD
  const maxBoxH = boxes.reduce((m, b) => Math.max(m, b.height), 0)
  const boxByKind = new Map(boxes.map((b) => [b.phase.kind, b]))

  // Implement-internal workflow edges (depends_on), between visible task cards.
  const implBox = boxByKind.get("implement")
  const memberPos = new Map<number, { x: number; y: number }>()
  if (implBox)
    for (const m of implBox.members)
      memberPos.set(m.node.artifact.id, { x: m.x, y: m.y })
  const implPhase = graph.phases.find((p) => p.kind === "implement")
  const workflowEdges: WorkflowEdgeLayout[] = []
  if (implPhase) {
    for (const e of implPhase.workflow) {
      const from = memberPos.get(e.from)
      const to = memberPos.get(e.to)
      if (!from || !to) continue
      workflowEdges.push({ id: `${e.from}-${e.to}`, from, to })
    }
  }

  // Folded lineage connectors. A reserved lane below the boxes routes skips.
  const hasSkip = graph.connectors.some(
    (c) =>
      c.connectorKind === "skip" && connectorVisibleCount(c, showSuperseded) > 0
  )
  const skipLaneY = y0 + maxBoxH + SKIP_LANE_GAP + SKIP_LANE_H / 2
  const connectors: ConnectorLayout[] = []
  let skipIdx = 0
  for (const c of graph.connectors) {
    const visibleCount = connectorVisibleCount(c, showSuperseded)
    if (visibleCount <= 0) continue // no zero-count lines when dead are hidden
    const a = boxByKind.get(c.earlier)
    const b = boxByKind.get(c.later)
    if (!a || !b) continue
    const hiddenCount = c.totalCount - c.activeCount
    if (c.connectorKind === "skip") {
      const laneY = skipLaneY + skipIdx * 10
      skipIdx += 1
      connectors.push({
        connector: c,
        path: skipConnectorPath(a, b, laneY),
        dashed: true,
        badgeX: (a.x + a.width + b.x) / 2,
        badgeY: laneY,
        visibleCount,
        hiddenCount,
      })
    } else {
      connectors.push({
        connector: c,
        path: phaseConnectorPath(a, b),
        dashed: false,
        badgeX: (a.x + a.width + b.x) / 2,
        badgeY: (a.y + a.height / 2 + (b.y + b.height / 2)) / 2,
        visibleCount,
        hiddenCount,
      })
    }
  }

  const canvasBottom =
    (hasSkip ? skipLaneY + SKIP_LANE_H / 2 : y0 + maxBoxH) + PAD
  return {
    width: canvasRight,
    height: canvasBottom,
    boxes,
    workflowEdges,
    connectors,
  }
}

/**
 * Self-drawn process graph: six phase containers laid left → right, with
 * Implement holding the `depends_on` task forest (folded reviews, plan ghosts)
 * internally. The 1→N / N→1 lineage that once sprawled as an edge-soup is folded
 * into one connector per phase pair, drawn between container boundaries with a
 * focusable midpoint badge whose tooltip traces every underlying link. Clicking
 * any node opens its drawer; clicking a ghost opens its live iteration session.
 */
export function DagGraph({
  artifacts,
  links,
  liveIterations,
  executingIds,
  attentionMap,
  focus,
  onFocusConsumed,
  onSelect,
  onOpenIteration,
  artifactIterationRefs,
  onOpenSession,
}: {
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
  /** queued|running iterations — drives ghost nodes for in-flight stages. */
  liveIterations: LoopIterationRow[]
  /** Namespaced executing keys (`artifact:{id}`) for nodes with a live iteration. */
  executingIds: Set<string>
  /** Pending inbox cards keyed by the node they concern (D8). A node whose
   *  `artifact:{id}` key has cards shows an amber attention ring + alert glyph. */
  attentionMap?: Map<AttentionKey, LoopInboxItemRow[]>
  /** A locate request: scroll this artifact's node into view and pulse it, then
   *  call `onFocusConsumed`. Replayed on layout changes so it lands even when the
   *  graph mounts after the request (Codex r1 I6). */
  focus?: number | null
  onFocusConsumed?: () => void
  onSelect: (artifactId: number) => void
  /** Open a ghost's live iteration session (when it has a conversation). */
  onOpenIteration?: (pending: PhasePending) => void
  /** Resolved producing-iteration refs (P3 agent facet). An array (incl. `[]`)
   *  turns the facet on (`Array.isArray` probe); absent/null ⇒ facet off. */
  artifactIterationRefs?: readonly ArtifactIterationRef[] | null
  /** Open a live artifact-less session (Issue triage / Result finalize chip). */
  onOpenSession?: (session: {
    conversationId: number
    agentType?: AgentType | null
    outcome?: LoopIterationOutcome | null
    stage?: LoopStage
  }) => void
}) {
  const tKind = useTranslations("Loops.artifactKind")
  const tStatus = useTranslations("Loops.artifactStatus")
  const tVerdict = useTranslations("Loops.reviewVerdict")
  const tDetail = useTranslations("Loops.issueDetail")
  const tDag = useTranslations("Loops.dag")
  const tPhase = useTranslations("Loops.phase")
  const tPhaseState = useTranslations("Loops.phaseState")
  const tStage = useTranslations("Loops.stage")

  // The model is toggle-independent: built once, includes dead nodes flagged.
  const graph = useMemo(
    () =>
      buildProcessGraph({
        artifacts,
        links,
        liveIterations,
        artifactIterationRefs,
      }),
    [artifacts, links, liveIterations, artifactIterationRefs]
  )

  // Dead nodes (superseded / cancelled) are hidden by default so the graph shows
  // the live plan; the toggle reveals them (dimmed) for audit. This is a pure
  // render-layer choice — the geometry recomputes, the model does not.
  const [showSuperseded, setShowSuperseded] = useState(false)
  const geom = useMemo(
    () => layoutGraph(graph, showSuperseded),
    [graph, showSuperseded]
  )

  // Connector tooltips resolve endpoint titles from the FULL artifact set
  // (top-level members AND reviews folded into clusters), since a review is a
  // first-class folded node that can be a lineage endpoint.
  const labelById = useMemo(
    () => new Map(artifacts.map((a) => [a.id, a])),
    [artifacts]
  )

  // P3 agent facet: a flat lookup of each node's (and folded review's) resolved
  // agent + attempt count, keeping the cards decoupled from the geometry/fold.
  // Gated by agentFacetAvailable — facet off ⇒ every lookup is null/null (no
  // icons/badges, P1 appearance preserved).
  const facetOf = useMemo(() => {
    const m = new Map<
      number,
      { agentType: AgentType | null; attemptCount: number | null }
    >()
    if (graph.agentFacetAvailable) {
      for (const p of graph.phases)
        for (const node of p.members) {
          m.set(node.artifact.id, {
            agentType: node.producedBy?.agentType ?? null,
            attemptCount: node.attemptCount,
          })
          for (const r of node.reviews)
            m.set(r.artifact.id, {
              agentType: r.producedBy?.agentType ?? null,
              attemptCount: r.attemptCount,
            })
        }
    }
    return (id: number) => m.get(id) ?? { agentType: null, attemptCount: null }
  }, [graph])

  // Locate-in-graph: when a `focus` request lands and its node is rendered,
  // scroll to it and pulse it for a moment, then consume the request. Re-runs on
  // geometry changes so a focus set before the data arrived still resolves; if
  // the node never renders (e.g. a focus on a hidden superseded node), the
  // request is left for a later layout — the drawer remains the reliable locator.
  const rootRef = useRef<HTMLDivElement>(null)
  const [pulsingId, setPulsingId] = useState<number | null>(null)
  const pulseTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // The graph has data once any node would render; only then is a missing focus
  // target genuinely absent (vs. still loading / mounting after the request).
  const layoutReady =
    graph.phases.some((p) => p.members.length > 0 || p.pending.length > 0) ||
    graph.supersededCount > 0
  useEffect(() => {
    if (focus == null) return
    const el = rootRef.current?.querySelector<HTMLElement>(
      `[data-artifact-id="${focus}"]`
    )
    if (!el) {
      if (layoutReady) onFocusConsumed?.()
      return
    }
    el.scrollIntoView({ block: "center", inline: "center" })
    // Reacting to an external locate request (URL nav) by scrolling the DOM and
    // flashing a transient pulse — a legitimate effect→setState.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setPulsingId(focus)
    if (pulseTimer.current) clearTimeout(pulseTimer.current)
    pulseTimer.current = setTimeout(() => setPulsingId(null), 1600)
    onFocusConsumed?.()
  }, [focus, geom, layoutReady, onFocusConsumed])
  useEffect(
    () => () => {
      if (pulseTimer.current) clearTimeout(pulseTimer.current)
    },
    []
  )

  // Per-node attention count. The issue root additionally surfaces issue-level
  // cards (budget / dependency / coverage / triage / reflect) that
  // `buildAttentionMap` roots at "issue-root"; without this they'd be grouped but
  // never ring any node (Codex r2).
  const nodeAttentionCount = (a: LoopArtifactRow): number =>
    (attentionMap?.get(`artifact:${a.id}`)?.length ?? 0) +
    (a.kind === "issue" ? (attentionMap?.get("issue-root")?.length ?? 0) : 0)

  const unmappedCount = graph.unmappedArtifacts + graph.unmappedIterations
  const hasAnyMember = graph.phases.some((p) => p.members.length > 0)
  const hasAnyPending = graph.phases.some((p) => p.pending.length > 0)
  // A brand-new issue can be just a live triage session (no artifacts/ghosts yet);
  // its session chip must still render, so sessions count toward "has content".
  const hasAnySession = graph.phases.some((p) => p.sessionRefs.length > 0)
  if (
    !hasAnyMember &&
    !hasAnyPending &&
    !hasAnySession &&
    graph.supersededCount === 0
  ) {
    return null
  }

  return (
    <div ref={rootRef} className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {graph.supersededCount > 0 && (
          <button
            type="button"
            onClick={() => setShowSuperseded((v) => !v)}
            aria-pressed={showSuperseded}
            className="rounded-md border px-2 py-1 text-xs text-muted-foreground outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
          >
            {showSuperseded
              ? tDetail("hideSuperseded")
              : tDetail("showSuperseded", { count: graph.supersededCount })}
          </button>
        )}
        {unmappedCount > 0 && (
          <span className="inline-flex items-center gap-1 rounded-md border border-amber-500/40 px-2 py-1 text-xs text-muted-foreground">
            <AttentionMark />
            {tDag("unmappedNodes", { count: unmappedCount })}
          </span>
        )}
      </div>

      <div
        className="relative"
        style={{ width: geom.width, height: geom.height }}
      >
        {/* Phase containers (background). */}
        {geom.boxes.map((box) => (
          <PhaseContainer
            key={box.phase.kind}
            box={box}
            name={tPhase(box.phase.kind)}
            stateLabel={tPhaseState(box.phase.state)}
          />
        ))}

        {/* Decorative edges: folded-lineage connectors + Implement-internal
            depends_on workflow. The interactive/labeled element is the badge. */}
        <svg
          className="pointer-events-none absolute inset-0 text-muted-foreground"
          width={geom.width}
          height={geom.height}
          aria-hidden
        >
          {geom.connectors.map((c) => (
            <path
              key={`conn:${c.connector.earlier}-${c.connector.later}-${c.connector.connectorKind}`}
              d={c.path}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeDasharray={c.dashed ? "5 4" : undefined}
              className={c.dashed ? "opacity-50" : "opacity-30"}
            />
          ))}
          {geom.workflowEdges.map((e) => (
            <path
              key={`wf:${e.id}`}
              d={edgePath(e.from, e.to)}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              className="opacity-40"
            />
          ))}
        </svg>

        {/* Connector badges — the focusable, labeled hit target per connector. */}
        {geom.connectors.map((c) => (
          <ConnectorBadge
            key={`badge:${c.connector.earlier}-${c.connector.later}-${c.connector.connectorKind}`}
            layout={c}
            label={connectorAriaLabel(c, showSuperseded, {
              earlier: tPhase(c.connector.earlier),
              later: tPhase(c.connector.later),
              summary: tDag("connector", { count: c.visibleCount }),
              hidden: tDag("connectorHidden", { count: c.hiddenCount }),
            })}
            tooltip={connectorTooltip(c, labelById, {
              earlier: tPhase(c.connector.earlier),
              later: tPhase(c.connector.later),
            })}
          />
        ))}

        {/* Member + pending cards (foreground). */}
        {geom.boxes.flatMap((box) =>
          box.members.map((m) =>
            box.phase.kind === "implement" ? (
              <ClusterCard
                key={m.node.artifact.id}
                cluster={{ task: m.node.artifact, fold: m.fold }}
                x={m.x}
                y={m.y}
                height={m.height}
                dimmed={m.node.dead}
                executingIds={executingIds}
                pulsingId={pulsingId}
                attentionCountOf={nodeAttentionCount}
                kindLabel={tKind(m.node.artifact.kind)}
                reviewKindLabel={tKind("review")}
                statusLabelOf={(s) => tStatus(s)}
                verdictLabelOf={(v) => tVerdict(v)}
                executingLabel={tDetail("executingNow")}
                attentionLabelOf={(count) => tDag("attention", { count })}
                olderLabelOf={(count) => tDetail("reviewsOlder", { count })}
                facetOf={facetOf}
                agentRunByLabelOf={(agent) => tDag("agentRunBy", { agent })}
                attemptsLabelOf={(count) => tDag("attempts", { count })}
                onSelect={onSelect}
              />
            ) : (
              <NodeCard
                key={m.node.artifact.id}
                artifact={m.node.artifact}
                x={m.x}
                y={m.y}
                executing={executingIds.has(`artifact:${m.node.artifact.id}`)}
                dimmed={m.node.dead}
                attentionCount={nodeAttentionCount(m.node.artifact)}
                pulsing={pulsingId === m.node.artifact.id}
                kindLabel={tKind(m.node.artifact.kind)}
                statusLabel={tStatus(m.node.artifact.status)}
                executingLabel={tDetail("executingNow")}
                attentionLabelOf={(count) => tDag("attention", { count })}
                facet={facetOf(m.node.artifact.id)}
                agentRunByLabelOf={(agent) => tDag("agentRunBy", { agent })}
                attemptsLabelOf={(count) => tDag("attempts", { count })}
                onSelect={onSelect}
              />
            )
          )
        )}
        {geom.boxes.flatMap((box) =>
          box.pending.map((p) => (
            <PendingCard
              key={`pending:${p.pending.iterationId}`}
              pending={p.pending}
              x={p.x}
              y={p.y}
              kindLabel={tKind(p.pending.kind)}
              statusLabel={
                p.pending.status === "running"
                  ? tDag("running")
                  : tDag("queued")
              }
              agentFacetAvailable={graph.agentFacetAvailable}
              agentRunByLabelOf={(agent) => tDag("agentRunBy", { agent })}
              onOpen={onOpenIteration}
            />
          ))
        )}

        {/* Live artifact-less session chips (Issue triage / Result finalize). */}
        {geom.boxes.flatMap((box) =>
          box.sessions.map((s) => (
            <SessionChip
              key={`session:${s.ref.iterationId}`}
              session={s.ref}
              x={s.x}
              y={s.y}
              kindLabel={tStage(s.ref.stage)}
              statusLabel={
                s.ref.status === "running" ? tDag("running") : tDag("queued")
              }
              agentRunByLabelOf={(agent) => tDag("agentRunBy", { agent })}
              onOpenSession={onOpenSession}
            />
          ))
        )}
      </div>
    </div>
  )
}

/** Accessible label for a connector: phase pair + visible link count, plus the
 *  hidden count when the toggle is hiding dead-endpoint links. */
function connectorAriaLabel(
  c: ConnectorLayout,
  showSuperseded: boolean,
  t: { earlier: string; later: string; summary: string; hidden: string }
): string {
  const head = `${t.earlier} → ${t.later}, ${t.summary}`
  return !showSuperseded && c.hiddenCount > 0 ? `${head}, ${t.hidden}` : head
}

/** Plain-text tooltip tracing every folded link in its canonical direction. */
function connectorTooltip(
  c: ConnectorLayout,
  labelById: Map<number, LoopArtifactRow>,
  t: { earlier: string; later: string }
): string {
  const lines = [`${t.earlier} → ${t.later}`]
  for (const link of c.connector.sourceLinks) {
    const from = labelById.get(link.fromArtifactId)
    const to = labelById.get(link.toArtifactId)
    const fromLabel = from ? from.title : `#${link.fromArtifactId}`
    const toLabel = to ? to.title : `#${link.toArtifactId}`
    lines.push(`${fromLabel} → ${toLabel}`)
  }
  return lines.join("\n")
}

/** A phase's bordered container: a title bar (state dot + name) over the region
 *  its member/pending cards overlay. Empty phases render a slim placeholder. */
function PhaseContainer({
  box,
  name,
  stateLabel,
}: {
  box: PhaseBoxLayout
  name: string
  stateLabel: string
}) {
  return (
    <div
      data-phase={box.phase.kind}
      data-phase-state={box.phase.state}
      data-placeholder={box.solid ? undefined : "true"}
      style={{ left: box.x, top: box.y, width: box.width, height: box.height }}
      className={cn(
        "absolute rounded-xl border",
        box.solid ? "bg-muted/20" : "border-dashed bg-muted/5"
      )}
    >
      <div
        className="flex items-center gap-1.5 px-3"
        style={{ height: PHASE_HEADER_H }}
      >
        <span
          title={stateLabel}
          className={cn(
            "h-2 w-2 shrink-0 rounded-full",
            PHASE_STATE_DOT[box.phase.state]
          )}
        />
        <span
          className={cn(
            "truncate text-[0.625rem] font-medium uppercase tracking-wide",
            box.solid ? "text-muted-foreground" : "text-muted-foreground/60"
          )}
        >
          {name}
        </span>
      </div>
    </div>
  )
}

/** The focusable, labeled hit target for a folded connector — its midpoint
 *  badge. The line itself is decorative; this carries count + a11y + tooltip. */
function ConnectorBadge({
  layout,
  label,
  tooltip,
}: {
  layout: ConnectorLayout
  label: string
  tooltip: string
}) {
  const c = layout.connector
  return (
    <span
      role="img"
      tabIndex={0}
      aria-label={label}
      title={tooltip}
      data-connector={`${c.earlier}->${c.later}:${c.connectorKind}`}
      data-skip={c.connectorKind === "skip" ? "true" : undefined}
      data-total={c.totalCount}
      data-active={c.activeCount}
      data-hidden={layout.hiddenCount}
      style={{
        left: layout.badgeX,
        top: layout.badgeY,
        transform: "translate(-50%, -50%)",
      }}
      className={cn(
        "absolute inline-flex min-w-[1.25rem] items-center justify-center rounded-full border bg-background px-1.5 text-[0.625rem] font-medium tabular-nums text-muted-foreground shadow-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
        layout.dashed && "border-dashed"
      )}
    >
      {layout.visibleCount}
    </span>
  )
}

/**
 * Ghost card for an in-flight stage whose output artifact doesn't exist yet
 * (spec D2). Dashed + pulsing; clickable to its live iteration session when one
 * is attached. Derives its label from the pending kind + status only (no title).
 */
function PendingCard({
  pending,
  x,
  y,
  kindLabel,
  statusLabel,
  agentFacetAvailable,
  agentRunByLabelOf,
  onOpen,
}: {
  pending: PhasePending
  x: number
  y: number
  kindLabel: string
  statusLabel: string
  agentFacetAvailable: boolean
  agentRunByLabelOf: (agent: string) => string
  onOpen?: (pending: PhasePending) => void
}) {
  const clickable = pending.conversationId != null && onOpen != null
  // Gate on agentFacetAvailable too (not just agentType != null) so a partial
  // deployment with the facet off never shows a ghost icon (Codex minor).
  const agentType = agentFacetAvailable ? pending.agentType : null
  const runBy = agentType ? agentRunByLabelOf(agentName(agentType)) : null
  return (
    <button
      type="button"
      disabled={!clickable}
      onClick={() => onOpen?.(pending)}
      style={{ left: x, top: y, width: NODE_W, height: HEADER_H }}
      aria-label={
        runBy
          ? `${kindLabel}: ${statusLabel} — ${runBy}`
          : `${kindLabel}: ${statusLabel}`
      }
      className={cn(
        "absolute flex flex-col justify-center gap-1 rounded-lg border border-dashed bg-card/60 px-3 py-2 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
        clickable ? "hover:bg-accent" : "cursor-default"
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className="h-2 w-2 shrink-0 animate-pulse rounded-full bg-sky-500" />
        <span className="text-[0.625rem] uppercase tracking-wide text-muted-foreground">
          {kindLabel}
        </span>
        {agentType && (
          <span
            className="ml-auto inline-flex shrink-0"
            title={runBy ?? undefined}
          >
            <AgentIcon agentType={agentType} className="h-3 w-3" />
          </span>
        )}
      </div>
      <span className="truncate text-sm font-medium text-muted-foreground">
        {statusLabel}
      </span>
    </button>
  )
}

/**
 * A live, artifact-less session chip (Issue triage / Result finalize). Unlike a
 * landed node there's no drawer to open, so the chip itself is the session entry
 * point — clickable to its conversation when it has one.
 */
function SessionChip({
  session,
  x,
  y,
  kindLabel,
  statusLabel,
  agentRunByLabelOf,
  onOpenSession,
}: {
  session: IterationRef
  x: number
  y: number
  kindLabel: string
  statusLabel: string
  agentRunByLabelOf: (agent: string) => string
  onOpenSession?: (session: {
    conversationId: number
    agentType?: AgentType | null
    outcome?: LoopIterationOutcome | null
    stage?: LoopStage
  }) => void
}) {
  const clickable = session.conversationId != null && onOpenSession != null
  const runBy = session.agentType
    ? agentRunByLabelOf(agentName(session.agentType))
    : null
  return (
    <button
      type="button"
      disabled={!clickable}
      onClick={() => {
        if (session.conversationId != null)
          onOpenSession?.({
            conversationId: session.conversationId,
            agentType: session.agentType,
            outcome: session.outcome,
            stage: session.stage,
          })
      }}
      style={{ left: x, top: y, width: NODE_W, height: SESSION_CHIP_H }}
      aria-label={
        runBy
          ? `${kindLabel}: ${statusLabel} — ${runBy}`
          : `${kindLabel}: ${statusLabel}`
      }
      className={cn(
        "absolute flex items-center gap-1.5 rounded-md border border-dashed bg-card/60 px-2 text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
        clickable ? "hover:bg-accent" : "cursor-default"
      )}
    >
      <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-sky-500" />
      {session.agentType && (
        <span className="inline-flex shrink-0" title={runBy ?? undefined}>
          <AgentIcon agentType={session.agentType} className="h-3 w-3" />
        </span>
      )}
      <span className="truncate text-xs text-muted-foreground">
        {statusLabel}
      </span>
    </button>
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

/** The agent facet shown on a node: an `×N` attempt badge (N > 1) and the
 *  producing agent's icon (decorative — the node's own click opens the drawer,
 *  which surfaces the producing session). Renders nothing when the facet is off
 *  or the node has no agent / a single attempt. */
function AgentBadge({
  agentType,
  attemptCount,
  runByLabelOf,
  attemptsLabelOf,
}: {
  agentType: AgentType | null
  attemptCount: number | null
  runByLabelOf: (agent: string) => string
  attemptsLabelOf: (count: number) => string
}) {
  const showAttempts = attemptCount != null && attemptCount > 1
  if (!agentType && !showAttempts) return null
  return (
    <>
      {showAttempts && (
        <span
          title={attemptsLabelOf(attemptCount)}
          className="rounded bg-muted px-1 text-[0.5625rem] font-medium tabular-nums text-muted-foreground"
        >
          ×{attemptCount}
        </span>
      )}
      {agentType && (
        <span
          title={runByLabelOf(agentName(agentType))}
          className="inline-flex shrink-0"
        >
          <AgentIcon agentType={agentType} className="h-3 w-3" />
        </span>
      )}
    </>
  )
}

/** A read-stage (issue/requirement/design), result, or reflection node. */
function NodeCard({
  artifact,
  x,
  y,
  executing,
  dimmed,
  attentionCount,
  pulsing,
  kindLabel,
  statusLabel,
  executingLabel,
  attentionLabelOf,
  facet,
  agentRunByLabelOf,
  attemptsLabelOf,
  onSelect,
}: {
  artifact: LoopArtifactRow
  x: number
  y: number
  executing: boolean
  dimmed: boolean
  attentionCount: number
  pulsing: boolean
  kindLabel: string
  statusLabel: string
  executingLabel: string
  attentionLabelOf: (count: number) => string
  facet: { agentType: AgentType | null; attemptCount: number | null }
  agentRunByLabelOf: (agent: string) => string
  attemptsLabelOf: (count: number) => string
  onSelect: (artifactId: number) => void
}) {
  const attention = attentionCount > 0
  const attentionLabel = attention ? attentionLabelOf(attentionCount) : null
  const runBy = facet.agentType
    ? agentRunByLabelOf(agentName(facet.agentType))
    : null
  const aria = [`${kindLabel}: ${artifact.title}`]
  if (runBy) aria.push(runBy)
  if (attentionLabel) aria.push(attentionLabel)
  return (
    <button
      type="button"
      data-artifact-id={artifact.id}
      onClick={() => onSelect(artifact.id)}
      style={{ left: x, top: y, width: NODE_W, height: HEADER_H }}
      aria-label={aria.join(" — ")}
      className={cn(
        "absolute flex flex-col justify-center gap-1 rounded-lg border bg-card px-3 py-2 text-left shadow-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
        nodeRingClass({ pulsing, attention, executing }),
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
        <span className="ml-auto flex items-center gap-1">
          <AgentBadge
            agentType={facet.agentType}
            attemptCount={facet.attemptCount}
            runByLabelOf={agentRunByLabelOf}
            attemptsLabelOf={attemptsLabelOf}
          />
          {attention && (
            <span title={attentionLabel ?? undefined}>
              <AttentionMark />
            </span>
          )}
        </span>
      </div>
      <span className="truncate text-sm font-medium">{artifact.title}</span>
    </button>
  )
}

/** A task and its reviews, rendered as one bordered cluster. Each review is a
 *  first-class node: it carries its own `data-artifact-id`, locate pulse, and
 *  attention ring so it participates in locate / attention / drawer like a task. */
function ClusterCard({
  cluster,
  x,
  y,
  height,
  dimmed,
  executingIds,
  pulsingId,
  attentionCountOf,
  kindLabel,
  reviewKindLabel,
  statusLabelOf,
  verdictLabelOf,
  executingLabel,
  attentionLabelOf,
  olderLabelOf,
  facetOf,
  agentRunByLabelOf,
  attemptsLabelOf,
  onSelect,
}: {
  cluster: TaskClusterView
  x: number
  y: number
  height: number
  dimmed: boolean
  executingIds: Set<string>
  pulsingId: number | null
  attentionCountOf: (artifact: LoopArtifactRow) => number
  kindLabel: string
  reviewKindLabel: string
  statusLabelOf: (s: LoopArtifactStatus) => string
  verdictLabelOf: (v: LoopReviewVerdict) => string
  executingLabel: string
  attentionLabelOf: (count: number) => string
  olderLabelOf: (count: number) => string
  facetOf: (id: number) => {
    agentType: AgentType | null
    attemptCount: number | null
  }
  agentRunByLabelOf: (agent: string) => string
  attemptsLabelOf: (count: number) => string
  onSelect: (artifactId: number) => void
}) {
  const { task, fold } = cluster
  const taskExecuting = executingIds.has(`artifact:${task.id}`)
  const hasReviews = fold.latest.length > 0 || fold.olderCount > 0
  const taskAttentionCount = attentionCountOf(task)
  const taskAttention = taskAttentionCount > 0
  const taskAttentionLabel = taskAttention
    ? attentionLabelOf(taskAttentionCount)
    : null
  const taskFacet = facetOf(task.id)
  const taskRunBy = taskFacet.agentType
    ? agentRunByLabelOf(agentName(taskFacet.agentType))
    : null
  const taskAria = [`${kindLabel}: ${task.title}`]
  if (taskRunBy) taskAria.push(taskRunBy)
  if (taskAttentionLabel) taskAria.push(taskAttentionLabel)
  return (
    <div
      data-artifact-id={task.id}
      style={{ left: x, top: y, width: NODE_W, height }}
      className={cn(
        "absolute flex flex-col overflow-hidden rounded-lg border bg-card shadow-sm",
        nodeRingClass({
          pulsing: pulsingId === task.id,
          attention: taskAttention,
          executing: false,
        }),
        dimmed && "opacity-50"
      )}
    >
      <button
        type="button"
        onClick={() => onSelect(task.id)}
        style={{ height: HEADER_H }}
        aria-label={taskAria.join(" — ")}
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
          <span className="ml-auto flex items-center gap-1">
            <AgentBadge
              agentType={taskFacet.agentType}
              attemptCount={taskFacet.attemptCount}
              runByLabelOf={agentRunByLabelOf}
              attemptsLabelOf={attemptsLabelOf}
            />
            {taskAttention && (
              <span title={taskAttentionLabel ?? undefined}>
                <AttentionMark />
              </span>
            )}
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
            const executing = executingIds.has(`artifact:${review.id}`)
            const reviewAttentionCount = attentionCountOf(review)
            const reviewAttention = reviewAttentionCount > 0
            // Row text keeps the artifact title so sibling reviews stay distinct;
            // the pass/fail outcome shows as a shape glyph (✓/✗) — not color alone
            // — and is named in the accessible label + tooltip.
            const verdictLabel = review.verdict
              ? verdictLabelOf(review.verdict)
              : null
            const attentionLabel = reviewAttention
              ? attentionLabelOf(reviewAttentionCount)
              : null
            const statusLabel = executing
              ? executingLabel
              : statusLabelOf(review.status)
            const baseLabel = verdictLabel
              ? `${reviewKindLabel}: ${review.title} — ${verdictLabel}`
              : `${reviewKindLabel}: ${review.title}`
            const reviewFacet = facetOf(review.id)
            const reviewAgentType = reviewFacet.agentType
            const reviewRunBy = reviewAgentType
              ? agentRunByLabelOf(agentName(reviewAgentType))
              : null
            const reviewAria = [baseLabel]
            if (reviewRunBy) reviewAria.push(reviewRunBy)
            if (attentionLabel) reviewAria.push(attentionLabel)
            return (
              <button
                key={review.id}
                type="button"
                data-artifact-id={review.id}
                onClick={() => onSelect(review.id)}
                style={{ height: REVIEW_H }}
                aria-label={reviewAria.join(" — ")}
                title={verdictLabel ?? statusLabel}
                className={cn(
                  "flex items-center gap-1.5 px-3 text-left outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                  nodeRingClass(
                    {
                      pulsing: pulsingId === review.id,
                      attention: reviewAttention,
                      executing: false,
                    },
                    true
                  ),
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
                {reviewAttention && (
                  <span title={attentionLabel ?? undefined}>
                    <AttentionMark />
                  </span>
                )}
                <AgentBadge
                  agentType={reviewFacet.agentType}
                  attemptCount={reviewFacet.attemptCount}
                  runByLabelOf={agentRunByLabelOf}
                  attemptsLabelOf={attemptsLabelOf}
                />
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
 * other, so an edge never cuts through a node body. Used for Implement-internal
 * depends_on edges (which run between equal-size task cards).
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
