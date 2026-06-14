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

// The tabbed form is unit-tested in loop-config-form.test.tsx; here we stub the
// component (keeping the real config↔form helpers) and focus on the dialog's
// own logic: inherit/custom toggle, total budget, and the save wiring.
vi.mock("./loop-config-form", async (orig) => {
  const real = await orig<typeof import("./loop-config-form")>()
  return {
    ...real,
    LoopConfigForm: ({ disabled }: { disabled?: boolean }) => (
      <div data-testid="config-form" data-disabled={String(!!disabled)} />
    ),
  }
})

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
    reviewers: [],
    stall_alert_secs: null,
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
    config_inherits: false,
    worktree_folder_id: null,
    base_branch: null,
    base_commit: null,
    active_task_artifact_id: null,
    ...over,
  }
}

beforeEach(() => vi.clearAllMocks())

describe("IssueSettingsDialog", () => {
  it("saves a custom config with config_inherits=false", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.click(screen.getByText("save"))
    await waitFor(() =>
      expect(updateLoopIssueConfig).toHaveBeenCalledWith(
        5,
        fullConfig(),
        50000,
        false
      )
    )
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
      expect(updateLoopIssueConfig).toHaveBeenCalledWith(
        5,
        fullConfig(),
        null,
        false
      )
    )
  })

  it("saves with config_inherits=true and a disabled form when inheriting", async () => {
    render(
      <IssueSettingsDialog
        open
        onOpenChange={() => {}}
        issue={makeIssue({ config_inherits: true })}
      />
    )
    expect(screen.getByTestId("config-form").dataset.disabled).toBe("true")
    fireEvent.click(screen.getByText("save"))
    await waitFor(() => {
      const call = updateLoopIssueConfig.mock.calls[0]
      expect(call[0]).toBe(5)
      expect(call[2]).toBe(50000)
      expect(call[3]).toBe(true)
    })
  })

  it("switches an inheriting issue to custom and saves false", async () => {
    render(
      <IssueSettingsDialog
        open
        onOpenChange={() => {}}
        issue={makeIssue({ config_inherits: true })}
      />
    )
    fireEvent.click(screen.getByText("custom"))
    expect(screen.getByTestId("config-form").dataset.disabled).toBe("false")
    fireEvent.click(screen.getByText("save"))
    await waitFor(() =>
      expect(updateLoopIssueConfig.mock.calls[0][3]).toBe(false)
    )
  })

  it("switches a custom issue to space default and saves true", async () => {
    render(
      <IssueSettingsDialog open onOpenChange={() => {}} issue={makeIssue()} />
    )
    fireEvent.click(screen.getByText("useSpaceDefault"))
    fireEvent.click(screen.getByText("save"))
    await waitFor(() =>
      expect(updateLoopIssueConfig.mock.calls[0][3]).toBe(true)
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
