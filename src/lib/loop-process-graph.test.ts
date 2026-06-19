import { describe, expect, it } from "vitest"

import { phaseRank } from "@/lib/loop-phase"
import {
  type ArtifactIterationRef,
  buildProcessGraph,
  type Phase,
  type ProcessGraph,
} from "@/lib/loop-process-graph"
import type {
  LoopArtifactKind,
  LoopArtifactRow,
  LoopIterationRow,
  LoopLinkKind,
  LoopLinkRow,
  LoopStage,
} from "@/lib/types"

// --- fixture builders ------------------------------------------------------

function art(
  id: number,
  kind: LoopArtifactKind,
  opts: Partial<LoopArtifactRow> = {}
): LoopArtifactRow {
  return {
    id,
    issue_id: 1,
    issue_seq: id,
    kind,
    title: `${kind} ${id}`,
    status: "done",
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    attempt: 0,
    contribution_kind: "delta",
    sort: id,
    updated_at: "2026-06-18T00:00:00Z",
    ...opts,
  }
}

function link(
  id: number,
  from: number,
  to: number,
  kind: LoopLinkKind,
  sourceRevisionId: number | null = null
): LoopLinkRow {
  return {
    id,
    from_artifact_id: from,
    to_artifact_id: to,
    kind,
    source_revision_id: sourceRevisionId,
  }
}

function iter(
  id: number,
  stage: LoopStage,
  opts: Partial<LoopIterationRow> = {}
): LoopIterationRow {
  return {
    id,
    issue_id: 1,
    issue_seq: id,
    stage,
    target_artifact_id: null,
    target_title: null,
    conversation_id: null,
    status: "running",
    launched_by: "engine",
    attempt: 0,
    tokens_used: 0,
    outcome: null,
    created_at: "2026-06-18T00:00:00Z",
    started_at: null,
    ended_at: null,
    ...opts,
  }
}

const phaseOf = (g: ProcessGraph, kind: Phase["kind"]): Phase =>
  g.phases.find((p) => p.kind === kind)!

// ---------------------------------------------------------------------------

describe("buildProcessGraph: shape & phases", () => {
  it("always returns the six phases in pipeline order", () => {
    const g = buildProcessGraph({ artifacts: [], links: [] })
    expect(g.phases.map((p) => p.kind)).toEqual([
      "issue",
      "requirement",
      "design",
      "implement",
      "result",
      "reflect",
    ])
  })

  it("an empty graph is all empty/no_members with no connectors", () => {
    const g = buildProcessGraph({ artifacts: [], links: [] })
    for (const p of g.phases) {
      expect(p.state).toBe("empty")
      expect(p.emptyReason).toBe("no_members")
      expect(p.members).toEqual([])
      expect(p.pending).toEqual([])
      expect(p.sessionRefs).toEqual([])
    }
    expect(g.connectors).toEqual([])
    expect(g.unexpectedSamePhaseLineage).toEqual([])
    expect(g.unmappedArtifacts).toBe(0)
    expect(g.unmappedIterations).toBe(0)
    expect(g.supersededCount).toBe(0)
  })
})

describe("buildProcessGraph: edge classification", () => {
  it("depends_on becomes a workflow edge inside Implement, never a connector", () => {
    const artifacts = [art(10, "task"), art(11, "task")]
    const links = [link(1, 11, 10, "depends_on")]
    const g = buildProcessGraph({ artifacts, links })
    const impl = phaseOf(g, "implement")
    expect(impl.workflow).toEqual([{ from: 11, to: 10, kind: "depends_on" }])
    expect(g.connectors).toEqual([])
    // workflow lives only on Implement.
    for (const p of g.phases) {
      if (p.kind !== "implement") expect(p.workflow).toEqual([])
    }
  })

  it("reviews fold into the task and never become a member or a connector", () => {
    const artifacts = [
      art(10, "task", { status: "in_progress" }),
      art(20, "review", { verdict: "pass" }),
    ]
    const links = [link(1, 20, 10, "reviews")]
    const g = buildProcessGraph({ artifacts, links })
    const impl = phaseOf(g, "implement")
    // one member (the task); the review is folded in, not standalone.
    expect(impl.members).toHaveLength(1)
    expect(impl.members[0].artifact.id).toBe(10)
    expect(impl.members[0].reviews.map((r) => r.artifact.id)).toEqual([20])
    expect(g.connectors).toEqual([])
    expect(g.unexpectedSamePhaseLineage).toEqual([])
  })

  it("the three lineage kinds fold into connectors", () => {
    const artifacts = [art(1, "issue"), art(2, "requirement")]
    const links = [link(1, 2, 1, "derives_from")]
    const g = buildProcessGraph({ artifacts, links })
    expect(g.connectors).toHaveLength(1)
    expect(g.connectors[0]).toMatchObject({
      earlier: "issue",
      later: "requirement",
      connectorKind: "lineage",
      totalCount: 1,
      activeCount: 1,
    })
  })
})

