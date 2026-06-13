import { describe, it, expect, vi, beforeEach } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"

import { InboxPanel } from "./inbox-panel"
import type { LoopInboxItemRow } from "@/lib/types"

// Stable `t` so the panel's `refresh` callback identity is steady (per next-intl
// mock guidance); returns the key verbatim, which is enough to address labels.
const {
  stableT,
  listLoopInbox,
  approveLoopDesign,
  rejectLoopDesign,
  approveLoopMerge,
  rejectLoopMerge,
  cancelLoopIssue,
} = vi.hoisted(() => ({
  stableT: (key: string) => key,
  listLoopInbox: vi.fn(),
  approveLoopDesign: vi.fn(),
  rejectLoopDesign: vi.fn(),
  approveLoopMerge: vi.fn(),
  rejectLoopMerge: vi.fn(),
  cancelLoopIssue: vi.fn(),
}))

vi.mock("next-intl", () => ({ useTranslations: () => stableT }))
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }))
vi.mock("@/hooks/use-loop-changed", () => ({ useLoopChanged: () => {} }))
vi.mock("@/lib/loops-api", () => ({
  listLoopInbox,
  approveLoopDesign,
  rejectLoopDesign,
  approveLoopMerge,
  rejectLoopMerge,
  cancelLoopIssue,
}))

function item(over: Partial<LoopInboxItemRow> = {}): LoopInboxItemRow {
  return {
    id: 1,
    issue_id: 7,
    issue_seq: 3,
    iteration_id: null,
    kind: "approval",
    subject_key: "design:7",
    payload: { gate: "design" },
    status: "pending",
    created_at: "2026-06-14T00:00:00Z",
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe("InboxPanel", () => {
  it("shows the empty state when nothing is pending", async () => {
    listLoopInbox.mockResolvedValue([])
    render(<InboxPanel spaceId={1} />)
    expect(await screen.findByText("empty")).toBeInTheDocument()
  })

  it("approves a design gate", async () => {
    listLoopInbox.mockResolvedValue([item()])
    approveLoopDesign.mockResolvedValue(undefined)
    render(<InboxPanel spaceId={1} />)
    fireEvent.click(await screen.findByRole("button", { name: "approve" }))
    await waitFor(() => expect(approveLoopDesign).toHaveBeenCalledWith(7))
  })

  it("approves a merge gate via the Merge action", async () => {
    listLoopInbox.mockResolvedValue([
      item({ id: 2, subject_key: "merge:7", payload: { gate: "merge" } }),
    ])
    approveLoopMerge.mockResolvedValue(undefined)
    render(<InboxPanel spaceId={1} />)
    fireEvent.click(await screen.findByRole("button", { name: "merge" }))
    await waitFor(() => expect(approveLoopMerge).toHaveBeenCalledWith(7))
  })

  it("rejects a design gate with a comment", async () => {
    listLoopInbox.mockResolvedValue([item()])
    rejectLoopDesign.mockResolvedValue(undefined)
    render(<InboxPanel spaceId={1} />)
    fireEvent.click(await screen.findByRole("button", { name: "reject" }))
    const textarea = await screen.findByPlaceholderText("rejectPlaceholder")
    fireEvent.change(textarea, { target: { value: "need more detail" } })
    fireEvent.click(screen.getByRole("button", { name: "submitReject" }))
    await waitFor(() =>
      expect(rejectLoopDesign).toHaveBeenCalledWith(7, "need more detail")
    )
  })

  it("cancels the issue for a blocked card", async () => {
    listLoopInbox.mockResolvedValue([
      item({
        id: 3,
        kind: "blocked",
        subject_key: "no_progress:9",
        payload: { reason: "max_attempts" },
      }),
    ])
    cancelLoopIssue.mockResolvedValue(undefined)
    render(<InboxPanel spaceId={1} />)
    fireEvent.click(await screen.findByRole("button", { name: "cancelIssue" }))
    await waitFor(() => expect(cancelLoopIssue).toHaveBeenCalledWith(7))
  })
})
