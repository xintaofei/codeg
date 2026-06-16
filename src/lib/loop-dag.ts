import type {
  LoopArtifactKind,
  LoopArtifactRow,
  LoopLinkKind,
  LoopLinkRow,
} from "@/lib/types"

/**
 * Read-stage columns, left → right. Tasks and reviews are NOT columns: each task
 * becomes a {@link DagCluster} that folds in its own reviews, and `result` closes
 * the pipeline from the task clusters on the right. A skipped read stage (e.g.
 * `design` on the `skip_design` route) simply contributes no node.
 */
export const STAGE_COLUMNS: LoopArtifactKind[] = [
  "issue",
  "requirement",
  "design",
]

/** First pipeline column a task cluster can occupy (after the read stages). */
const TASK_COL_BASE = STAGE_COLUMNS.length // 3

export interface DagNode {
  artifact: LoopArtifactRow
  /** Pipeline column. */
  col: number
  /** Row within the column (0-based, top → bottom). */
  row: number
}

/**
 * A task and the reviews that belong to it, laid out as one unit. Reviews fold
 * into their task's cluster (via `reviews` edges) rather than sprawling as a
 * separate column; parallel task chains stack as {@link DagCluster.lane}s and a
 * `depends_on` chain runs rightward via {@link DagCluster.col}.
 */
export interface DagCluster {
  task: LoopArtifactRow
  /** Reviews of the task, oldest → newest by (attempt, sort, id). */
  reviews: LoopArtifactRow[]
  /** Pipeline column: `TASK_COL_BASE + dependency depth` (chains run rightward). */
  col: number
  /** Vertical lane among parallel task chains (0-based, top → bottom). */
  lane: number
}

export interface DagEdge {
  id: number
  /** Dependent artifact (child / result / source) — the edge's tail. */
  from: number
  /** Referenced artifact (parent / task / subject) — the edge's head. */
  to: number
  kind: LoopLinkKind
  /** `skips_to` provenance renders dashed; everything else solid. */
  dashed: boolean
}

export interface DagLayout {
  /** Read-stage nodes (issue / requirement / design), packed by (sort, id). */
  stageNodes: DagNode[]
  /** Task clusters, positioned by (col, lane), sorted by (lane, col). */
  clusters: DagCluster[]
  /** The single result node + its trailing column, or null before finalize. */
  result: { artifact: LoopArtifactRow; col: number } | null
  /** Edges with both endpoints present, excluding any that touch a folded review. */
  edges: DagEdge[]
  /** Highest occupied column index + 1 (width driver); 0 when empty. */
  colCount: number
  /** Number of task lanes (cluster-band height driver); 0 when no tasks. */
  laneCount: number
  /** Tallest read-stage column's node count (stage-band height driver). */
  stageRowCount: number
  /**
   * Count of `superseded`/`cancelled` artifacts (any kind) hidden from the
   * layout. Drives the "show N superseded" toggle. Counted over the FULL input
   * regardless of `includeSuperseded`, so the toggle stays available to hide
   * them again. 0 when there are none.
   */
  supersededCount: number
}

const isDeadStatus = (s: LoopArtifactRow["status"]): boolean =>
  s === "superseded" || s === "cancelled"

const isDashed = (kind: LoopLinkKind): boolean => kind === "skips_to"

const bySortId = (a: LoopArtifactRow, b: LoopArtifactRow) =>
  a.sort - b.sort || a.id - b.id

/**
 * Build a cluster/lane layout from a DagView's artifacts + links. Pure and
 * stable: read stages keep fixed columns; each task becomes a cluster folding in
 * its reviews; the `depends_on` forest places chains rightward (col) and parallel
 * chains in separate lanes; `result` closes at the trailing column. No I/O —
 * unit-tested directly.
 */