describe("buildProcessGraph: connector folding", () => {
  it("dedups many links into one connector per (earlier, later, kind)", () => {
    const issue = art(1, "issue")
    const reqs = [2, 3, 4, 5].map((id) => art(id, "requirement"))
    // each requirement derives_from the issue → 4 links, one connector.
    const links = reqs.map((r, i) => link(i + 1, r.id, 1, "derives_from"))
    const g = buildProcessGraph({ artifacts: [issue, ...reqs], links })
    expect(g.connectors).toHaveLength(1)
    const c = g.connectors[0]
    expect(c.earlier).toBe("issue")
    expect(c.later).toBe("requirement")
    expect(c.totalCount).toBe(4)
    expect(c.sourceLinks).toHaveLength(4)
  })

  it("preserves each link's canonical direction (results_from is result→task)", () => {
    const task = art(10, "task")
    const result = art(30, "result")
    // results_from canonical direction: from = result, to = task.
    const links = [link(1, 30, 10, "results_from", 7)]
    const g = buildProcessGraph({ artifacts: [task, result], links })
    expect(g.connectors).toHaveLength(1)
    const c = g.connectors[0]
    // connector normalized earlier→later by phase rank…
    expect(c.earlier).toBe("implement")
    expect(c.later).toBe("result")
    // …but the underlying link keeps result→task and its revision.
    expect(c.sourceLinks[0]).toEqual({
      linkId: 1,
      kind: "results_from",
      fromArtifactId: 30,
      toArtifactId: 10,
      fromPhase: "result",
      toPhase: "implement",
      sourceRevisionId: 7,
    })
  })

  it("keeps skip and lineage as separate connectors for the same phase pair", () => {
    // skip_design route: a task both derives_from and skips_to the requirement.
    const req = art(2, "requirement")
    const task = art(10, "task")
    const links = [link(1, 10, 2, "derives_from"), link(2, 10, 2, "skips_to")]
    const g = buildProcessGraph({ artifacts: [req, task], links })
    expect(g.connectors).toHaveLength(2)
    const kinds = g.connectors.map((c) => c.connectorKind).sort()
    expect(kinds).toEqual(["lineage", "skip"])
    for (const c of g.connectors) {
      expect(c.earlier).toBe("requirement")
      expect(c.later).toBe("implement")
    }
  })

  it("always orients connectors earlier→later regardless of link direction", () => {
    // derives_from points child→parent (requirement→issue), i.e. later→earlier.
    const g = buildProcessGraph({
      artifacts: [art(1, "issue"), art(2, "requirement")],
      links: [link(1, 2, 1, "derives_from")],
    })
    const c = g.connectors[0]
    expect(phaseRank(c.earlier)).toBeLessThan(phaseRank(c.later))
  })

  it("routes same-phase lineage to unexpectedSamePhaseLineage, not a connector", () => {
    // A (future) task→task derives_from: both endpoints are Implement.
    const a = art(10, "task")
    const b = art(11, "task")
    const links = [link(1, 11, 10, "derives_from")]
    const g = buildProcessGraph({ artifacts: [a, b], links })
    expect(g.connectors).toEqual([])
    expect(g.unexpectedSamePhaseLineage).toHaveLength(1)
    expect(g.unexpectedSamePhaseLineage[0]).toMatchObject({
      fromPhase: "implement",
      toPhase: "implement",
    })
    // and it is not mistaken for a workflow edge (only depends_on is).
    expect(phaseOf(g, "implement").workflow).toEqual([])
  })
})

