import { describe, expect, it } from "vitest"

import { buildDag, DAG_COLUMNS } from "@/lib/loop-dag"
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
  return { id: nextId++, from_artifact_id: from, to_artifact_id: to, kind }
}

describe("buildDag", () => {
  it("assigns each artifact kind to its fixed column (all six)", () => {
    const arts = DAG_COLUMNS.map((kind) => artifact(kind))
    const { nodes, colCount } = buildDag(arts, [])

    expect(colCount).toBe(6)
    for (const node of nodes) {
      expect(node.col).toBe(DAG_COLUMNS.indexOf(node.artifact.kind))
    }
  })

  it("keeps the first four columns' positions when later stages are absent", () => {
    // M2.1 reaches plan (tasks) but never produces review/result.
    const issue = artifact("issue")
    const req = artifact("requirement")
    const design = artifact("design")
    const task = artifact("task", { status: "pending" })
    const { colCount } = buildDag([issue, req, design, task], [])

    expect(colCount).toBe(4) // issue/requirement/design/task — no trailing gap
    expect(DAG_COLUMNS.slice(0, 4)).toEqual([
      "issue",
      "requirement",
      "design",
      "task",
    ])
  })

  it("marks skips_to edges dashed and derivation edges solid", () => {
    const issue = artifact("issue")
    const task = artifact("task", { status: "pending" })
    const { edges } = buildDag(
      [issue, task],
      [link(task.id, issue.id, "skips_to"), link(task.id, issue.id)]
    )

    const skip = edges.find((e) => e.kind === "skips_to")
    const derive = edges.find((e) => e.kind === "derives_from")
    expect(skip?.dashed).toBe(true)
    expect(derive?.dashed).toBe(false)
  })

  it("preserves edge direction: tail = dependent, head = referenced", () => {
    const issue = artifact("issue")
    const req = artifact("requirement")
    // A requirement derives_from the issue root: tail = req, head = issue.
    const { edges } = buildDag([issue, req], [link(req.id, issue.id)])

    expect(edges).toHaveLength(1)
    expect(edges[0].from).toBe(req.id)
    expect(edges[0].to).toBe(issue.id)
  })

  it("orders nodes within a column by sort then id", () => {
    const issue = artifact("issue")
    const t1 = artifact("task", { sort: 2 })
    const t2 = artifact("task", { sort: 1 })
    const t3 = artifact("task", { sort: 1 })
    const { nodes, rowCount } = buildDag([issue, t1, t2, t3], [])

    const tasks = nodes
      .filter((n) => n.artifact.kind === "task")
      .sort((a, b) => a.row - b.row)
    // sort=1 (t2,t3 by id) before sort=2 (t1).
    expect(tasks.map((n) => n.artifact.id)).toEqual([t2.id, t3.id, t1.id])
    expect(rowCount).toBe(3) // tallest column (task) has three rows
  })

  it("drops edges whose endpoints are missing from the DAG", () => {
    const issue = artifact("issue")
    const req = artifact("requirement")
    const { edges } = buildDag(
      [issue, req],
      [link(req.id, issue.id), link(req.id, 9999)]
    )

    expect(edges).toHaveLength(1)
    expect(edges[0].to).toBe(issue.id)
  })
})