export function buildDag(
  artifacts: LoopArtifactRow[],
  links: LoopLinkRow[],
  opts?: { includeSuperseded?: boolean }
): DagLayout {
  // By default, dead nodes (superseded / cancelled — e.g. tasks a replan
  // discarded, or a rejected design) are dropped from the layout so the graph
  // shows the LIVE plan; their edges fall away with them (the design no longer
  // links to old tasks). The audit copies live on in the artifact list/drawer,
  // and the toggle re-includes them (dimmed) on demand.
  const includeSuperseded = opts?.includeSuperseded ?? false
  const supersededCount = artifacts.filter((a) => isDeadStatus(a.status)).length
  const visible = includeSuperseded
    ? artifacts
    : artifacts.filter((a) => !isDeadStatus(a.status))

  const byId = new Map(visible.map((a) => [a.id, a]))
  const tasks = visible.filter((a) => a.kind === "task")
  const reviewIds = new Set(
    visible.filter((a) => a.kind === "review").map((r) => r.id)
  )

  // --- Fold reviews into the task each reviews (review --reviews--> task). ---
  const reviewsByTask = new Map<number, LoopArtifactRow[]>()
  for (const l of links) {
    if (l.kind !== "reviews") continue
    const review = byId.get(l.from_artifact_id)
    const task = byId.get(l.to_artifact_id)
    if (!review || review.kind !== "review") continue
    if (!task || task.kind !== "task") continue
    const bucket = reviewsByTask.get(task.id)
    if (bucket) bucket.push(review)
    else reviewsByTask.set(task.id, [review])
  }
  for (const bucket of reviewsByTask.values()) {
    bucket.sort(
      (a, b) => a.attempt - b.attempt || a.sort - b.sort || a.id - b.id
    )
  }

  // --- Dependency forest over tasks (depends_on: from = child, to = parent). ---
  const parentOf = new Map<number, number>()
  for (const l of links) {
    if (l.kind !== "depends_on") continue
    const child = byId.get(l.from_artifact_id)
    const parent = byId.get(l.to_artifact_id)
    if (!child || child.kind !== "task") continue
    if (!parent || parent.kind !== "task") continue
    // v1 is a forest (≤1 predecessor); keep the first parent defensively.
    if (!parentOf.has(child.id)) parentOf.set(child.id, parent.id)
  }

  // depth = dependency-chain length to a root (0 = no predecessor). `seen` guards
  // against a stray cycle (the backend enforces acyclicity).
  const depthOf = new Map<number, number>()
  const depth = (id: number, seen: Set<number>): number => {
    const cached = depthOf.get(id)
    if (cached !== undefined) return cached
    const parent = parentOf.get(id)
    let d = 0
    if (parent !== undefined && !seen.has(id)) {
      seen.add(id)
      d = depth(parent, seen) + 1
    }
    depthOf.set(id, d)
    return d
  }
  for (const t of tasks) depth(t.id, new Set())

  // children index, each sorted by (sort, id) for stable lane assignment.
  const childrenOf = new Map<number, LoopArtifactRow[]>()
  for (const t of tasks) {
    const p = parentOf.get(t.id)
    if (p === undefined) continue
    const bucket = childrenOf.get(p)
    if (bucket) bucket.push(t)
    else childrenOf.set(p, [t])
  }
  for (const bucket of childrenOf.values()) bucket.sort(bySortId)

  // Tidy lane assignment: a parent shares its first child's lane (chains run
  // horizontally); extra children + independent roots take fresh lanes below.
  const laneOf = new Map<number, number>()
  let nextLane = 0
  const assignLane = (id: number, seen: Set<number>): number => {
    const existing = laneOf.get(id)
    if (existing !== undefined) return existing
    seen.add(id)
    const kids = (childrenOf.get(id) ?? []).filter((c) => !seen.has(c.id))
    const lane =
      kids.length === 0
        ? nextLane++
        : kids.map((c) => assignLane(c.id, seen))[0]
    laneOf.set(id, lane)
    return lane
  }
  for (const r of tasks.filter((t) => !parentOf.has(t.id)).sort(bySortId)) {
    assignLane(r.id, new Set())
  }
  // Defensive: any task unreached by the forest walk still gets its own lane.
  for (const t of tasks) if (!laneOf.has(t.id)) laneOf.set(t.id, nextLane++)

  const clusters: DagCluster[] = tasks
    .map((task) => ({
      task,
      reviews: reviewsByTask.get(task.id) ?? [],
      col: TASK_COL_BASE + (depthOf.get(task.id) ?? 0),
      lane: laneOf.get(task.id) ?? 0,
    }))
    .sort((a, b) => a.lane - b.lane || a.col - b.col)

  // --- Read-stage nodes: issue / requirement / design, packed by (sort, id). ---
  const stageNodes: DagNode[] = []
  let stageRowCount = 0
  STAGE_COLUMNS.forEach((kind, col) => {
    const bucket = visible.filter((a) => a.kind === kind).sort(bySortId)
    bucket.forEach((artifact, row) => stageNodes.push({ artifact, col, row }))
    if (bucket.length > stageRowCount) stageRowCount = bucket.length
  })

  // --- Result closes the pipeline at the trailing column. ---
  const maxTaskCol = clusters.reduce(
    (m, c) => Math.max(m, c.col),
    TASK_COL_BASE - 1
  )
  const resultArtifact = visible.find((a) => a.kind === "result") ?? null
  const result = resultArtifact
    ? { artifact: resultArtifact, col: maxTaskCol + 1 }
    : null

  // --- Edges: drop dangling + any touching a folded review (containment says it). ---
  const present = new Set(visible.map((a) => a.id))
  const edges: DagEdge[] = links
    .filter(
      (l) =>
        present.has(l.from_artifact_id) &&
        present.has(l.to_artifact_id) &&
        !reviewIds.has(l.from_artifact_id) &&
        !reviewIds.has(l.to_artifact_id)
    )
    .map((l) => ({
      id: l.id,
      from: l.from_artifact_id,
      to: l.to_artifact_id,
      kind: l.kind,
      dashed: isDashed(l.kind),
    }))

  let maxCol = -1
  for (const n of stageNodes) maxCol = Math.max(maxCol, n.col)
  for (const c of clusters) maxCol = Math.max(maxCol, c.col)
  if (result) maxCol = Math.max(maxCol, result.col)

  return {
    stageNodes,
    clusters,
    result,
    edges,
    colCount: maxCol + 1,
    laneCount: clusters.length
      ? Math.max(...clusters.map((c) => c.lane)) + 1
      : 0,
    stageRowCount,
    supersededCount,
  }
}

/**
 * Split a cluster's reviews into the latest attempt (shown expanded) and the
 * count of older-attempt reviews (folded into a "+N earlier" chip). Reviews are
 * assumed pre-sorted oldest → newest by {@link buildDag}.
 */
export function foldReviews(reviews: LoopArtifactRow[]): {
  latest: LoopArtifactRow[]
  olderCount: number
} {
  if (reviews.length === 0) return { latest: [], olderCount: 0 }
  const latestAttempt = reviews.reduce((m, r) => Math.max(m, r.attempt), 0)
  const latest = reviews.filter((r) => r.attempt === latestAttempt)
  return { latest, olderCount: reviews.length - latest.length }
}
