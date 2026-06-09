import { type ReactElement } from "react"
import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { AskQuestionResultCard } from "./ask-question-result-card"
import enMessages from "@/i18n/messages/en.json"

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const SINGLE_INPUT = JSON.stringify({
  questions: [
    {
      question: "Which approach?",
      header: "Approach",
      multiSelect: false,
      options: [
        { label: "Incremental (Recommended)", description: "Smaller steps" },
        { label: "Rewrite", description: "Start fresh" },
      ],
    },
  ],
})

function selectedState(label: string): string | null | undefined {
  return screen
    .getByText(label)
    .closest("[data-selected]")
    ?.getAttribute("data-selected")
}

describe("AskQuestionResultCard", () => {
  it("highlights the chosen option and renders its description + recommended badge", () => {
    renderWithIntl(
      <AskQuestionResultCard
        input={SINGLE_INPUT}
        output={
          "The user answered your question(s):\n" +
          "1. [Approach] Which approach?\n   → Incremental (Recommended)\n"
        }
        state="output-available"
      />
    )

    expect(screen.getByText("Which approach?")).toBeInTheDocument()
    expect(screen.getByText("Smaller steps")).toBeInTheDocument()
    // "(Recommended)" is split off the label into a badge.
    expect(screen.getByText("Recommended")).toBeInTheDocument()
    expect(screen.getByText("Incremental")).toBeInTheDocument()

    expect(selectedState("Incremental")).toBe("true")
    expect(selectedState("Rewrite")).toBe("false")
  })

  it("highlights multiple options and surfaces a free-text Other answer", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "Pick any",
          header: "Pick",
          multiSelect: true,
          options: [
            { label: "Alpha", description: "" },
            { label: "Beta", description: "" },
          ],
        },
      ],
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={"1. [Pick] Pick any\n   → Alpha, Custom thing\n"}
        state="output-available"
      />
    )

    expect(selectedState("Alpha")).toBe("true")
    expect(selectedState("Beta")).toBe("false")
    // The label that isn't one of the options renders as an "Other" answer.
    expect(screen.getByText("Custom thing")).toBeInTheDocument()
    expect(screen.getByText("Other")).toBeInTheDocument()
  })

  it("shows a dismissed note and highlights nothing when declined", () => {
    renderWithIntl(
      <AskQuestionResultCard
        input={SINGLE_INPUT}
        output={
          "The user dismissed the question(s) without choosing an answer. " +
          "Proceed using your best judgment and reasonable defaults."
        }
        state="output-available"
      />
    )

    expect(
      screen.getByText(enMessages.Folder.chat.askQuestionResult.declined)
    ).toBeInTheDocument()
    const rows = screen.getAllByText(/Incremental|Rewrite/)
    expect(rows.length).toBeGreaterThan(0)
    for (const node of screen.getAllByText(/Incremental|Rewrite/)) {
      expect(
        node.closest("[data-selected]")?.getAttribute("data-selected")
      ).toBe("false")
    }
  })

  it("shows an awaiting state with question chips while the call is in flight", () => {
    renderWithIntl(
      <AskQuestionResultCard input={SINGLE_INPUT} state="input-available" />
    )

    expect(
      screen.getByText(enMessages.Folder.chat.askQuestionResult.awaiting)
    ).toBeInTheDocument()
    // The compact in-flight view shows the question's header as a chip and does
    // NOT render the full option list (those belong to the pinned answer card).
    expect(screen.getByText("Approach")).toBeInTheDocument()
    expect(screen.queryByText("Rewrite")).not.toBeInTheDocument()
  })
})
