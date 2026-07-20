import { type ReactElement } from "react"
import { fireEvent, render, screen, within } from "@testing-library/react"
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

/** Expand the collapsed capsule so the read-only card is in the DOM. */
function expand() {
  fireEvent.click(screen.getByTestId("ask-question-result-card"))
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

// The real on-disk tool result: the structured envelope, `selected` an array.
const SINGLE_OUTPUT = JSON.stringify({
  answers: [
    {
      header: "Approach",
      question: "Which approach?",
      selected: ["Incremental (Recommended)"],
    },
  ],
  declined: false,
})

const result = enMessages.Folder.chat.askQuestionResult

describe("AskQuestionResultCard", () => {
  it("is collapsed by default into a capsule summarizing the picks", () => {
    renderWithIntl(
      <AskQuestionResultCard
        input={SINGLE_INPUT}
        output={SINGLE_OUTPUT}
        state="output-available"
      />
    )

    // Capsule shows the localized label + the chosen value; the full option
    // controls stay hidden.
    expect(screen.getByText(result.answeredLabel)).toBeInTheDocument()
    expect(screen.getByText("Incremental (Recommended)")).toBeInTheDocument()
    expect(screen.queryByRole("radio")).toBeNull()
    expect(screen.queryByText("Which approach?")).toBeNull()
  })

  it("expands to the read-only card, with the choice checked and disabled", () => {
    renderWithIntl(
      <AskQuestionResultCard
        input={SINGLE_INPUT}
        output={SINGLE_OUTPUT}
        state="output-available"
      />
    )
    expand()

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
    const output = JSON.stringify({
      answers: [
        {
          header: "Pick",
          question: "Pick any",
          selected: ["Alpha", "Custom thing"],
        },
      ],
      declined: false,
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={output}
        state="output-available"
      />
    )
    expand()

    expect(screen.getByRole("checkbox", { name: "Alpha" })).toBeChecked()
    expect(screen.getByRole("checkbox", { name: "Beta" })).not.toBeChecked()
    // The pick that isn't an option is the free-text "Other" answer.
    expect(screen.getByRole("checkbox", { name: "Other" })).toBeChecked()
    expect(screen.getByDisplayValue("Custom thing")).toBeInTheDocument()
  })

  it("matches an option label that itself contains a comma", () => {
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
    const output = JSON.stringify({
      answers: [
        { header: "Pick", question: "Pick", selected: ["Rewrite, then test"] },
      ],
      declined: false,
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={output}
        state="output-available"
      />
    )
    expand()

    // The pick is one whole array entry, so the comma is no obstacle: the real
    // option is checked and "Incremental" stays unchosen.
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
        output={JSON.stringify({ answers: [], declined: true })}
        state="output-available"
      />
    )

    // Collapsed capsule carries the dismissed note.
    expect(screen.getByText(result.declined)).toBeInTheDocument()
    expand()
    for (const radio of screen.getAllByRole("radio")) {
      expect(radio).not.toBeChecked()
    }
    expect(screen.queryByRole("button", { name: "Submit" })).toBeNull()
  })

  it("lays multiple questions out as tabs once expanded", () => {
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
    const output = JSON.stringify({
      answers: [
        { header: "First", question: "First?", selected: ["X"] },
        { header: "Second", question: "Second?", selected: ["Q"] },
      ],
      declined: false,
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={output}
        state="output-available"
      />
    )
    expand()

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

  it("matches grok's header-less questions to their empty-header answers", () => {
    // Grok's native ask carries no `header`; the connection bridge (live) and the
    // history parser both emit header-less questions + answers with `header: ""`.
    // The answer↔question match key (header + question) must still align so the
    // capsule shows the pick, not the "no selection" fallback. Regression guard
    // for the in-stream grok card (both the live and reloaded paths).
    const input = JSON.stringify({
      questions: [
        {
          question: "你更喜欢哪种演示方式？",
          multiSelect: false,
          options: [
            { label: "单选示例", description: "" },
            { label: "多选示例", description: "" },
            { label: "随便看看", description: "" },
          ],
        },
      ],
    })
    const output = JSON.stringify({
      answers: [
        {
          header: "",
          question: "你更喜欢哪种演示方式？",
          selected: ["随便看看"],
        },
      ],
      declined: false,
    })
    renderWithIntl(
      <AskQuestionResultCard
        input={input}
        output={output}
        state="output-available"
      />
    )

    // Collapsed capsule shows the pick — NOT the noSelection fallback.
    expect(screen.getByText("随便看看")).toBeInTheDocument()
    expect(screen.queryByText(result.noSelection)).toBeNull()
    // Expanded: the question renders and the chosen option is checked + disabled.
    expand()
    const chosen = screen.getByRole("radio", { name: "随便看看" })
    expect(chosen).toBeChecked()
    expect(chosen).toBeDisabled()
  })
})
