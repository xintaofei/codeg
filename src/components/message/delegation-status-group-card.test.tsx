import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import { DelegationStatusGroupCard } from "./delegation-status-group-card"
import type {
  AdaptedToolCallPart,
  ToolCallState,
} from "@/lib/adapters/ai-elements-adapter"
import enMessages from "@/i18n/messages/en.json"

// Same rationale as delegation-status-card.test.tsx: stub the heavy Markdown
// renderer to a sentinel that echoes its source.
vi.mock("@/components/ai-elements/message", () => ({
  MessageResponse: ({ children }: { children: string }) => (
    <div data-testid="markdown-response">{children}</div>
  ),
}))

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function envelope(report: Record<string, unknown>, isError = false): string {
  const text =
    report.status === "completed"
      ? ((report.text ?? report.message ?? "") as string)
      : ((report.message ?? report.text ?? "") as string)
  return JSON.stringify({
    content: [{ type: "text", text }],
    isError,
    structuredContent: report,
  })
}

let seq = 0
function poll(
  taskId: string,
  opts: {
    output?: string
    state?: ToolCallState
    /** Override the call arguments (pass `null` to simulate a lost input). */
    input?: string | null
  } = {}
): AdaptedToolCallPart {
  return {
    type: "tool-call",
    toolCallId: `poll-${taskId}-${seq++}`,
    toolName: "get_delegation_status",
    input:
      opts.input !== undefined
        ? opts.input
        : JSON.stringify({ task_id: taskId }),
    state: opts.state ?? "output-available",
    output: opts.output ?? null,
  }
}

describe("DelegationStatusGroupCard", () => {
  it("collapses N polls of one task into a single row with its final outcome", () => {
    renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("abc12345", {
            output: envelope({ task_id: "abc12345", status: "running" }),
          }),
          poll("abc12345", {
            output: envelope({ task_id: "abc12345", status: "running" }),
          }),
          poll("abc12345", {
            output: envelope({
              task_id: "abc12345",
              status: "completed",
              text: "All tests pass.",
            }),
          }),
        ]}
      />
    )
    // One row only — the interim "running" snapshots are subsumed.
    expect(
      screen.getAllByText("Waiting for task #abc12345 result")
    ).toHaveLength(1)
    expect(screen.getByText("done")).toBeInTheDocument()
    // Poll-count hint reflects the collapsed run.
    expect(screen.getByText("×3")).toBeInTheDocument()
    // Latest poll's result is revealed on expand.
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("All tests pass.")).toBeInTheDocument()
  })

  it("shows the neutral 'checked' badge (no spinner) when the latest poll returned still-running", () => {
    const { container } = renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("abc12345", {
            output: envelope({
              task_id: "abc12345",
              status: "running",
              message: "still working",
            }),
          }),
        ]}
      />
    )
    expect(screen.getByText("checked")).toBeInTheDocument()
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
  })

  it("keeps the live spinner for a poll still in flight", () => {
    const { container } = renderWithIntl(
      <DelegationStatusGroupCard
        polls={[poll("abc12345", { state: "input-available" })]}
      />
    )
    expect(container.querySelector(".animate-spin")).toBeInTheDocument()
  })

  it("renders one row per task for parallel waits", () => {
    renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("aaaa1111", {
            output: envelope({ task_id: "aaaa1111", status: "running" }),
          }),
          poll("bbbb2222", {
            output: envelope({ task_id: "bbbb2222", status: "running" }),
          }),
          poll("aaaa1111", {
            output: envelope({
              task_id: "aaaa1111",
              status: "completed",
              text: "A done",
            }),
          }),
          poll("bbbb2222", {
            output: envelope({
              task_id: "bbbb2222",
              status: "completed",
              text: "B done",
            }),
          }),
        ]}
      />
    )
    expect(
      screen.getByText("Waiting for task #aaaa1111 result")
    ).toBeInTheDocument()
    expect(
      screen.getByText("Waiting for task #bbbb2222 result")
    ).toBeInTheDocument()
    expect(screen.getAllByText("done")).toHaveLength(2)
  })

  it("shows the neutral 'checked' badge for a content-only returned-running poll", () => {
    // Historical reload: only the backend running sentinel survives (no
    // structuredContent). It must not read as a false 'done'.
    const { container } = renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("abc12345", {
            output: "Sub-agent is still running in the background.",
          }),
        ]}
      />
    )
    expect(screen.getByText("checked")).toBeInTheDocument()
    expect(screen.queryByText("done")).not.toBeInTheDocument()
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
  })

  it("keeps polls separate by their output task_id when the input lost the id", () => {
    // The call arguments carry no task_id, but each output's structured report
    // does — distinct tasks must NOT collapse into one row where the latest
    // hides the others.
    renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("", {
            input: null,
            output: envelope({
              task_id: "aaaa1111",
              status: "completed",
              text: "A done",
            }),
          }),
          poll("", {
            input: null,
            output: envelope(
              { task_id: "bbbb2222", status: "failed", error_code: "timeout" },
              true
            ),
          }),
        ]}
      />
    )
    expect(
      screen.getByText("Waiting for task #aaaa1111 result")
    ).toBeInTheDocument()
    expect(
      screen.getByText("Waiting for task #bbbb2222 result")
    ).toBeInTheDocument()
    expect(screen.getByText("done")).toBeInTheDocument()
  })

  it("does not collapse unattributable polls (no task_id anywhere) into one row", () => {
    // Neither input nor output yields a task_id — keep each poll as its own row
    // rather than letting the latest hide the earlier interim notes.
    renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("", { input: null, output: "first interim note" }),
          poll("", { input: null, output: "second interim note" }),
        ]}
      />
    )
    expect(screen.getAllByText("Waiting for task result")).toHaveLength(2)
  })

  it("tints the card destructive when the only task failed", () => {
    renderWithIntl(
      <DelegationStatusGroupCard
        polls={[
          poll("abc12345", {
            state: "output-error",
            output: envelope(
              { task_id: "abc12345", status: "failed", error_code: "timeout" },
              true
            ),
          }),
        ]}
      />
    )
    expect(screen.getByTestId("delegation-status-group")).toHaveClass(
      "bg-destructive/5"
    )
  })
})
