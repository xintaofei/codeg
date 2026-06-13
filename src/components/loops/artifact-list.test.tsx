import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ArtifactList } from "./artifact-list"
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

describe("ArtifactList", () => {
  it("omits the issue root and renders kind, status and iteration ref", () => {
    const onSelect = vi.fn()
    render(
      <ArtifactList
        onSelect={onSelect}
        artifacts={[
          art({ id: 1, kind: "issue", title: "Root issue" }),
          art({
            id: 2,
            kind: "design",
            title: "Design doc",
            status: "awaiting_approval",
            produced_by_iteration_id: 7,
          }),
        ]}
      />
    )

    expect(screen.queryByText("Root issue")).not.toBeInTheDocument()
    expect(screen.getByText("Design doc")).toBeInTheDocument()
    expect(screen.getByText("design")).toBeInTheDocument() // kind label
    expect(screen.getByText("awaiting_approval")).toBeInTheDocument() // status
    expect(screen.getByText("fromIteration")).toBeInTheDocument() // iter ref

    fireEvent.click(screen.getByText("Design doc"))
    expect(onSelect).toHaveBeenCalledWith(2)
  })

  it("shows the empty state when only the issue root exists", () => {
    render(
      <ArtifactList
        onSelect={() => {}}
        artifacts={[art({ id: 1, kind: "issue", title: "Root" })]}
      />
    )
    expect(screen.getByText("empty")).toBeInTheDocument()
  })
})
