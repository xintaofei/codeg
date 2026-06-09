import { type ReactElement } from "react"
import { render, screen, within } from "@testing-library/react"
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

const result = enMessages.Folder.chat.askQuestionResult

describe("AskQuestionResultCard", () => {
  it("renders the answered single-select choice as a checked, read-only radio", () => {
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
    const chosen = screen.getByRole("radio", { name: /Incremental/ })
    expect(chosen).toBeChecked()
    expect(chosen).toBeDisabled()
    expect(screen.getByRole("radio", { name: /Rewrite/ })).not.toBeChecked()
    // "(Recommended)" is split off into a badge; the chosen description shows.
    expect(screen.getByText("Recommended")).toBeInTheDocument()
    expect(screen.getByText("Smaller steps")).toBeInTheDocument()
    // No footer actions in the read-only record.
    expect(screen.queryByRole("button", { name: "Submit" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Skip" })).toBeNull()
  })

  it("checks the picked options and surfaces a free-text Other answer in multi-select", () => {
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

    expect(screen.getByRole("checkbox", { name: "Alpha" })).toBeChecked()
    expect(screen.getByRole("checkbox", { name: "Beta" })).not.toBeChecked()
    // The label that isn't an option is the free-text "Other" answer.
    expect(screen.getByRole("checkbox", { name: "Other" })).toBeChecked()
    expect(screen.getByDisplayValue("Custom thing")).toBeInTheDocument()
  })

  it("recovers an option label that itself contains a comma", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "Pick",
          header: "Pick",
          multiSelect: true,
          options: [
            { label: "Rewrite, then test", description: "" },
            { label: "Incremental", description: "" },
          ],
        },
      ],
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={"1. [Pick] Pick\n   → Rewrite, then test\n"}
        state="output-available"
      />
    )

    // Naive ", " splitting would have left this unmatched; option-aware
    // matching checks the real option and leaves "Incremental" unchosen.
    expect(
      screen.getByRole("checkbox", { name: "Rewrite, then test" })
    ).toBeChecked()
    expect(
      screen.getByRole("checkbox", { name: "Incremental" })
    ).not.toBeChecked()
  })

  it("shows the dismissed note and checks nothing when declined", () => {
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

    expect(screen.getByText(result.declined)).toBeInTheDocument()
    for (const radio of screen.getAllByRole("radio")) {
      expect(radio).not.toBeChecked()
    }
    expect(screen.queryByRole("button", { name: "Submit" })).toBeNull()
  })

  it("lays multiple questions out as tabs", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "First?",
          header: "First",
          multiSelect: false,
          options: [{ label: "X" }, { label: "Y" }],
        },
        {
          question: "Second?",
          header: "Second",
          multiSelect: false,
          options: [{ label: "P" }, { label: "Q" }],
        },
      ],
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={"1. [First] First?\n   → X\n2. [Second] Second?\n   → Q\n"}
        state="output-available"
      />
    )

    const tabs = screen.getAllByRole("tab")
    expect(tabs).toHaveLength(2)
    expect(within(tabs[0]).getByText("First")).toBeInTheDocument()
    expect(within(tabs[1]).getByText("Second")).toBeInTheDocument()
  })

  it("shows an awaiting state with question chips while in flight", () => {
    renderWithIntl(
      <AskQuestionResultCard input={SINGLE_INPUT} state="input-available" />
    )

    expect(screen.getByText(result.awaiting)).toBeInTheDocument()
    // Compact in-flight view: header chip only, no option controls.
    expect(screen.getByText("Approach")).toBeInTheDocument()
    expect(screen.queryByRole("radio")).toBeNull()
  })
})
