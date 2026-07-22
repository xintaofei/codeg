import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import { DelegationStatusCard } from "./delegation-status-card"
import enMessages from "@/i18n/messages/en.json"

// MessageResponse (Streamdown) pulls in the link-safety hook (workspace
// context + toasts) and async Shiki highlighting — too heavy for a unit test,
// and its Markdown correctness is covered by its own lib. Stub it to a sentinel
// that echoes its source so we can assert the card routes the result THROUGH
// the Markdown renderer (rather than a raw <pre>), and that the text reaches it.
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

// The MCP CallToolResult envelope the companion emits (render_task_report):
// content[0].text mirrors the result text for completed / the message otherwise.
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

describe("DelegationStatusCard", () => {
  it("shows a single-line waiting label + spinner while the poll is in flight", () => {
    const { container } = renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345", wait_ms: 5000 })}
        state="input-available"
      />
    )
    // No "Delegation status" title, no running/done badge — just the intent.
    expect(
      screen.getByText("Waiting for task #abc12345 result")
    ).toBeInTheDocument()
    // In flight with no result yet → a spinner, and nothing to expand.
    expect(container.querySelector(".animate-spin")).toBeInTheDocument()
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("uses the cancel label for the cancel tool", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="cancel"
        input={JSON.stringify({ task_id: "abc12345" })}
        state="input-available"
      />
    )
    expect(screen.getByText("Canceling task #abc12345")).toBeInTheDocument()
  })

  it("falls back to a task-less label when the task_id can't be parsed", () => {
    // A settled poll that returned a note but no resolvable id still shows the
    // task-less label (there is content to surface).
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        output="Interim note from the sub-agent."
        state="output-available"
      />
    )
    expect(screen.getByText("Waiting for task result")).toBeInTheDocument()
  })

  it("suppresses a still-in-flight poll whose task_id hasn't streamed in yet", () => {
    // claude-agent-acp ships an arg-less initial tool_call (rawInput not yet on
    // the wire), so a live in-flight poll carries no identity and nothing to
    // show. It must NOT render as an anonymous "waiting for a task's result"
    // row (that stacked into visible duplicates); it appears once its id or a
    // report resolves.
    const { container } = renderWithIntl(
      <DelegationStatusCard kind="status" state="input-available" />
    )
    expect(container.firstChild).toBeNull()
    expect(
      screen.queryByText("Waiting for task result")
    ).not.toBeInTheDocument()
  })

  it("expands a plain streamed result inline (no child-session button)", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output="The sub-agent finished the migration."
        state="output-available"
      />
    )
    // Result hidden until expanded; opens inline, not via a session dialog.
    expect(
      screen.queryByText("The sub-agent finished the migration.")
    ).not.toBeInTheDocument()
    fireEvent.click(
      screen.getByRole("button", {
        name: /Waiting for task #abc12345 result/,
      })
    )
    expect(
      screen.getByText("The sub-agent finished the migration.")
    ).toBeInTheDocument()
  })

  it("reveals a completed structuredContent result on expand", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "completed",
          child_conversation_id: 42,
          text: "All tests pass.",
        })}
        state="output-available"
      />
    )
    expect(screen.queryByText("All tests pass.")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("All tests pass.")).toBeInTheDocument()
  })

  it("tints the card destructive for a failed status and shows the message on expand", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope(
          {
            task_id: "abc12345",
            status: "failed",
            error_code: "timeout",
            message: "timed out",
          },
          true
        )}
        state="output-available"
      />
    )
    expect(screen.getByTestId("delegation-status-card")).toHaveClass(
      "bg-destructive/5",
      // Destructive card still gets the workspace-bg frosted surface.
      "ws-msg-card"
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("timed out")).toBeInTheDocument()
  })

  it("treats a canceled task as success (no destructive tint) for the cancel action", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="cancel"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "canceled",
          error_code: "canceled",
          message: "Task canceled.",
        })}
        state="output-available"
      />
    )
    expect(screen.getByText("Canceling task #abc12345")).toBeInTheDocument()
    expect(screen.getByTestId("delegation-status-card")).not.toHaveClass(
      "bg-destructive/5"
    )
  })

  it("treats a canceled task as a terminal error for a status query", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({ task_id: "abc12345", status: "canceled" })}
        state="output-available"
      />
    )
    expect(screen.getByTestId("delegation-status-card")).toHaveClass(
      "bg-destructive/5"
    )
  })

  it("treats an unknown task id as terminal error, not an endless spinner", () => {
    const { container } = renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "deadbeef" })}
        output={envelope({ task_id: "deadbeef", status: "unknown" })}
        state="output-available"
      />
    )
    expect(screen.getByTestId("delegation-status-card")).toHaveClass(
      "bg-destructive/5"
    )
    // Terminal, with no message → no spinner and nothing to expand.
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("recovers a Codex-wrapped report (Wall time prefix) and keeps unknown terminal", () => {
    const report = JSON.stringify({ task_id: "abc12345", status: "unknown" })
    const wrapped = `Wall time: 1 seconds\nOutput:\n${report}_`
    const { container } = renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={wrapped}
        state="output-available"
      />
    )
    // A plain-text fallback would have rendered a non-error row — must not.
    expect(screen.getByTestId("delegation-status-card")).toHaveClass(
      "bg-destructive/5"
    )
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
  })

  it("recovers a failed report inlined in content[0].text without structuredContent", () => {
    const report = JSON.stringify({
      task_id: "abc12345",
      status: "failed",
      message: "boom",
    })
    const output = JSON.stringify({
      content: [{ type: "text", text: report }],
      isError: true,
    })
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={output}
        state="output-available"
      />
    )
    expect(screen.getByTestId("delegation-status-card")).toHaveClass(
      "bg-destructive/5"
    )
    fireEvent.click(screen.getByRole("button"))
    // Shows the report's own message, not the raw inlined JSON.
    expect(screen.getByText("boom")).toBeInTheDocument()
  })

  it("settles a returned running poll to a neutral state (no live spinner), still expandable", () => {
    // A poll that RETURNED "still running" is a stale snapshot, not live work:
    // it must not keep spinning, but its interim message is still revealable.
    const { container } = renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "running",
          message: "still working",
        })}
        state="output-available"
      />
    )
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
    expect(screen.getByText("checked")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("still working")).toBeInTheDocument()
  })

  it("settles a content-only returned-running poll (no structuredContent) to checked, not a false done", () => {
    // Historical Claude reload keeps only content[0].text — the backend running
    // sentinel sentence. Without structured status it must still read as the
    // neutral 'checked' (no spinner), never 'done'.
    const { container } = renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output="Sub-agent is still running in the background."
        state="output-available"
      />
    )
    expect(container.querySelector(".animate-spin")).not.toBeInTheDocument()
    expect(screen.getByText("checked")).toBeInTheDocument()
    expect(screen.queryByText("done")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(
      screen.getByText("Sub-agent is still running in the background.")
    ).toBeInTheDocument()
  })

  it("parses a double-encoded task_id input", () => {
    const input = JSON.stringify(JSON.stringify({ task_id: "abc12345" }))
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={input}
        state="input-available"
      />
    )
    expect(
      screen.getByText("Waiting for task #abc12345 result")
    ).toBeInTheDocument()
  })

  it("parses a task_id nested in a JSON-string wrapper field", () => {
    const input = JSON.stringify({
      arguments: JSON.stringify({ task_id: "abc12345" }),
    })
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={input}
        state="input-available"
      />
    )
    expect(
      screen.getByText("Waiting for task #abc12345 result")
    ).toBeInTheDocument()
  })

  it("shows the execution time and a status badge on the title line for a completed result", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "completed",
          text: "Done.",
          duration_ms: 1234,
        })}
        state="output-available"
      />
    )
    expect(screen.getByText("1.2s")).toBeInTheDocument()
    // StatusBadge for a completed task (en: status.ok = "done").
    expect(screen.getByText("done")).toBeInTheDocument()
  })

  it("renders the expanded result through the Markdown renderer (not a <pre>)", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "completed",
          text: "# Heading\n\n- one\n- two",
        })}
        state="output-available"
      />
    )
    fireEvent.click(screen.getByRole("button"))
    const md = screen.getByTestId("markdown-response")
    expect(md).toHaveTextContent("# Heading")
    expect(document.querySelector("pre")).toBeNull()
  })

  it("does not mistake a child's own JSON-with-status output for a delegation report (live path)", () => {
    // Live wire forwards only the child's result text. A child that returns
    // JSON carrying a `status` field must NOT be read as a (failed) report —
    // there's no task_id marker, so it's plain output shown verbatim.
    const childOutput = JSON.stringify({
      status: "failed",
      message: "child said so",
    })
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={childOutput}
        state="output-available"
      />
    )
    expect(screen.getByTestId("delegation-status-card")).not.toHaveClass(
      "bg-destructive/5"
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "child said so"
    )
  })

  it("links the disclosure button to its panel via aria-controls when expanded", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output="The sub-agent finished the migration."
        state="output-available"
      />
    )
    const button = screen.getByRole("button")
    // Collapsed: no dangling reference (the panel isn't mounted yet).
    expect(button).toHaveAttribute("aria-expanded", "false")
    expect(button).not.toHaveAttribute("aria-controls")
    fireEvent.click(button)
    // Expanded: aria-controls points at the now-mounted panel's id.
    expect(button).toHaveAttribute("aria-expanded", "true")
    const panelId = button.getAttribute("aria-controls")
    expect(panelId).toBeTruthy()
    const panel = document.getElementById(panelId as string)
    expect(panel).not.toBeNull()
    expect(panel).toHaveTextContent("The sub-agent finished the migration.")
  })

  it("formats a near-minute duration without rolling over to 60 seconds", () => {
    renderWithIntl(
      <DelegationStatusCard
        kind="status"
        input={JSON.stringify({ task_id: "abc12345" })}
        output={envelope({
          task_id: "abc12345",
          status: "completed",
          text: "ok",
          duration_ms: 119999,
        })}
        state="output-available"
      />
    )
    expect(screen.getByText("2m 0s")).toBeInTheDocument()
    expect(screen.queryByText("1m 60s")).not.toBeInTheDocument()
  })
})
