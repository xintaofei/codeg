import { describe, expect, it } from "vitest"

import { buildDag, foldReviews, STAGE_COLUMNS } from "@/lib/loop-dag"
import type {
  LoopArtifactKind,
  LoopArtifactRow,
  LoopLinkKind,
  LoopLinkRow,
} from "@/lib/types"

let nextId = 1

function artifact(
  kind: LoopArtifactKind,
  extra: Partial<LoopArtifactRow> = {}
): LoopArtifactRow {
  return {
    id: nextId++,
    issue_id: 1,
    issue_seq: 1,
    kind,
    title: `${kind}-${nextId}`,
    status: "done",
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    attempt: 0,
    sort: 0,
    updated_at: "2026-06-13T00:00:00Z",
    ...extra,
  }
}

function link(
  from: number,
  to: number,
  kind: LoopLinkKind = "derives_from"
): LoopLinkRow {
  return {
    id: nextId++,
    from_artifact_id: from,
    to_artifact_id: to,
    kind,
    source_revision_id: null,
  }
}

describe("buildDag", () => {
  it("groups a task with its reviews into one cluster (reviews are not nodes)", () => {
    const issue = artifact("issue")
    const task = artifact("task", { status: "in_progress" })
    const r1 = artifact("review", { attempt: 0 })
    const r2 = artifact("review", { attempt: 0, sort: 1 })
    const { clusters, stageNodes } = buildDag(
      [issue, task, r1, r2],
      [link(r1.id, task.id, "reviews"), link(r2.id, task.id, "reviews")]
    )

    expect(clusters).toHaveLength(1)
    expect(clusters[0].task.id).toBe(task.id)
    expect(clusters[0].reviews.map((r) => r.id)).toEqual([r1.id, r2.id])
    // Reviews never appear as standalone stage nodes.
    expect(stageNodes.some((n) => n.artifact.kind === "review")).toBe(false)
  })

  it("places independent tasks in parallel lanes at the same column", () => {
    const issue = artifact("issue")
    const a = artifact("task", { sort: 0 })
    const b = artifact("task", { sort: 1 })
    const { clusters, laneCount } = buildDag([issue, a, b], [])

    const ca = clusters.find((c) => c.task.id === a.id)!
    const cb = clusters.find((c) => c.task.id === b.id)!
    expect(ca.col).toBe(cb.col) // both at the base task column
    expect(ca.lane).not.toBe(cb.lane) // but distinct lanes
    expect(laneCount).toBe(2)
  })

  it("runs a depends_on chain rightward in a shared lane and keeps the edge", () => {
    const issue = artifact("issue")
    const a = artifact("task", { sort: 0 })
    const b = artifact("task", { sort: 1 })
    // b depends_on a: edge tail = dependent (b), head = predecessor (a).
    const { clusters, edges } = buildDag(
      [issue, a, b],
      [link(b.id, a.id, "depends_on")]
    )

    const ca = clusters.find((c) => c.task.id === a.id)!
    const cb = clusters.find((c) => c.task.id === b.id)!
    expect(cb.col).toBe(ca.col + 1) // chain runs one column rightward
    expect(cb.lane).toBe(ca.lane) // same lane (horizontal chain)

    const dep = edges.find((e) => e.kind === "depends_on")
    expect(dep).toBeDefined()
    expect(dep!.from).toBe(b.id)
    expect(dep!.to).toBe(a.id)
  })

  it("fans out a parent's extra children into new lanes at the next column", () => {
    const issue = artifact("issue")
    const a = artifact("task", { sort: 0 })
    const b = artifact("task", { sort: 1 })
    const c = artifact("task", { sort: 2 })
    // b and c both depend_on a (fan-out).
    const { clusters } = buildDag(
      [issue, a, b, c],
      [link(b.id, a.id, "depends_on"), link(c.id, a.id, "depends_on")]
    )
    const ca = clusters.find((x) => x.task.id === a.id)!
    const cb = clusters.find((x) => x.task.id === b.id)!
    const cc = clusters.find((x) => x.task.id === c.id)!

    expect(cb.col).toBe(ca.col + 1)
    expect(cc.col).toBe(ca.col + 1)
    expect(ca.lane).toBe(cb.lane) // parent aligns with its first child
    expect(cc.lane).not.toBe(cb.lane) // the second child gets its own lane
  })

  it("places read stages in fixed columns and result at the trailing column", () => {
    const issue = artifact("issue")
    const req = artifact("requirement")
    const design = artifact("design")
    const task = artifact("task")
    const result = artifact("result")
    const { stageNodes, result: res } = buildDag(
      [issue, req, design, task, result],
      []
    )

    const colOf = (id: number) =>
      stageNodes.find((n) => n.artifact.id === id)?.col
    expect(colOf(issue.id)).toBe(STAGE_COLUMNS.indexOf("issue"))
    expect(colOf(req.id)).toBe(STAGE_COLUMNS.indexOf("requirement"))
    expect(colOf(design.id)).toBe(STAGE_COLUMNS.indexOf("design"))
    // result closes after the task column (3) → column 4.
    expect(res?.col).toBe(4)
  })

  it("marks skips_to edges dashed and derivation edges solid", () => {
    const issue = artifact("issue")
    const task = artifact("task", { status: "pending" })
    const { edges } = buildDag(
      [issue, task],
      [link(task.id, issue.id, "skips_to"), link(task.id, issue.id)]
    )

    expect(edges.find((e) => e.kind === "skips_to")?.dashed).toBe(true)
    expect(edges.find((e) => e.kind === "derives_from")?.dashed).toBe(false)
  })

  it("preserves edge direction: tail = dependent, head = referenced", () => {
    const issue = artifact("issue")
    const req = artifact("requirement")
    const { edges } = buildDag([issue, req], [link(req.id, issue.id)])

    expect(edges).toHaveLength(1)
    expect(edges[0].from).toBe(req.id)
    expect(edges[0].to).toBe(issue.id)
  })

  it("drops edges that dangle or touch a folded review", () => {
    const issue = artifact("issue")
    const task = artifact("task")
    const review = artifact("review")
    const { edges } = buildDag(
      [issue, task, review],
      [
        link(task.id, issue.id), // kept
        link(task.id, 9999), // dangling → dropped
        link(review.id, task.id, "reviews"), // touches a review → dropped
      ]
    )

    expect(edges).toHaveLength(1)
    expect(edges[0].from).toBe(task.id)
    expect(edges[0].to).toBe(issue.id)
  })

  it("hides superseded/cancelled tasks by default and reports the count", () => {
    const issue = artifact("issue")
    const design = artifact("design")
    const live = artifact("task", { status: "pending", sort: 1 })
    const old = artifact("task", { status: "superseded", sort: 0 })
    const cancelled = artifact("task", { status: "cancelled", sort: 2 })
    const layout = buildDag(
      [issue, design, live, old, cancelled],
      [
        link(live.id, design.id), // live task → design (kept)
        link(old.id, design.id), // superseded task → design (drops with the node)
      ]
    )

    expect(layout.clusters.map((c) => c.task.id)).toEqual([live.id])
    expect(layout.supersededCount).toBe(2)
    // The design no longer connects to a dead task.
    expect(layout.edges.some((e) => e.from === old.id || e.to === old.id)).toBe(
      false
    )
    // The live task's lineage survives.
    expect(
      layout.edges.some((e) => e.from === live.id && e.to === design.id)
    ).toBe(true)
  })

  it("includeSuperseded reveals dead nodes and their edges", () => {
    const issue = artifact("issue")
    const design = artifact("design")
    const live = artifact("task", { status: "pending", sort: 1 })
    const old = artifact("task", { status: "superseded", sort: 0 })
    const layout = buildDag(
      [issue, design, live, old],
      [link(live.id, design.id), link(old.id, design.id)],
      { includeSuperseded: true }
    )

    expect(layout.clusters.map((c) => c.task.id).sort((a, b) => a - b)).toEqual(
      [live.id, old.id].sort((a, b) => a - b)
    )
    // Count reflects the full input regardless of the toggle (so it can hide again).
    expect(layout.supersededCount).toBe(1)
    expect(layout.edges.some((e) => e.from === old.id)).toBe(true)
  })

  it("folds a dead review under a live task only when superseded are revealed", () => {
    const issue = artifact("issue")
    const task = artifact("task", { status: "done" })
    const liveReview = artifact("review", { status: "done", sort: 0 })
    const deadReview = artifact("review", { status: "cancelled", sort: 1 })
    const links = [
      link(liveReview.id, task.id, "reviews"),
      link(deadReview.id, task.id, "reviews"),
    ]

    // Hidden by default: the dead review is not folded into the live cluster…
    const def = buildDag([issue, task, liveReview, deadReview], links)
    expect(
      def.clusters.find((c) => c.task.id === task.id)!.reviews.map((r) => r.id)
    ).toEqual([liveReview.id])
    expect(def.supersededCount).toBe(1)

    // …revealed: it folds in (so the row can be dimmed by its own status).
    const shown = buildDag([issue, task, liveReview, deadReview], links, {
      includeSuperseded: true,
    })
    expect(
      shown.clusters
        .find((c) => c.task.id === task.id)!
        .reviews.map((r) => r.id)
    ).toEqual([liveReview.id, deadReview.id])
  })
})

describe("foldReviews", () => {
  it("expands the latest attempt and folds older attempts into a count", () => {
    const reviews = [
      artifact("review", { attempt: 0 }),
      artifact("review", { attempt: 0 }),
      artifact("review", { attempt: 1 }),
    ]
    const { latest, olderCount } = foldReviews(reviews)
    expect(latest.every((r) => r.attempt === 1)).toBe(true)
    expect(latest).toHaveLength(1)
    expect(olderCount).toBe(2)
  })

  it("returns empty for a task with no reviews", () => {
    expect(foldReviews([])).toEqual({ latest: [], olderCount: 0 })
  })
})