describe("buildProcessGraph: toggle-independent double counts", () => {
  it("counts a link with a dead endpoint in total but not active", () => {
    const design = art(14, "design")
    const reqLive = art(2, "requirement")
    const reqDead = art(3, "requirement", { status: "superseded" })
    const links = [
      link(1, 14, 2, "derives_from"), // both live
      link(2, 14, 3, "derives_from"), // `to` is dead
    ]
    const g = buildProcessGraph({
      artifacts: [design, reqLive, reqDead],
      links,
    })
    const c = g.connectors.find(
      (x) => x.earlier === "requirement" && x.later === "design"
    )!
    expect(c.totalCount).toBe(2)
    expect(c.activeCount).toBe(1)
    expect(g.supersededCount).toBe(1)
  })

  it("always includes dead nodes in members, flagged dead", () => {
    const g = buildProcessGraph({
      artifacts: [
        art(2, "requirement", { status: "done" }),
        art(3, "requirement", { status: "cancelled" }),
      ],
      links: [],
    })
    const req = phaseOf(g, "requirement")
    expect(req.members).toHaveLength(2)
    expect(req.members.map((m) => m.dead)).toEqual([false, true])
  })
})

describe("buildProcessGraph: PhaseState table", () => {
  const reqGraph = (status: LoopArtifactRow["status"]) =>
    phaseOf(
      buildProcessGraph({
        artifacts: [art(2, "requirement", { status })],
        links: [],
      }),
      "requirement"
    )

  it("blocked wins over everything", () => {
    const g = buildProcessGraph({
      artifacts: [
        art(2, "requirement", { status: "blocked" }),
        art(3, "requirement", { status: "in_progress" }),
        art(4, "requirement", { status: "awaiting_approval" }),
      ],
      links: [],
    })
    expect(phaseOf(g, "requirement").state).toBe("blocked")
  })

  it("awaiting_approval over active/pending/done", () => {
    const g = buildProcessGraph({
      artifacts: [
        art(2, "requirement", { status: "awaiting_approval" }),
        art(3, "requirement", { status: "in_progress" }),
      ],
      links: [],
    })
    expect(phaseOf(g, "requirement").state).toBe("awaiting_approval")
  })

  it("in_progress → active", () => {
    expect(reqGraph("in_progress").state).toBe("active")
  })

  it("a pending ghost makes the phase active even with no members", () => {
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: [iter(99, "refine", { status: "queued" })],
    })
    const req = phaseOf(g, "requirement")
    expect(req.members).toEqual([])
    expect(req.pending).toHaveLength(1)
    expect(req.state).toBe("active")
    expect(req.emptyReason).toBeNull()
  })

  it("pending status → pending", () => {
    expect(reqGraph("pending").state).toBe("pending")
  })

  it("all done → done", () => {
    expect(reqGraph("done").state).toBe("done")
  })

  it("no members and no pending → empty/no_members", () => {
    const req = reqGraph("done") // requirement phase only; design is empty
    expect(req.state).toBe("done")
    const design = phaseOf(
      buildProcessGraph({ artifacts: [art(2, "requirement")], links: [] }),
      "design"
    )
    expect(design.state).toBe("empty")
    expect(design.emptyReason).toBe("no_members")
  })

  it("only-dead members → empty/all_hidden (dead never drives state)", () => {
    const g = buildProcessGraph({
      artifacts: [art(2, "requirement", { status: "superseded" })],
      links: [],
    })
    const req = phaseOf(g, "requirement")
    expect(req.members).toHaveLength(1)
    expect(req.state).toBe("empty")
    expect(req.emptyReason).toBe("all_hidden")
  })

  it("P1 never produces session_only (sessionRefs are deferred to P3)", () => {
    // Even an in-flight triage (which P3 will surface as an Issue sessionRef)
    // leaves sessionRefs empty in P1, so no phase is ever session_only.
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: [iter(99, "triage", { status: "running" })],
    })
    for (const p of g.phases) {
      expect(p.sessionRefs).toEqual([])
      expect(p.emptyReason).not.toBe("session_only")
    }
  })
})

