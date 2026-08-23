import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import { CodegMcpToolCard } from "./codeg-mcp-tool-card"
import enMessages from "@/i18n/messages/en.json"

// MessageResponse (Streamdown) drags in the link-safety hook and async Shiki
// highlighting — too heavy for a unit test. Stub it so we can still assert the
// result goes THROUGH the Markdown renderer and arrives intact.
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

/** The MCP CallToolResult envelope the companion's `render_*` helpers emit. */
function envelope(text: string, structured: Record<string, unknown>): string {
  return JSON.stringify({
    content: [{ type: "text", text }],
    isError: false,
    structuredContent: structured,
  })
}

describe("CodegMcpToolCard", () => {
  it("states which session a get_session_info call is reading", () => {
    const { container } = renderWithIntl(
      <CodegMcpToolCard
        tool="get_session_info"
        input={JSON.stringify({ session_id: 2122, max_messages: 20 })}
        state="input-available"
      />
    )
    expect(screen.getByText("Reading session #2122")).toBeInTheDocument()
    // In flight with no result → spinner, and nothing to expand.
    expect(container.querySelector(".animate-spin")).toBeInTheDocument()
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("accepts the stringified session id some hosts send", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="get_session_info"
        input={JSON.stringify({ session_id: "2122" })}
        state="output-available"
      />
    )
    expect(screen.getByText("Reading session #2122")).toBeInTheDocument()
  })

  it("puts the progress message itself in the row", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_progress"
        input={JSON.stringify({ message: "tests passing, starting cleanup" })}
        output={envelope("Recorded.", { recorded: true })}
        state="output-available"
      />
    )
    expect(
      screen.getByText("Progress: tests passing, starting cleanup")
    ).toBeInTheDocument()
  })

  it("leads a task_complete row with the verdict, then the summary", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_complete"
        input={JSON.stringify({
          verdict: "needs_review",
          summary: "works, but check the migration",
        })}
        output={envelope("Recorded.", { recorded: true })}
        state="output-available"
      />
    )
    expect(
      screen.getByText(
        "Task finished · needs review: works, but check the migration"
      )
    ).toBeInTheDocument()
  })

  it("falls back to an unknown verdict rather than dropping the row", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_complete"
        input={JSON.stringify({ verdict: "sorta-done" })}
        state="output-available"
      />
    )
    expect(screen.getByText("Task finished · unknown")).toBeInTheDocument()
  })

  it("names the automation and the work task being created", () => {
    const { unmount } = renderWithIntl(
      <CodegMcpToolCard
        tool="create_automation"
        input={JSON.stringify({ name: "nightly sweep", cron: "7 3 * * *" })}
        state="output-available"
      />
    )
    expect(
      screen.getByText("Creating automation nightly sweep")
    ).toBeInTheDocument()
    unmount()

    renderWithIntl(
      <CodegMcpToolCard
        tool="create_work_task"
        input={JSON.stringify({ title: "fix the flaky test", prompt: "…" })}
        state="output-available"
      />
    )
    expect(
      screen.getByText("Creating work task fix the flaky test")
    ).toBeInTheDocument()
  })

  it("looks past a JSON-RPC {name, arguments} relay envelope", () => {
    // `create_automation`'s own `name` argument collides with the `name` of the
    // `{name, arguments}` envelope some MCP relays wrap a call in, so accepting
    // the outer object would label the card with the TOOL's name. The walker
    // takes the deepest match for exactly this reason.
    renderWithIntl(
      <CodegMcpToolCard
        tool="create_automation"
        input={JSON.stringify({
          name: "mcp__codeg-mcp__create_automation",
          arguments: { name: "nightly sweep", prompt: "sweep the logs" },
        })}
        state="output-available"
      />
    )
    expect(
      screen.getByText("Creating automation nightly sweep")
    ).toBeInTheDocument()
  })

  it("still reads flat arguments, which is what every known host sends", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_complete"
        input={JSON.stringify({ verdict: "success", summary: "shipped" })}
        state="output-available"
      />
    )
    expect(
      screen.getByText("Task finished \u00b7 success: shipped")
    ).toBeInTheDocument()
  })

  it("exposes the untruncated label on hover", () => {
    // The row is CSS-truncated and the panel carries the RESULT, not the
    // arguments — so `title` is the only way to read a long message back.
    const message = "a".repeat(200)
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_progress"
        input={JSON.stringify({ message })}
        state="output-available"
      />
    )
    expect(screen.getByText(`Progress: ${message}`)).toHaveAttribute(
      "title",
      `Progress: ${message}`
    )
  })

  it("surfaces the prompt an authoring call will actually run", () => {
    // `create_automation` / `create_work_task` create something persistent, and
    // the companion's result echoes id/title/folder/schedule but never the
    // prompt — so the panel is the only place it can be audited. The generic
    // shell this card replaced dumped every argument; this keeps parity.
    renderWithIntl(
      <CodegMcpToolCard
        tool="create_work_task"
        input={JSON.stringify({
          title: "fix the flaky test",
          prompt: "Investigate why auth.spec.ts retries, then fix it.",
        })}
        state="input-available"
      />
    )
    // Expandable from frame 1 — the prompt arrives with the arguments, long
    // before any result.
    fireEvent.click(screen.getByRole("button"))
    expect(
      screen.getByText("Investigate why auth.spec.ts retries, then fix it.")
    ).toBeInTheDocument()
  })

  it("shows the prompt above the result once both exist", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="create_automation"
        input={JSON.stringify({ name: "nightly sweep", prompt: "sweep logs" })}
        output={envelope("Created automation #12.", {
          created: true,
          kind: "automation",
          id: 12,
        })}
        state="output-available"
      />
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("sweep logs")).toBeInTheDocument()
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "Created automation #12."
    )
  })

  it("does not show a prompt block for the non-authoring tools", () => {
    // `task_progress` has no `prompt` argument; a stray one must not leak into
    // the panel and must not make an ack-less call look expandable.
    const { container } = renderWithIntl(
      <CodegMcpToolCard
        tool="task_progress"
        input={JSON.stringify({ message: "halfway", prompt: "not mine" })}
        state="input-available"
      />
    )
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
    expect(container.textContent).not.toContain("not mine")
  })

  it("expands to the result text, rendered as Markdown", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="get_session_info"
        input={JSON.stringify({ session_id: 7 })}
        output={envelope("Session #7 (codex)\nTitle: probe", {
          found: true,
          session_id: 7,
        })}
        state="output-available"
      />
    )
    expect(screen.queryByTestId("markdown-response")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "Session #7 (codex) Title: probe"
    )
  })

  it("peels codex-acp's live {result, error} envelope", () => {
    // codex-acp forwards every MCP outcome as `{result: <CallToolResult>,
    // error: null}`; without the peel the whole envelope lands in the card.
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_progress"
        input={JSON.stringify({ message: "halfway" })}
        output={JSON.stringify({
          result: {
            content: [{ type: "text", text: "Recorded." }],
            isError: false,
            structuredContent: { recorded: true },
          },
          error: null,
        })}
        state="output-available"
      />
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "Recorded."
    )
  })

  it("reads Codex's Wall time / Output wrap", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_complete"
        input={JSON.stringify({ verdict: "success", summary: "shipped" })}
        output={
          "Wall time: 0.0031 seconds\nOutput:\n" +
          envelope("Recorded.", { recorded: true })
        }
        state="output-available"
      />
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "Recorded."
    )
  })

  it("shows plain text hosts (claude-agent-acp) verbatim", () => {
    renderWithIntl(
      <CodegMcpToolCard
        tool="task_progress"
        input={JSON.stringify({ message: "halfway" })}
        output="Recorded."
        state="output-available"
      />
    )
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "Recorded."
    )
  })

  it("surfaces a real tool error and tints the card", () => {
    const { container } = renderWithIntl(
      <CodegMcpToolCard
        tool="get_session_info"
        input={JSON.stringify({ session_id: 7 })}
        errorText="socket closed"
        state="output-error"
      />
    )
    expect(container.querySelector(".border-destructive\\/30")).not.toBeNull()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByTestId("markdown-response")).toHaveTextContent(
      "socket closed"
    )
  })

  it("keeps a soft refusal out of the error state", () => {
    // These tools report "couldn't do it, here's why" as ordinary isError:false
    // text — an err badge must mean the CALL failed, not that it was refused.
    const { container } = renderWithIntl(
      <CodegMcpToolCard
        tool="create_automation"
        input={JSON.stringify({ name: "nightly sweep" })}
        output={envelope("Automation creation from chat is turned off.", {
          created: false,
          kind: "automation",
        })}
        state="output-available"
      />
    )
    expect(container.querySelector(".border-destructive\\/30")).toBeNull()
  })

  it("still renders a row when the arguments never arrived", () => {
    renderWithIntl(
      <CodegMcpToolCard tool="task_progress" state="input-streaming" />
    )
    expect(screen.getByText("Reporting task progress")).toBeInTheDocument()
  })
})
