import { describe, expect, it } from "vitest"

import {
  acceptanceOrdinalMap,
  coveringTaskTitles,
  criterionCheckMap,
  taskCovers,
} from "@/lib/loop-coverage"
import type {
  LoopArtifactDetail,
  LoopArtifactRow,
  LoopCoverageRow,
  LoopCriterionCheckRow,
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

  it("orders covered criteria numerically (R10 after R2)", () => {
    const map = new Map([
      [1, { ordinal: "R2.AC1", text: "two" }],
      [2, { ordinal: "R10.AC1", text: "ten" }],
    ])
    const covers = taskCovers(100, [cov(100, 2), cov(100, 1)], map)
    expect(covers.map((c) => c.ordinal)).toEqual(["R2.AC1", "R10.AC1"])
  })

  it("criterionCheckMap scores each criterion from its latest gate round", () => {
    const chk = (
      id: number,
      criterion_id: number,
      scope: number,
      verdict: "pass" | "fail"
    ): LoopCriterionCheckRow => ({
      id,
      criterion_id,
      iteration_id: 0,
      scope_artifact_id: scope,
      verdict,
      evidence: "",
    })
    const dec = (id: number, input_check_ids: number[]) => ({
      id,
      target_artifact_id: 0,
      stage: "review",
      attempt: 0,
      outcome: "pass" as const,
      input_check_ids,
      created_at: "",
    })

    const checks = [
      // Criterion 10 reworked: attempt-0 task round failed (check 1), attempt-1
      // round passed (check 2) — SAME task scope. The newer round must win → pass
      // (no stale fail across the rework).
      chk(1, 10, 500, "fail"),
      chk(2, 10, 500, "pass"),
      // Criterion 11: two reviewers in ONE round disagree (fail 3 + pass 4).
      // Fail-dominant within the round → fail (never a misleading pass).
      chk(3, 11, 500, "fail"),
      chk(4, 11, 500, "pass"),
      // Criterion 12: task round fail (5), then a later integration round pass (6).
      chk(5, 12, 500, "fail"),
      chk(6, 12, 900, "pass"),
      // Criterion 13: a check with NO decision yet (in-flight) → fallback fail-dom.
      chk(7, 13, 500, "fail"),
    ]
    const decisions = [
      dec(10, [1]), // attempt-0 task round (failed) — older
      dec(20, [2]), // attempt-1 task round (passed) — newer, wins criterion 10
      dec(11, [3, 4]), // criterion 11's single round (both reviewers)
      dec(30, [5]), // criterion 12 task round (failed) — older
      dec(40, [6]), // criterion 12 integration round (passed) — newer, wins
    ]

    const map = criterionCheckMap(checks, decisions)
    expect(map.get(10)?.verdict).toBe("pass") // rework: newer round wins, no stale fail
    expect(map.get(11)?.verdict).toBe("fail") // within-round disagreement → fail
    expect(map.get(12)?.verdict).toBe("pass") // integration round superseded the task fail
    expect(map.get(12)?.scope_artifact_id).toBe(900)
    expect(map.get(13)?.verdict).toBe("fail") // undecided fallback
    expect(map.has(99)).toBe(false)
  })
})