describe("buildProcessGraph: pending ghosts", () => {
  it("places ghosts by iterationPhase; triage/finalize/implement/review make none", () => {
    const live = [
      iter(1, "refine", { status: "queued" }), // → requirement ghost
      iter(2, "design", { status: "running" }), // → design ghost
      iter(3, "plan", { status: "running" }), // → implement ghost
      iter(4, "reflect", { status: "queued" }), // → reflect ghost
      iter(5, "triage", { status: "running" }), // → no ghost (sessionRef in P3)
      iter(6, "finalize", { status: "running" }), // → no ghost (sessionRef in P3)
      iter(7, "implement", { status: "running" }), // → no ghost (advances a task)
      iter(8, "review", { status: "running" }), // → no ghost
    ]
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: live,
    })
    expect(phaseOf(g, "requirement").pending.map((p) => p.kind)).toEqual([
      "requirement",
    ])
    expect(phaseOf(g, "design").pending.map((p) => p.kind)).toEqual(["design"])
    expect(phaseOf(g, "implement").pending.map((p) => p.kind)).toEqual(["task"])
    expect(phaseOf(g, "reflect").pending.map((p) => p.kind)).toEqual([
      "reflection",
    ])
    expect(phaseOf(g, "issue").pending).toEqual([])
    expect(phaseOf(g, "result").pending).toEqual([])
  })

  it("suppresses a ghost once its iteration's artifact has landed", () => {
    const live = [iter(50, "design", { status: "running" })]
    const artifacts = [art(14, "design", { produced_by_iteration_id: 50 })]
    const g = buildProcessGraph({ artifacts, links: [], liveIterations: live })
    // the design artifact landed (produced by iter 50) → no ghost.
    expect(phaseOf(g, "design").pending).toEqual([])
    expect(phaseOf(g, "design").members).toHaveLength(1)
  })
})

describe("buildProcessGraph: unmapped kinds/stages (version mismatch)", () => {
  it("counts unknown kinds/stages without placing them or inventing a phase", () => {
    const g = buildProcessGraph({
      artifacts: [
        art(1, "issue"),
        art(2, "mystery_kind" as unknown as LoopArtifactKind),
      ],
      links: [],
      liveIterations: [
        iter(9, "mystery_stage" as unknown as LoopStage, { status: "running" }),
      ],
    })
    expect(g.unmappedArtifacts).toBe(1)
    expect(g.unmappedIterations).toBe(1)
    expect(g.phases).toHaveLength(6)
    // the issue still maps; the mystery artifact appears in no phase.
    const total = g.phases.reduce((n, p) => n + p.members.length, 0)
    expect(total).toBe(1)
  })
})

describe("buildProcessGraph: agent-facet capability probe (spec §3.3)", () => {
  const base = { artifacts: [art(2, "requirement")], links: [] }

  it("an array (even empty) ⇒ agentFacetAvailable true", () => {
    expect(
      buildProcessGraph({ ...base, artifactIterationRefs: [] })
        .agentFacetAvailable
    ).toBe(true)
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 2,
        iteration_id: 5,
        stage: "refine",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "codex",
        conversation_id: 100,
        attempt_count: 1,
      },
    ]
    expect(
      buildProcessGraph({ ...base, artifactIterationRefs: refs })
        .agentFacetAvailable
    ).toBe(true)
  })

  it("null ⇒ false (Option<Vec> serialized as null, not undefined)", () => {
    expect(
      buildProcessGraph({ ...base, artifactIterationRefs: null })
        .agentFacetAvailable
    ).toBe(false)
  })

  it("omitted (old server) ⇒ false", () => {
    expect(buildProcessGraph(base).agentFacetAvailable).toBe(false)
  })

  it("an artifact with no producer stays null even with the facet on", () => {
    // base's requirement has produced_by_iteration_id null ⇒ §3.2 case ①.
    const g = buildProcessGraph({ ...base, artifactIterationRefs: [] })
    const m = phaseOf(g, "requirement").members[0]
    expect(m.producedBy).toBeNull()
    expect(m.attemptCount).toBeNull()
  })
})

