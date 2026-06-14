import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { ArtifactDrawer } from "./artifact-drawer"
import type { LoopArtifactDetail, LoopIssueDetail } from "@/lib/types"

// next-intl: stable identity translator that echoes the key (project mock
// convention) — assertions match key strings / verbatim content, not English.
const stableT = (key: string) => key
vi.mock("next-intl", () => ({ useTranslations: () => stableT }))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

// MessageResponse (Streamdown) pulls in the link-safety hook (workspace
// context) and heavy markdown deps jsdom lacks; stub it to a passthrough that
// renders the raw content so content assertions stay simple.
vi.mock("@/components/ai-elements/message", () => ({
  MessageResponse: ({ children }: { children: string }) => (
    <div data-testid="markdown">{children}</div>
  ),
}))

const getLoopArtifact = vi.fn()
const getLoopIssue = vi.fn()
const approveLoopDesign = vi.fn().mockResolvedValue(undefined)
const rejectLoopDesign = vi.fn().mockResolvedValue(undefined)
const approveLoopMerge = vi.fn().mockResolvedValue(undefined)
const rejectLoopMerge = vi.fn().mockResolvedValue(undefined)
vi.mock("@/lib/loops-api", () => ({
  getLoopArtifact: (...a: unknown[]) => getLoopArtifact(...a),
  getLoopIssue: (...a: unknown[]) => getLoopIssue(...a),
  approveLoopDesign: (...a: unknown[]) => approveLoopDesign(...a),
  rejectLoopDesign: (...a: unknown[]) => rejectLoopDesign(...a),
  approveLoopMerge: (...a: unknown[]) => approveLoopMerge(...a),
  rejectLoopMerge: (...a: unknown[]) => rejectLoopMerge(...a),
}))

// Radix Sheet/Dialog portal through to document.body and need browser APIs jsdom
// lacks; stub them as plain wrappers that honor `open` so the drawer's own
// structure (sections, diffs, gates) is what's under test.
vi.mock("@/components/ui/sheet", () => ({
  Sheet: ({ open, children }: { open: boolean; children: React.ReactNode }) =>
    open ? <div>{children}</div> : null,
  SheetContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SheetHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SheetTitle: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  SheetDescription: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))
vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ open, children }: { open: boolean; children: React.ReactNode }) =>
    open ? <div>{children}</div> : null,
  DialogContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogFooter: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogTitle: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))
vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))

function artifact(over: Partial<LoopArtifactDetail>): LoopArtifactDetail {
  return {
    id: 1,
    issue_id: 5,
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
    revisions: [],
    criteria: [],
    links: [],
    ...over,
  }
}

function issue(status: LoopIssueDetail["status"]): LoopIssueDetail {
  return {
    id: 5,
    space_id: 1,
    seq_no: 1,
    title: "Issue",
    priority: "medium",
    status,
    pause_reason: null,
    route: "full",
    token_used: 0,
    token_budget: null,
    created_at: "2026-06-14T00:00:00Z",
    updated_at: "2026-06-14T00:00:00Z",
    description: "",
    config: {
      v: 1,
      agents: {},
      validation_commands: [],
      reviewer_count: 1,
      review_pass_rule: "unanimous",
      max_attempts: 0,
      auto_merge: false,
      force_route: null,
      iteration_timeout_secs: null,
      token_budget_per_turn: null,
      reviewers: [],
    },
    worktree_folder_id: null,
    base_branch: null,
    base_commit: null,
    active_task_artifact_id: null,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  getLoopIssue.mockResolvedValue(issue("running"))
})

describe("ArtifactDrawer", () => {
  it("renders content, criteria, and a review verdict", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({
        kind: "review",
        verdict: "fail",
        produced_by_iteration_id: 42,
        revisions: [
          {
            id: 10,
            seq: 1,
            content: "review body text",
            actor_kind: "agent",
            iteration_id: 42,
            created_at: "2026-06-14T00:00:00Z",
          },
        ],
        criteria: [{ id: 1, label: "C1", text: "must compile", sort: 0 }],
      })
    )
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    expect(await screen.findByText("review body text")).toBeInTheDocument()
    expect(screen.getByText("C1")).toBeInTheDocument()
    expect(screen.getByText("fail")).toBeInTheDocument() // verdict badge
    expect(screen.getByText("producedBy")).toBeInTheDocument() // linked iteration
    // A review is not a gate — no approve/merge controls.
    expect(screen.queryByText("approve")).not.toBeInTheDocument()
    expect(screen.queryByText("merge")).not.toBeInTheDocument()
  })

  it("renders a colored line diff between adjacent revisions", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({
        kind: "design",
        status: "done",
        revisions: [
          {
            id: 1,
            seq: 1,
            content: "alpha",
            actor_kind: "agent",
            iteration_id: null,
            created_at: "2026-06-14T00:00:00Z",
          },
          {
            id: 2,
            seq: 2,
            content: "alpha\nbeta",
            actor_kind: "agent",
            iteration_id: null,
            created_at: "2026-06-14T00:01:00Z",
          },
        ],
      })
    )
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    // The added line is tagged with the add color; the unchanged line is context.
    const added = await screen.findByText("beta")
    expect(added).toHaveClass("text-emerald-700")
    expect(screen.getByText("alpha")).toHaveClass("text-muted-foreground")
  })

  it("approves a design gate via approveLoopDesign(issue_id)", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({ kind: "design", status: "awaiting_approval", issue_id: 5 })
    )
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    fireEvent.click(await screen.findByText("approve"))
    await waitFor(() => expect(approveLoopDesign).toHaveBeenCalledWith(5))
    // Re-loads after the action so the resolved status reflects.
    await waitFor(() => expect(getLoopArtifact).toHaveBeenCalledTimes(2))
  })

  it("rejects a design gate with a comment", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({ kind: "design", status: "awaiting_approval", issue_id: 5 })
    )
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    fireEvent.click(await screen.findByText("reject"))
    const box = await screen.findByPlaceholderText("rejectPlaceholder")
    fireEvent.change(box, { target: { value: "needs work" } })
    fireEvent.click(screen.getByText("submitReject"))
    await waitFor(() =>
      expect(rejectLoopDesign).toHaveBeenCalledWith(5, "needs work")
    )
  })

  it("shows the merge gate for a result whose issue is running", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({ kind: "result", status: "done", issue_id: 7 })
    )
    getLoopIssue.mockResolvedValue(issue("running"))
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    fireEvent.click(await screen.findByText("merge"))
    await waitFor(() => expect(approveLoopMerge).toHaveBeenCalledWith(7))
  })

  it("hides the merge gate once the issue is no longer running", async () => {
    getLoopArtifact.mockResolvedValue(
      artifact({ kind: "result", status: "done", issue_id: 7 })
    )
    getLoopIssue.mockResolvedValue(issue("done"))
    render(<ArtifactDrawer artifactId={1} onClose={() => {}} />)

    // Wait for the issue fetch to resolve, then assert no merge control.
    await waitFor(() => expect(getLoopIssue).toHaveBeenCalled())
    expect(screen.queryByText("merge")).not.toBeInTheDocument()
  })
})
