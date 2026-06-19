import { bySortId, layoutTaskForest } from "@/lib/loop-dag"
import {
  artifactPhase,
  iterationPhase,
  type LoopPhase,
  PHASE_ORDER,
  phaseRank,
} from "@/lib/loop-phase"
import type {
  AgentType,
  ArtifactIterationRef,
  LoopArtifactKind,
  LoopArtifactRow,
  LoopArtifactStatus,
  LoopIssueRoute,
  LoopIterationOutcome,
  LoopIterationRow,
  LoopIterationStatus,
  LoopLinkKind,
  LoopLinkRow,
  LoopStage,
} from "@/lib/types"

// `ArtifactIterationRef` is the backend DTO mirror; its home is `types.ts`.
// Re-exported here so consumers of the model's input contract keep importing it
// from this module.
export type { ArtifactIterationRef }

/**
 * The two-level process model that replaces the artifact-level "edge soup" DAG.
 *
 * Macro level: a fixed six-phase pipeline (issue → requirement → design →
 * implement → result → reflect). Micro level: only Implement has rich internal
 * structure (the `depends_on` task workflow + folded reviews). The 1→N / N→1
 * lineage edges (`derives_from` / `results_from` / `skips_to`) that today sprawl
 * across the canvas are folded into **one connector per phase pair**, while every
 * underlying link's full metadata is preserved on the connector for tooltips and
 * traceability (spec §3.2).
 *
 * `buildProcessGraph` is pure, deterministic, and **toggle-independent**: it is
 * built once, always includes dead (superseded/cancelled) nodes flagged `dead`,
 * and precomputes both `activeCount`/`totalCount` connector counts. Hiding dead
 * nodes is a pure render-layer choice that never rebuilds the model (spec §4.6).
 *
 * The agent facet (producedBy / attemptCount / sessionRefs) is populated only when
 * `artifactIterationRefs` is supplied (P3); without it the model degrades cleanly
 * to the structural-only graph (all facet fields null/empty).
 */
export interface ProcessGraph {
  /** Always 6, in {@link PHASE_ORDER}. Includes empty phases (rail full-spectrum). */
  phases: Phase[]
  /** Folded lineage, one per `(earlier, later, connectorKind)`. */
  connectors: PhaseConnector[]
  /** Count of dead (superseded/cancelled) artifacts; render decides visibility. */
  supersededCount: number
  /** Same-phase lineage — a v1 model violation kept for assertion/warning rather
   *  than silently dropped (spec #M1). Expected empty. */
  unexpectedSamePhaseLineage: ConnectorLink[]
  /** Diagnostic counts for kinds/stages this frontend doesn't recognize (version
   *  mismatch). Such nodes never enter the six phases nor invent a seventh. */
  unmappedArtifacts: number
  unmappedIterations: number
  /** Capability probe (spec §3.3): whether the agent facet (icons, producedBy,
   *  sessionRefs) can be populated — true iff `artifactIterationRefs` was given. */
  agentFacetAvailable: boolean
}

export type PhaseState =
  | "blocked"
  | "awaiting_approval"
  | "active"
  | "pending"
  | "done"
  | "empty"

export interface Phase {
  kind: LoopPhase
  /** §4.3 table: driven ONLY by non-dead member status + pending count (so it is
   *  invariant under the superseded toggle); gate/coverage/criterion excluded. */
  state: PhaseState
  /** Only set when `state === "empty"`. */
  emptyReason: "no_members" | "all_hidden" | "session_only" | null
  /** Includes dead members (flagged); render shows/hides per toggle. Reviews are
   *  NOT members — each folds into the task it reviews. */
  members: ArtifactNode[]
  /** Only Implement has these (`depends_on` between tasks). */
  workflow: WorkflowEdge[]
  /** In-flight iterations of this phase whose output artifact hasn't landed. */
  pending: PhasePending[]
  /** Live (queued|running) artifact-less sessions (in-flight triage on Issue,
   *  finalize/fan-in on Result). Empty unless the agent facet is available. */
  sessionRefs: IterationRef[]
}