describe("buildProcessGraph: agent-facet resolution (spec §3.2)", () => {
  it("a resolved ref fills producedBy + attemptCount", () => {
    const a = art(2, "requirement", { produced_by_iteration_id: 5 })
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 2,
        iteration_id: 5,
        stage: "refine",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "codex",
        conversation_id: 100,
        attempt_count: 2,
      },
    ]
    const m = phaseOf(
      buildProcessGraph({
        artifacts: [a],
        links: [],
        artifactIterationRefs: refs,
      }),
      "requirement"
    ).members[0]
    expect(m.producedBy).toEqual({
      iterationId: 5,
      stage: "refine",
      agentType: "codex",
      conversationId: 100,
    })
    expect(m.attemptCount).toBe(2)
  })

  it("a producer with no matching ref ⇒ unresolved (iterationId null)", () => {
    const a = art(2, "requirement", { produced_by_iteration_id: 999 })
    const m = phaseOf(
      buildProcessGraph({
        artifacts: [a],
        links: [],
        artifactIterationRefs: [],
      }),
      "requirement"
    ).members[0]
    expect(m.producedBy).toEqual({
      iterationId: null,
      stage: null,
      agentType: null,
      conversationId: null,
    })
    expect(m.attemptCount).toBeNull()
  })

  it("facet off ⇒ null even for an artifact with a producer", () => {
    const a = art(2, "requirement", { produced_by_iteration_id: 5 })
    const m = phaseOf(
      buildProcessGraph({ artifacts: [a], links: [] }),
      "requirement"
    ).members[0]
    expect(m.producedBy).toBeNull()
    expect(m.attemptCount).toBeNull()
  })

  it("a folded review resolves its own producedBy from refs", () => {
    const task = art(10, "task", { produced_by_iteration_id: 1 })
    const review = art(11, "review", { produced_by_iteration_id: 7 })
    const refs: ArtifactIterationRef[] = [
      {
        artifact_id: 10,
        iteration_id: 1,
        stage: "plan",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "claude_code",
        conversation_id: 1,
        attempt_count: 1,
      },
      {
        artifact_id: 11,
        iteration_id: 7,
        stage: "review",
        status: "succeeded",
        outcome: "succeeded",
        agent_type: "codex",
        conversation_id: 9,
        attempt_count: 1,
      },
    ]
    const impl = phaseOf(
      buildProcessGraph({
        artifacts: [task, review],
        links: [link(1, 11, 10, "reviews")],
        artifactIterationRefs: refs,
      }),
      "implement"
    )
    expect(impl.members[0].producedBy?.agentType).toBe("claude_code")
    expect(impl.members[0].reviews[0].producedBy).toEqual({
      iterationId: 7,
      stage: "review",
      agentType: "codex",
      conversationId: 9,
    })
  })

  it("sessionRefs take live artifact-less triage→Issue, finalize→Result only", () => {
    const live = [
      iter(1, "triage", {
        status: "running",
        agent_type: "codex",
        conversation_id: 5,
      }),
      iter(2, "finalize", {
        status: "queued",
        agent_type: "claude_code",
        conversation_id: 6,
      }),
      iter(3, "finalize", { status: "running", target_artifact_id: 99 }), // targeted → out
      iter(4, "triage", { status: "succeeded" }), // settled → out
    ]
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: live,
      artifactIterationRefs: [],
    })
    expect(phaseOf(g, "issue").sessionRefs.map((s) => s.iterationId)).toEqual([
      1,
    ])
    expect(phaseOf(g, "issue").sessionRefs[0].agentType).toBe("codex")
    expect(phaseOf(g, "result").sessionRefs.map((s) => s.iterationId)).toEqual([
      2,
    ])
    // session_only becomes reachable (no members, but a live session present).
    expect(phaseOf(g, "issue").emptyReason).toBe("session_only")
  })

  it("facet off ⇒ sessionRefs empty even with live triage", () => {
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: [iter(1, "triage", { status: "running" })],
    })
    expect(phaseOf(g, "issue").sessionRefs).toEqual([])
  })

  it("a pending ghost carries its iteration's agentType", () => {
    const g = buildProcessGraph({
      artifacts: [],
      links: [],
      liveIterations: [
        iter(1, "design", { status: "running", agent_type: "gemini" }),
      ],
      artifactIterationRefs: [],
    })
    const pending = phaseOf(g, "design").pending
    expect(pending).toHaveLength(1)
    expect(pending[0].agentType).toBe("gemini")
  })
})

// ---------------------------------------------------------------------------
// Golden fixture = issue #1 (the real "坦克大战" issue that motivated this work):
// 1 issue + 12 requirements + 1 design + 10 tasks (task0 an isolated parallel
// root; task1→…→task9 a serial chain) + 1 review, no result/reflection. The
// 34 lineage edges that sprawl as edge-soup today must fold into 3 connectors.
// ---------------------------------------------------------------------------

