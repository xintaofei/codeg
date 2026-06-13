import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { BoardView } from "./board-view"
import type { LoopArtifactRow } from "@/lib/types"

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
    updated_at: "2026-06-14T00:00:00Z",
    ...over,
  }
}

describe("BoardView", () => {
  it("lays out cards in per-kind columns and opens one on click", () => {
    const onSelect = vi.fn()
    render(
      <BoardView
        onSelect={onSelect}
        artifacts={[
          art({ id: 1, kind: "issue", title: "Root" }), // excluded
          art({ id: 2, kind: "task", title: "Task A", status: "pending" }),
          art({ id: 3, kind: "review", title: "Review X", status: "done" }),
        ]}
      />
    )

    // Five kind columns are always present (issue is not a column).
    for (const col of ["requirement", "design", "task", "review", "result"]) {
      expect(screen.getByText(col)).toBeInTheDocument()
    }
    expect(screen.queryByText("Root")).not.toBeInTheDocument()
    expect(screen.getByText("Task A")).toBeInTheDocument()

    fireEvent.click(screen.getByText("Review X"))
    expect(onSelect).toHaveBeenCalledWith(3)
  })

  it("shows the empty state with no non-issue artifacts", () => {
    render(
      <BoardView
        onSelect={() => {}}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
      />
    )
    expect(screen.getByText("empty")).toBeInTheDocument()
  })
})
