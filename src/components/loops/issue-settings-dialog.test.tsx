import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { IssueSettingsDialog } from "./issue-settings-dialog"
import type { IssueConfig, LoopIssueDetail } from "@/lib/types"

const stableT = (key: string) => key
vi.mock("next-intl", () => ({ useTranslations: () => stableT }))

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))

const updateLoopIssueConfig = vi.fn().mockResolvedValue(undefined)
vi.mock("@/lib/loops-api", () => ({
  updateLoopIssueConfig: (...a: unknown[]) => updateLoopIssueConfig(...a),
}))

// Dialog/Select/Switch portal + need browser APIs jsdom lacks — stub Dialog as
// an open-honoring wrapper, Select as a native <select>, Switch as a checkbox.
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
  DialogDescription: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}))
vi.mock("@/components/ui/select", () => ({
  Select: ({
    value,
    onValueChange,
    children,
  }: {
    value: string
    onValueChange: (v: string) => void
    children: React.ReactNode
  }) => (
    <select value={value} onChange={(e) => onValueChange(e.target.value)}>
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
  SelectItem: ({
    value,
    children,
  }: {
    value: string
    children: React.ReactNode
  }) => <option value={value}>{children}</option>,
}))
vi.mock("@/components/ui/switch", () => ({
  Switch: ({
    id,
    checked,
    onCheckedChange,
  }: {
    id?: string
    checked: boolean
    onCheckedChange: (v: boolean) => void
  }) => (
    <input
      type="checkbox"
      role="switch"
      id={id}
      checked={checked}
      onChange={(e) => onCheckedChange(e.target.checked)}
    />
  ),
}))

function fullConfig(): IssueConfig {
  return {
    v: 1,
    agents: { default: "claude_code", review: "codex" },
    validation_commands: ["pnpm test"],
    reviewer_count: 2,
    review_pass_rule: "majority",
    max_attempts: 6,
    auto_merge: true,
    force_route: "full",
    iteration_timeout_secs: 120,
    token_budget_per_turn: 1000,
  }
}

function makeIssue(over: Partial<LoopIssueDetail> = {}): LoopIssueDetail {
  return {
    id: 5,
    space_id: 1,
    seq_no: 1,
    title: "Issue",
    priority: "medium",
    status: "pending",
    pause_reason: null,
    route: "full",
    token_used: 0,
    token_budget: 50000,
    created_at: "2026-06-14T00:00:00Z",
    updated_at: "2026-06-14T00:00:00Z",
    description: "",
    config: fullConfig(),
    worktree_folder_id: null,
    base_branch: null,
    base_commit: null,
    active_task_artifact_id: null,
    ...over,
  }
}

beforeEach(() => vi.clearAllMocks())

describe("IssueSettingsDialog", () => {
  it("round-trips the config and total budget on save with no edits", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.click(screen.getByText("save"))
    await waitFor(() =>
      expect(updateLoopIssueConfig).toHaveBeenCalledWith(5, fullConfig(), 50000)
    )
  })

  it("persists an edited reviewer count and toggled auto-merge", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.change(screen.getByLabelText("reviewerCount"), {
      target: { value: "3" },
    })
    fireEvent.click(screen.getByRole("switch")) // auto_merge: true -> false
    fireEvent.click(screen.getByText("save"))

    await waitFor(() => expect(updateLoopIssueConfig).toHaveBeenCalled())
    const [, config] = updateLoopIssueConfig.mock.calls[0]
    expect(config.reviewer_count).toBe(3)
    expect(config.auto_merge).toBe(false)
  })

  it("adds a validation command", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.click(screen.getByText("addCommand"))
    const inputs = screen.getAllByPlaceholderText("commandPlaceholder")
    fireEvent.change(inputs[inputs.length - 1], {
      target: { value: "pnpm build" },
    })
    fireEvent.click(screen.getByText("save"))

    await waitFor(() => expect(updateLoopIssueConfig).toHaveBeenCalled())
    const [, config] = updateLoopIssueConfig.mock.calls[0]
    expect(config.validation_commands).toEqual(["pnpm test", "pnpm build"])
  })

  it("clears the total budget to unlimited (null)", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.change(screen.getByLabelText("tokenBudget"), {
      target: { value: "" },
    })
    fireEvent.click(screen.getByText("save"))
    await waitFor(() =>
      expect(updateLoopIssueConfig).toHaveBeenCalledWith(5, fullConfig(), null)
    )
  })

  it("shows the running hint only while the issue is running", () => {
    const { rerender } = render(
      <IssueSettingsDialog
        open
        onOpenChange={() => {}}
        issue={makeIssue({ status: "running" })}
      />
    )
    expect(screen.getByText("runningHint")).toBeInTheDocument()

    rerender(
      <IssueSettingsDialog
        open
        onOpenChange={() => {}}
        issue={makeIssue({ status: "pending" })}
      />
    )
    expect(screen.queryByText("runningHint")).not.toBeInTheDocument()
  })
})