function issueOneFixture(): {
  artifacts: LoopArtifactRow[]
  links: LoopLinkRow[]
} {
  const ISSUE = 1
  const REQ_IDS = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13] // 12
  const DESIGN = 14
  const TASK_IDS = [15, 16, 17, 18, 19, 20, 21, 22, 23, 24] // task0..task9
  const REVIEW = 25

  const artifacts: LoopArtifactRow[] = [
    art(ISSUE, "issue", { status: "done" }),
    ...REQ_IDS.map((id) => art(id, "requirement", { status: "done" })),
    art(DESIGN, "design", { status: "done" }),
    // task0 (id 15) is in flight → Implement is active; the chain is done.
    ...TASK_IDS.map((id, i) =>
      art(id, "task", { status: i === 0 ? "in_progress" : "done" })
    ),
    art(REVIEW, "review", { status: "done", verdict: "pass" }),
  ]

  let linkId = 0
  const links: LoopLinkRow[] = []
  // 12 requirement --derives_from--> issue
  for (const r of REQ_IDS) links.push(link(++linkId, r, ISSUE, "derives_from"))
  // 12 design --derives_from--> requirement
  for (const r of REQ_IDS) links.push(link(++linkId, DESIGN, r, "derives_from"))
  // 10 task --derives_from--> design
  for (const t of TASK_IDS)
    links.push(link(++linkId, t, DESIGN, "derives_from"))
  // 8 depends_on chain task1→…→task9 (task[i] depends_on task[i-1]); task0 isolated
  for (let i = 2; i < TASK_IDS.length; i++) {
    links.push(link(++linkId, TASK_IDS[i], TASK_IDS[i - 1], "depends_on"))
  }
  // 1 review --reviews--> task1 (id 16)
  links.push(link(++linkId, REVIEW, 16, "reviews"))

  return { artifacts, links }
}

describe("buildProcessGraph: golden fixture (issue #1)", () => {
  const { artifacts, links } = issueOneFixture()
  const g = buildProcessGraph({ artifacts, links })

  it("folds 34 lineage edges into exactly 3 connectors (12/12/10)", () => {
    expect(g.connectors).toHaveLength(3)
    const byPair = Object.fromEntries(
      g.connectors.map((c) => [`${c.earlier}->${c.later}`, c])
    )
    expect(byPair["issue->requirement"].totalCount).toBe(12)
    expect(byPair["requirement->design"].totalCount).toBe(12)
    expect(byPair["design->implement"].totalCount).toBe(10)
    for (const c of g.connectors) {
      expect(c.connectorKind).toBe("lineage")
      // nothing is dead → active == total.
      expect(c.activeCount).toBe(c.totalCount)
      expect(phaseRank(c.earlier)).toBeLessThan(phaseRank(c.later))
    }
    expect(g.unexpectedSamePhaseLineage).toEqual([])
  })

  it("assigns each phase the right state", () => {
    expect(phaseOf(g, "issue").state).toBe("done")
    expect(phaseOf(g, "requirement").state).toBe("done")
    expect(phaseOf(g, "design").state).toBe("done")
    expect(phaseOf(g, "implement").state).toBe("active") // task0 in_progress
    expect(phaseOf(g, "result").state).toBe("empty")
    expect(phaseOf(g, "result").emptyReason).toBe("no_members")
    expect(phaseOf(g, "reflect").state).toBe("empty")
    expect(phaseOf(g, "reflect").emptyReason).toBe("no_members")
  })

  it("Implement holds 10 task members across 2 lanes with a serial chain", () => {
    const impl = phaseOf(g, "implement")
    expect(impl.members).toHaveLength(10) // tasks only; the review is folded
    const lanes = new Set(impl.members.map((m) => m.lane))
    expect(lanes).toEqual(new Set([0, 1])) // task0 alone, chain shares a lane
    // task0 (id 15) is the isolated root in its own lane at column 0.
    const task0 = impl.members.find((m) => m.artifact.id === 15)!
    expect(task0.col).toBe(0)
    // the chain head task1 (id 16) is column 0; task9 (id 24) is column 8.
    expect(impl.members.find((m) => m.artifact.id === 16)!.col).toBe(0)
    expect(impl.members.find((m) => m.artifact.id === 24)!.col).toBe(8)
    // 8 depends_on edges, all inside Implement.
    expect(impl.workflow).toHaveLength(8)
  })

  it("folds the review into task1 (not a standalone member)", () => {
    const impl = phaseOf(g, "implement")
    const task1 = impl.members.find((m) => m.artifact.id === 16)!
    expect(task1.reviews.map((r) => r.artifact.id)).toEqual([25])
    expect(impl.members.some((m) => m.artifact.kind === "review")).toBe(false)
  })

  it("reports a clean, agent-facet-off model in P1", () => {
    expect(g.unmappedArtifacts).toBe(0)
    expect(g.unmappedIterations).toBe(0)
    expect(g.supersededCount).toBe(0)
    expect(g.agentFacetAvailable).toBe(false)
    for (const p of g.phases) {
      expect(p.sessionRefs).toEqual([])
      for (const m of p.members) {
        expect(m.producedBy).toBeNull()
        expect(m.attemptCount).toBeNull()
      }
    }
  })
})
