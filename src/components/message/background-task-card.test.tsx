import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { BackgroundTaskCard } from "./background-task-card"
import type { AdaptedToolCallPart } from "@/lib/adapters/ai-elements-adapter"
import enMessages from "@/i18n/messages/en.json"

function renderCard(polls: AdaptedToolCallPart[]) {
  const ui: ReactElement = <BackgroundTaskCard polls={polls} />
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function poll(over: Partial<AdaptedToolCallPart> = {}): AdaptedToolCallPart {
  return {
    type: "tool-call",
    toolCallId: "c1",
    toolName: "TaskOutput",
    input: JSON.stringify({ task_id: "bfb5xnq1t", block: true, timeout: 1000 }),
    output: null,
    state: "output-available",
    ...over,
  }
}

const RUNNING = `<retrieval_status>timeout</retrieval_status>\n<task_id>bfb5xnq1t</task_id>\n<task_type>local_bash</task_type>\n<status>running</status>`
const COMPLETED = `<retrieval_status>success</retrieval_status>\n<task_id>bfb5xnq1t</task_id>\n<task_type>local_bash</task_type>\n<status>completed</status>\n<exit_code>0</exit_code>\n<output>BUILD OK MARKER</output>`
const FAILED = `<retrieval_status>success</retrieval_status>\n<task_id>bfb5xnq1t</task_id>\n<task_type>local_bash</task_type>\n<status>completed</status>\n<exit_code>1</exit_code>\n<output>boom</output>`

describe("BackgroundTaskCard", () => {
  it("merges repeated polls into one row with the final status", () => {
    renderCard([
      poll({ toolCallId: "p1", output: RUNNING }),
      poll({ toolCallId: "p2", output: COMPLETED }),
    ])
    // One row, not two cards.
    expect(screen.getAllByTestId("background-task-group")).toHaveLength(1)
    expect(screen.getByText("Completed")).toBeInTheDocument()
    expect(screen.getByText("Background task · bfb5xnq1t")).toBeInTheDocument()
    // ×N poll-count hint.
    expect(screen.getByText("×2")).toBeInTheDocument()
  })

  it("reveals the ANSI output through the terminal on expand", () => {
    renderCard([poll({ output: COMPLETED })])
    expect(screen.queryByText("BUILD OK MARKER")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("BUILD OK MARKER")).toBeInTheDocument()
  })

  it("shows a failed badge + exit code for a non-zero exit", () => {
    renderCard([poll({ output: FAILED })])
    expect(screen.getByText("Failed")).toBeInTheDocument()
    expect(screen.getByText("exit 1")).toBeInTheDocument()
    // The all-failed (destructive) card still carries the workspace-bg frosted
    // surface class so it matches its non-error siblings over a background image.
    expect(screen.getByTestId("background-task-group")).toHaveClass(
      "ws-msg-card"
    )
  })

  it("shows a running badge for an in-flight poll", () => {
    renderCard([poll({ output: null, state: "input-available" })])
    expect(screen.getByText("Running")).toBeInTheDocument()
  })
})
