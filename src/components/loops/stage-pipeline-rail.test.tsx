import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { StagePipelineRail, deriveStages } from "./stage-pipeline-rail"
import type { LoopArtifactRow, LoopIterationRow, LoopStage } from "@/lib/types"

const stableT = (key: string) => key
vi.mock("next-intl", () => ({ useTranslations: () => stableT }))

function art(over: Partial<LoopArtifactRow>): LoopArtifactRow {
  return {
    id: 1,
    issue_id: 1,
    issue_seq: 1,
    kind: "task",
    title: "Artifact",
    status: "done",
    origin: "agent",
    produced_by_iteration_id: null,
    verdict: null,
    attempt: 0,
    sort: 0,
    updated_at: "2026-06-17T00:00:00Z",
    ...over,
  }
}

function iter(over: Partial<LoopIterationRow>): LoopIterationRow {
  return {
    id: 100,
    issue_id: 1,
    issue_seq: 1,
    stage: "design",
    target_artifact_id: null,
    target_title: null,
    conversation_id: null,
    status: "running",
    launched_by: "engine",
    attempt: 0,
    tokens_used: 0,
    created_at: "2026-06-17T00:00:00Z",
    started_at: "2026-06-17T00:00:00Z",
    ended_at: null,
    ...over,
  }
}

const statusOf = (
  ...args: Parameters<typeof deriveStages>
): Record<LoopStage, string> => {
  const map = {} as Record<LoopStage, string>
  for (const v of deriveStages(...args)) map[v.stage] = v.status
  return map
}

describe("deriveStages", () => {
  it("marks a stage active when it has a live iteration", () => {
    const s = statusOf(
      "full",
      [],
      [iter({ stage: "design", status: "running" })]
    )
    expect(s.design).toBe("active")
  })

  it("marks design skipped on the skip_design route", () => {
    expect(statusOf("skip_design", [], []).design).toBe("skipped")
  })

  it("marks refine and design skipped on the direct route", () => {
    const s = statusOf("direct", [], [])
    expect(s.refine).toBe("skipped")
    expect(s.design).toBe("skipped")
  })

  it("blocks implement (not plan) when a task is blocked and nothing is live", () => {
    const s = statusOf("full", [art({ kind: "task", status: "blocked" })], [])
    expect(s.implement).toBe("blocked")
    // plan only produces the tasks; a blocked task is implement's concern.
    expect(s.plan).toBe("done")
  })

  it("marks implement active when an implement iteration is live", () => {
    const arts = [
      art({ id: 1, kind: "task", status: "done" }),
      art({ id: 2, kind: "task", status: "in_progress" }),
    ]
    const s = statusOf("full", arts, [
      iter({ stage: "implement", status: "running", target_artifact_id: 2 }),
    ])
    expect(s.implement).toBe("active")
  })

  it("marks implement done only when every task is terminal", () => {
    expect(
      statusOf("full", [art({ kind: "task", status: "done" })], []).implement
    ).toBe("done")
    const partial = statusOf(
      "full",
      [
        art({ id: 1, kind: "task", status: "done" }),
        art({ id: 2, kind: "task", status: "pending" }),
      ],
      []
    )
    expect(partial.implement).not.toBe("done")
  })

  it("marks triage done once the route is decided, pending while undecided", () => {
    expect(statusOf("undecided", [], []).triage).toBe("pending")
    expect(statusOf("full", [], []).triage).toBe("done")
  })

  it("marks a read stage done when its artifact has landed", () => {
    expect(statusOf("full", [art({ kind: "result" })], []).finalize).toBe(
      "done"
    )
  })

  it("ignores superseded artifacts (a re-run shows its live stage, not done)", () => {
    // A superseded requirement + a live refine re-run → refine active, not done.
    const s = statusOf(
      "full",
      [art({ kind: "requirement", status: "superseded" })],
      [iter({ stage: "refine", status: "running" })]
    )
    expect(s.refine).toBe("active")
  })

  it("keeps implement pending when tasks are incomplete despite later evidence", () => {
    // 1 done + 1 pending task, plus a review artifact (a later stage has landed).
    // The passed-stage fallback must NOT override the completion ratio.
    const s = statusOf(
      "full",
      [
        art({ id: 1, kind: "task", status: "done" }),
        art({ id: 2, kind: "task", status: "pending" }),
        art({ id: 3, kind: "review", status: "done" }),
      ],
      []
    )
    expect(s.implement).toBe("pending")
  })
})

describe("StagePipelineRail", () => {
  it("renders all eight stage pills", () => {
    render(
      <StagePipelineRail route="full" artifacts={[]} liveIterations={[]} />
    )
    for (const stage of [
      "triage",
      "refine",
      "design",
      "plan",
      "implement",
      "review",
      "finalize",
      "reflect",
    ]) {
      expect(screen.getByText(stage)).toBeInTheDocument()
    }
  })

  it("exposes each stage as a listitem with name and status text (not color alone)", () => {
    render(
      <StagePipelineRail
        route="full"
        artifacts={[]}
        liveIterations={[iter({ stage: "design", status: "running" })]}
      />
    )
    // Explicit list semantics: one listitem per stage.
    expect(screen.getAllByRole("listitem")).toHaveLength(8)
    // The active stage exposes both its name and a textual status (sr-only),
    // never relying on color/icon alone.
    expect(screen.getByText("design")).toBeInTheDocument()
    expect(screen.getByText("statusActive")).toBeInTheDocument()
  })
})