export interface ArtifactNode {
  artifact: LoopArtifactRow
  /** status ∈ {superseded, cancelled}; model always includes these, flagged. */
  dead: boolean
  /** Pipeline column / vertical lane — meaningful only inside Implement. A folded
   *  review inherits its task's col/lane. */
  col: number
  lane: number
  /** Folded reviews of a task (each a full node, with its own `dead`), oldest →
   *  newest by (attempt, sort, id). Empty for non-task kinds. */
  reviews: ArtifactNode[]
  // —— agent facet, populated only when agentFacetAvailable (spec §4.3) ——
  producedBy: ProducedBy | null
  /** null ⇒ facet unavailable or no producer; semantics in spec §4.3. */
  attemptCount: number | null
}

/**
 * Producing-iteration reference. Three states via field combos (spec §3.2):
 * ① producedBy === null ⇔ artifact.produced_by_iteration_id was null (human);
 * ② producedBy set, iterationId === null ⇔ produced_by set but its ref is absent
 *    (orphan / cross-issue — facet available but no ref for this node);
 * ③ producedBy set, iterationId !== null ⇔ resolved (agentType/conversationId may
 *    each be independently null). Icon ⇔ agentType != null; openable ⇔
 *    conversationId != null.
 */
export interface ProducedBy {
  iterationId: number | null
  stage: LoopStage | null
  agentType: AgentType | null
  conversationId: number | null
}

export interface PhaseConnector {
  /** Normalized by phase order; **always rendered earlier → later** (flow dir). */
  earlier: LoopPhase
  later: LoopPhase
  /** `skip` ⇒ dashed; distinct connector from `lineage` for the same phase pair. */
  connectorKind: "lineage" | "skip"
  /** Each underlying link's full metadata — folding loses no direction/endpoint/
   *  revision (spec #M2/#m6). */
  sourceLinks: ConnectorLink[]
  /** Both counts are toggle-independent and precomputed; render picks one. */
  totalCount: number
  /** Links whose BOTH endpoint artifacts are non-dead (default render). */
  activeCount: number
}

export interface ConnectorLink {
  linkId: number
  kind: LoopLinkKind
  /** Canonical direction from the link row (e.g. results_from: from=result, to=task). */
  fromArtifactId: number
  toArtifactId: number
  fromPhase: LoopPhase
  toPhase: LoopPhase
  /** From the link row; staleness data survives folding here (spec #m6). */
  sourceRevisionId: number | null
}

export interface WorkflowEdge {
  from: number
  to: number
  kind: "depends_on"
}

/**
 * A live, artifact-less session attached to a phase (Issue triage / Result
 * finalize). Populated only when the agent facet is available (otherwise the
 * phase's `sessionRefs` stay empty).
 */
export interface IterationRef {
  iterationId: number
  stage: LoopStage
  attempt: number
  status: LoopIterationStatus
  outcome: LoopIterationOutcome | null
  agentType: AgentType | null
  conversationId: number | null
}

/**
 * A ghost for an in-flight iteration whose output artifact doesn't exist yet — the
 * "this phase is running" marker. It carries no DAG col/row geometry (that's the
 * renderer's concern), only the ghost's identity and which artifact kind it will
 * produce.
 */
export interface PhasePending {
  iterationId: number
  conversationId: number | null
  /** Producing agent of the in-flight iteration (P3). The render gates the icon by
   *  `agentFacetAvailable && agentType != null`. `null` on older servers. */
  agentType: AgentType | null
  stage: LoopStage
  /** Artifact kind this stage will produce. */
  kind: LoopArtifactKind
  status: "queued" | "running"
  startedAt: string | null
}

export interface BuildProcessGraphInput {
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
  /** In-flight (queued|running) iterations — drives ghosts (same source as buildDag). */
  liveIterations?: LoopIterationRow[]
  /** Agent-facet input (spec §4.4). Present (an array, even `[]`) ⇒ facet on.
   *  **P1 callers omit it** ⇒ facet off. Capability is probed via Array.isArray
   *  (not `!== undefined`) so a `null` from an Option<Vec> also reads as off. */
  artifactIterationRefs?: readonly ArtifactIterationRef[] | null
  /** Reserved for skip-route presentation; unused in P1 (connectors derive from
   *  links, and `skips_to` already encodes the skip). */
  route?: LoopIssueRoute | null
}

