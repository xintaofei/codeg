import { describe, expect, it } from "vitest"

import {
  acceptanceOrdinalMap,
  coveringTaskTitles,
  taskCovers,
} from "@/lib/loop-coverage"
import type {
  LoopArtifactDetail,
  LoopArtifactRow,
  LoopCoverageRow,
} from "@/lib/types"

function req(
  id: number,
  sort: number,
  criteria: { id: number; sort: number; kind?: string }[]
): LoopArtifactDetail {
  return {
    id,
    issue_id: 1,
    issue_seq: 1,
    kind: "requirement",
    title: `R${id}`,
    status: "done",
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    attempt: 0,
    sort,
    updated_at: "",
    revisions: [],
    links: [],
    criteria: criteria.map((c) => ({
      id: c.id,
      label: `AC-${c.id}`,
      text: `crit ${c.id}`,
      sort: c.sort,
      kind: (c.kind ?? "acceptance") as never,
    })),
  } as LoopArtifactDetail
}

function task(id: number, title: string, status = "pending"): LoopArtifactRow {
  return {
    id,
    issue_id: 1,
    issue_seq: 1,
    kind: "task",
    title,
    status,
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    attempt: 0,
    sort: 0,
    updated_at: "",
  } as LoopArtifactRow
}

const cov = (taskId: number, critId: number): LoopCoverageRow => ({
  id: 0,
  task_artifact_id: taskId,
  criterion_id: critId,
})

describe("loop-coverage", () => {
  it("assigns R{i}.AC{j} ordinals by (sort,id), acceptance only", () => {
    // R2 (sort 0) before R1 (sort 1); a constraint is skipped.
    const reqs = [
      req(1, 1, [{ id: 11, sort: 0 }]),
      req(2, 0, [
        { id: 20, sort: 0 },
        { id: 21, sort: 1, kind: "constraint" },
        { id: 22, sort: 2 },
      ]),
    ]
    const map = acceptanceOrdinalMap(reqs)
    expect(map.get(20)?.ordinal).toBe("R1.AC1")
    expect(map.get(22)?.ordinal).toBe("R1.AC2")
    expect(map.get(11)?.ordinal).toBe("R2.AC1")
    expect(map.has(21)).toBe(false) // constraint gets no acceptance ordinal
  })

  it("lists covering task titles and flags uncovered", () => {
    const tasks = [task(100, "Build A"), task(101, "Build B")]
    const coverage = [cov(100, 20), cov(101, 20)]
    expect(coveringTaskTitles(20, coverage, tasks)).toEqual([
      "Build A",
      "Build B",
    ])
    expect(coveringTaskTitles(22, coverage, tasks)).toEqual([]) // uncovered

    // A superseded task's coverage row does NOT count — the criterion is a gap.
    const replanned = [task(100, "Build A", "superseded"), task(101, "Build B")]
    expect(coveringTaskTitles(20, [cov(100, 20)], replanned)).toEqual([])
  })

  it("resolves a task's covered criteria as ordinal+text", () => {
    const reqs = [
      req(2, 0, [{ id: 20, sort: 0 }]),
      req(1, 1, [{ id: 11, sort: 0 }]),
    ]
    const map = acceptanceOrdinalMap(reqs)
    const covers = taskCovers(100, [cov(100, 11), cov(100, 20)], map)
    expect(covers.map((c) => c.ordinal)).toEqual(["R1.AC1", "R2.AC1"])
  })
})
