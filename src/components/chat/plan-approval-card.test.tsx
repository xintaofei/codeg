import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

// Exercise the real Streamdown pipeline for the plan markdown; only the
// link-safety hook is stubbed (no bearing on plan rendering), mirroring
// plan-mode-card.test.tsx.
vi.mock("@/components/ai-elements/link-safety", () => ({
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

import { PlanApprovalCard } from "./plan-approval-card"
import enMessages from "@/i18n/messages/en.json"
import type { PendingPlanApprovalState, PlanApprovalAnswer } from "@/lib/types"

function make(plan: string): PendingPlanApprovalState {
  return {
    approval_id: "ap-1",
    tool_call_id: "call-1",
    plan_markdown: plan,
    created_at: "2026-01-01T00:00:00Z",
  }
}

function renderCard(
  approval: PendingPlanApprovalState,
  onAnswer: (
    approvalId: string,
    answer: PlanApprovalAnswer
  ) => void | Promise<void> = vi.fn()
) {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <PlanApprovalCard approval={approval} onAnswer={onAnswer} />
    </NextIntlClientProvider>
  )
  return onAnswer
}

describe("PlanApprovalCard", () => {
  it("renders the plan markdown and the three actions", async () => {
    renderCard(make("# Migration plan\n\n- step one"))
    await waitFor(() =>
      expect(screen.getByText("Migration plan")).toBeInTheDocument()
    )
    expect(
      screen.getByRole("button", { name: "Approve & build" })
    ).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Request changes" })
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Abandon" })).toBeInTheDocument()
  })

  it("approves with an approve decision", () => {
    const onAnswer = renderCard(make("plan"))
    fireEvent.click(screen.getByRole("button", { name: "Approve & build" }))
    expect(onAnswer).toHaveBeenCalledWith("ap-1", {
      decision: "approve",
      feedback: null,
    })
  })

  it("abandons with an abandon decision", () => {
    const onAnswer = renderCard(make("plan"))
    fireEvent.click(screen.getByRole("button", { name: "Abandon" }))
    expect(onAnswer).toHaveBeenCalledWith("ap-1", {
      decision: "abandon",
      feedback: null,
    })
  })

  it("request-changes reveals a textarea and submits the trimmed feedback", () => {
    const onAnswer = renderCard(make("plan"))
    // Not requested yet: no feedback textarea.
    expect(screen.queryByPlaceholderText("What should change?")).toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "Request changes" }))
    const textarea = screen.getByPlaceholderText("What should change?")

    // Send stays disabled until non-empty feedback is typed.
    const send = screen.getByRole("button", { name: "Send" })
    expect(send).toBeDisabled()

    fireEvent.change(textarea, { target: { value: "  use SSE  " } })
    expect(send).not.toBeDisabled()
    fireEvent.click(send)
    expect(onAnswer).toHaveBeenCalledWith("ap-1", {
      decision: "request_changes",
      feedback: "use SSE",
    })
  })

  it("shows the empty-plan notice when no plan was written", () => {
    renderCard(make("   "))
    expect(
      screen.getByText(
        "The agent didn't write a plan. Approve to start building, or request changes."
      )
    ).toBeInTheDocument()
    // Actions still available so the user can approve or push back.
    expect(
      screen.getByRole("button", { name: "Approve & build" })
    ).toBeInTheDocument()
  })
})