const isDead = (s: LoopArtifactStatus): boolean =>
  s === "superseded" || s === "cancelled"

const LINEAGE_KINDS: ReadonlySet<LoopLinkKind> = new Set([
  "derives_from",
  "results_from",
  "skips_to",
])

/**
 * Stages that, while in flight, surface as a pending ghost in their phase (they
 * produce a concrete node in a structural phase). `triage`/`finalize` instead
 * surface as live `sessionRefs` (Issue/Result), and `implement`/`review` advance
 * an existing task node, so none of those yield a ghost (spec §3.1).
 */
const GHOST_KIND: Partial<Record<LoopStage, LoopArtifactKind>> = {
  refine: "requirement",
  design: "design",
  plan: "task",
  reflect: "reflection",
}

/**
 * Build the {@link ProcessGraph} from a DAG view's artifacts + links + live
 * iterations. Pure, deterministic, toggle-independent (always includes dead nodes
 * flagged; precomputes both connector counts). No I/O.
 */
export function buildProcessGraph(input: BuildProcessGraphInput): ProcessGraph {
  const { artifacts, links } = input
  const liveIterations = input.liveIterations ?? []
  // Probe per spec §3.3: an array (incl. empty) ⇒ facet available. P1 callers
  // don't pass refs, so this is false and the whole agent facet stays absent.
  const agentFacetAvailable = Array.isArray(input.artifactIterationRefs)

  // P3 agent facet: index the resolved producing refs by artifact (empty when the
  // facet is off, so resolution below is a no-op and P1 behavior is preserved).
  const refByArtifact = new Map<number, ArtifactIterationRef>()
  if (agentFacetAvailable) {
    for (const r of input.artifactIterationRefs!)
      refByArtifact.set(r.artifact_id, r)
  }
  // The 4 producedBy states (spec §3.2): facet off OR no producer ⇒ null/null;
  // producer set but its ref is absent (orphan / cross-issue) ⇒ unresolved
  // (iterationId null); ref present ⇒ fully resolved + per-kind attemptCount.
  const resolveProducedBy = (
    artifact: LoopArtifactRow
  ): Pick<ArtifactNode, "producedBy" | "attemptCount"> => {
    if (!agentFacetAvailable || artifact.produced_by_iteration_id == null)
      return { producedBy: null, attemptCount: null }
    const ref = refByArtifact.get(artifact.id)
    if (!ref)
      return {
        producedBy: {
          iterationId: null,
          stage: null,
          agentType: null,
          conversationId: null,
        },
        attemptCount: null,
      }
    return {
      producedBy: {
        iterationId: ref.iteration_id,
        stage: ref.stage,
        agentType: ref.agent_type,
        conversationId: ref.conversation_id,
      },
      attemptCount: ref.attempt_count,
    }
  }

  // Model is toggle-independent: index over ALL artifacts (dead included).
  const byId = new Map(artifacts.map((a) => [a.id, a]))
  const supersededCount = artifacts.filter((a) => isDead(a.status)).length

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

  // --- Implement placement: depends_on forest over ALL tasks (shared helper). ---
  const allTasks = artifacts.filter((a) => a.kind === "task")
  const { depthOf, laneOf } = layoutTaskForest(allTasks, links, byId)

  // --- Group artifact nodes by phase (reviews folded, not standalone members). ---
  const membersByPhase = new Map<LoopPhase, ArtifactNode[]>()
  for (const phase of PHASE_ORDER) membersByPhase.set(phase, [])
  let unmappedArtifacts = 0

  const toNode = (
    artifact: LoopArtifactRow,
    col: number,
    lane: number,
    reviews: ArtifactNode[]
  ): ArtifactNode => ({
    artifact,
    dead: isDead(artifact.status),
    col,
    lane,
    reviews,
    // Agent facet (P3): resolved for every node, including folded reviews. Facet
    // off ⇒ null/null, preserving P1 behavior.
    ...resolveProducedBy(artifact),
  })

  for (const a of artifacts) {
    // Reviews fold into their task; they're never standalone members.
    if (a.kind === "review") continue
    const phase = artifactPhase(a.kind)
    if (phase === null) {
      unmappedArtifacts += 1
      continue
    }
    if (a.kind === "task") {
      const col = depthOf.get(a.id) ?? 0
      const lane = laneOf.get(a.id) ?? 0
      const reviews = (reviewsByTask.get(a.id) ?? []).map((r) =>
        toNode(r, col, lane, [])
      )
      membersByPhase.get(phase)!.push(toNode(a, col, lane, reviews))
    } else {
      membersByPhase.get(phase)!.push(toNode(a, 0, 0, []))
    }
  }

  // Deterministic member order: Implement by (lane, col, sort, id); others by
  // (sort, id). Implement's geometry comes from col/lane, not array order.
  for (const [phase, members] of membersByPhase) {
    if (phase === "implement") {
      members.sort(
        (a, b) =>
          a.lane - b.lane || a.col - b.col || bySortId(a.artifact, b.artifact)
      )
    } else {
      members.sort((a, b) => bySortId(a.artifact, b.artifact))
    }
  }

  // --- Workflow edges: depends_on among tasks present (Implement-internal). ---
  const workflow: WorkflowEdge[] = []
  for (const l of links) {
    if (l.kind !== "depends_on") continue
    const from = byId.get(l.from_artifact_id)
    const to = byId.get(l.to_artifact_id)
    if (!from || from.kind !== "task") continue
    if (!to || to.kind !== "task") continue
    workflow.push({
      from: l.from_artifact_id,
      to: l.to_artifact_id,
      kind: "depends_on",
    })
  }

  // --- Lineage folding into connectors (one per (earlier, later, connectorKind)). ---
  const connectorMap = new Map<string, PhaseConnector>()
  const unexpectedSamePhaseLineage: ConnectorLink[] = []
  for (const l of links) {
    if (!LINEAGE_KINDS.has(l.kind)) continue // reviews + depends_on excluded
    const from = byId.get(l.from_artifact_id)
    const to = byId.get(l.to_artifact_id)
    if (!from || !to) continue // dangling endpoint (malformed view) — skip
    const fromPhase = artifactPhase(from.kind)
    const toPhase = artifactPhase(to.kind)
    // Unmapped endpoint kind: already counted via the artifact loop; can't place.
    if (fromPhase === null || toPhase === null) continue
    const connectorKind: PhaseConnector["connectorKind"] =
      l.kind === "skips_to" ? "skip" : "lineage"
    const link: ConnectorLink = {
      linkId: l.id,
      kind: l.kind,
      fromArtifactId: l.from_artifact_id,
      toArtifactId: l.to_artifact_id,
      fromPhase,
      toPhase,
      sourceRevisionId: l.source_revision_id,
    }
    if (fromPhase === toPhase) {
      // v1: all three lineage kinds must cross phases. Keep, warn, assert empty.
      unexpectedSamePhaseLineage.push(link)
      continue
    }
    const [earlier, later] =
      phaseRank(fromPhase) < phaseRank(toPhase)
        ? [fromPhase, toPhase]
        : [toPhase, fromPhase]
    const key = `${earlier}|${later}|${connectorKind}`
    let conn = connectorMap.get(key)
    if (!conn) {
      conn = {
        earlier,
        later,
        connectorKind,
        sourceLinks: [],
        totalCount: 0,
        activeCount: 0,
      }
      connectorMap.set(key, conn)
    }
    conn.sourceLinks.push(link)
    conn.totalCount += 1
    if (!isDead(from.status) && !isDead(to.status)) conn.activeCount += 1
  }
  const connectors = Array.from(connectorMap.values()).sort(
    (a, b) =>
      phaseRank(a.earlier) - phaseRank(b.earlier) ||
      phaseRank(a.later) - phaseRank(b.later) ||
      a.connectorKind.localeCompare(b.connectorKind)
  )

  // --- Pending ghosts per phase (from live iterations, output not yet landed). ---
  // Dedup by produced_by_iteration_id over ALL artifacts (a landed-but-hidden
  // artifact still suppresses its ghost). Mirrors buildDag's suppression.
  const landedIterationIds = new Set(
    artifacts
      .map((a) => a.produced_by_iteration_id)
      .filter((x): x is number => x != null)
  )
  const pendingByPhase = new Map<LoopPhase, PhasePending[]>()
  for (const phase of PHASE_ORDER) pendingByPhase.set(phase, [])
  let unmappedIterations = 0
  for (const it of liveIterations) {
    const phase = iterationPhase(it.stage)
    if (phase === null) {
      unmappedIterations += 1
      continue
    }
    if (it.status !== "queued" && it.status !== "running") continue
    const kind = GHOST_KIND[it.stage]
    if (!kind) continue // triage/finalize → sessionRefs; implement/review → no ghost
    if (landedIterationIds.has(it.id)) continue // already landed (stale snapshot)
    pendingByPhase.get(phase)!.push({
      iterationId: it.id,
      conversationId: it.conversation_id,
      agentType: it.agent_type ?? null,
      stage: it.stage,
      kind,
      status: it.status,
      startedAt: it.started_at,
    })
  }

  // --- Live, artifact-less sessions per phase (P3 facet): in-flight triage →
  // Issue, finalize → Result. Only target-less iterations (these are the
  // artifact-less phase history; a targeted run belongs to its node) and only live
  // (settled sessions are fetched lazily by the drawer). Off ⇒ all empty (P1). ---
  const sessionRefsByPhase = new Map<LoopPhase, IterationRef[]>()
  for (const phase of PHASE_ORDER) sessionRefsByPhase.set(phase, [])
  if (agentFacetAvailable) {
    for (const it of liveIterations) {
      if (it.status !== "queued" && it.status !== "running") continue
      if (it.target_artifact_id != null) continue
      const phase: LoopPhase | null =
        it.stage === "triage"
          ? "issue"
          : it.stage === "finalize"
            ? "result"
            : null
      if (phase === null) continue
      sessionRefsByPhase.get(phase)!.push({
        iterationId: it.id,
        stage: it.stage,
        attempt: it.attempt,
        status: it.status,
        outcome: it.outcome,
        agentType: it.agent_type ?? null,
        conversationId: it.conversation_id,
      })
    }
  }

  // --- Assemble the six phases. ---
  const phases: Phase[] = PHASE_ORDER.map((kind) => {
    const members = membersByPhase.get(kind)!
    const pending = pendingByPhase.get(kind)!
    const sessionRefs = sessionRefsByPhase.get(kind)!
    const state = computePhaseState(members, pending.length)
    const emptyReason =
      state === "empty"
        ? members.length > 0
          ? "all_hidden"
          : sessionRefs.length > 0
            ? "session_only"
            : "no_members"
        : null
    return {
      kind,
      state,
      emptyReason,
      members,
      workflow: kind === "implement" ? workflow : [],
      pending,
      sessionRefs,
    }
  })

  return {
    phases,
    connectors,
    supersededCount,
    unexpectedSamePhaseLineage,
    unmappedArtifacts,
    unmappedIterations,
    agentFacetAvailable,
  }
}

/**
 * §4.3 priority table. Inputs are ONLY the phase's non-dead member statuses + its
 * pending (ghost) count — dead members and gate/coverage/criterion are excluded,
 * so the result is invariant under the superseded toggle. First match wins.
 */
function computePhaseState(
  members: ArtifactNode[],
  pendingCount: number
): PhaseState {
  const live = members.filter((m) => !m.dead)
  if (live.some((m) => m.artifact.status === "blocked")) return "blocked"
  if (live.some((m) => m.artifact.status === "awaiting_approval"))
    return "awaiting_approval"
  if (live.some((m) => m.artifact.status === "in_progress") || pendingCount > 0)
    return "active"
  if (live.some((m) => m.artifact.status === "pending")) return "pending"
  // By elimination, any remaining live members are all `done`.
  if (live.length > 0) return "done"
  return "empty"
}
