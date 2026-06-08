import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { GoalRunPart, GoalToolCallPart } from "./goal-tool-call"
import enMessages from "@/i18n/messages/en.json"
import zhMessages from "@/i18n/messages/zh-CN.json"

function renderWithIntl(
  ui: ReactElement,
  messages = enMessages,
  locale: "en" | "zh-CN" = "en"
) {
  return render(
    <NextIntlClientProvider locale={locale} messages={messages}>
      {ui}
    </NextIntlClientProvider>
  )
}

describe("GoalToolCallPart", () => {
  it("renders Codex goal completion as a compact goal card", () => {
    renderWithIntl(
      <GoalToolCallPart
        part={{
          type: "tool-call",
          toolCallId: "call-goal",
          toolName: "update_goal",
          input: JSON.stringify({ status: "complete" }),
          state: "output-available",
          output: JSON.stringify({
            goal: {
              objective: "Analyze README file",
              status: "complete",
              tokensUsed: 5184,
              timeUsedSeconds: 19,
            },
            // Real codex output carries an internal LLM instruction here, not a
            // user-facing report. It must never be rendered in the card.
            completionBudgetReport:
              "Goal achieved. Report final usage from this tool result's structured goal fields.",
          }),
        }}
      />
    )

    expect(screen.getByText("Goal: Analyze README file")).toBeInTheDocument()
    expect(screen.queryByText("Goal complete")).not.toBeInTheDocument()
    expect(screen.getByText("5.2K tokens")).toBeInTheDocument()
    expect(screen.getByText("19s")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button"))

    expect(screen.getByText("Objective")).toBeInTheDocument()
    // The internal completionBudgetReport instruction must not leak to the UI.
    expect(screen.queryByText(/Report final usage/)).not.toBeInTheDocument()
  })

  it("wraps in-progress goal process content and shimmers the running title", () => {
    renderWithIntl(
      <GoalRunPart
        part={{
          type: "goal-run",
          start: {
            type: "tool-call",
            toolCallId: "call-create-goal",
            toolName: "create_goal",
            input: JSON.stringify({ objective: "Analyze README file" }),
            state: "output-available",
          },
          end: null,
          items: [{ type: "text", text: "Reading README.md" }],
          isRunning: true,
        }}
        renderPart={(part, key) =>
          part.type === "text" ? <div key={key}>{part.text}</div> : null
        }
      />
    )

    const button = screen.getByRole("button")
    const runningTitle = screen.getByText("Goal: Analyze README file")
    expect(runningTitle).toHaveClass("text-transparent")
    expect(screen.queryByText("Goal active")).not.toBeInTheDocument()
    expect(button.querySelectorAll("svg")).toHaveLength(1)
    expect(screen.queryByText("Reading README.md")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button"))

    expect(screen.getByText("Reading README.md")).toBeInTheDocument()
  })

  it("shows active status for wrapper-prefixed create_goal names", () => {
    renderWithIntl(
      <GoalRunPart
        part={{
          type: "goal-run",
          start: {
            type: "tool-call",
            toolCallId: "call-create-goal",
            toolName: "functions.create_goal",
            input: JSON.stringify({ objective: "Analyze README file" }),
            state: "output-available",
          },
          end: null,
          items: [],
          isRunning: true,
        }}
        renderPart={() => null}
      />
    )

    expect(screen.getByText("Goal: Analyze README file")).toBeInTheDocument()
    expect(screen.queryByText("Goal active")).not.toBeInTheDocument()
  })

  it("localizes the title label", () => {
    renderWithIntl(
      <GoalRunPart
        part={{
          type: "goal-run",
          start: {
            type: "tool-call",
            toolCallId: "call-create-goal",
            toolName: "create_goal",
            input: JSON.stringify({ objective: "分析 README 文件" }),
            state: "output-available",
          },
          end: null,
          items: [],
          isRunning: true,
        }}
        renderPart={() => null}
      />,
      zhMessages,
      "zh-CN"
    )

    expect(screen.getByText("目标：分析 README 文件")).toBeInTheDocument()
  })
})
