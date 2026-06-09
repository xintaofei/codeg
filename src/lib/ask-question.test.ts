import { describe, expect, it } from "vitest"

import {
  matchSelections,
  parseAskQuestionInput,
  parseAskQuestionOutcome,
  splitRecommended,
} from "./ask-question"

describe("splitRecommended", () => {
  it("strips a trailing (Recommended) suffix, case-insensitively", () => {
    expect(splitRecommended("Incremental (Recommended)")).toEqual({
      text: "Incremental",
      recommended: true,
    })
    expect(splitRecommended("Incremental (recommended)")).toEqual({
      text: "Incremental",
      recommended: true,
    })
  })

  it("leaves plain labels untouched", () => {
    expect(splitRecommended("Rewrite")).toEqual({
      text: "Rewrite",
      recommended: false,
    })
  })

  it("keeps a bare (Recommended) literal rather than rendering empty", () => {
    expect(splitRecommended("(Recommended)")).toEqual({
      text: "(Recommended)",
      recommended: false,
    })
  })
})

describe("parseAskQuestionInput", () => {
  it("parses questions with options and the camelCase multiSelect field", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "Which approach?",
          header: "Approach",
          multiSelect: false,
          options: [
            { label: "Incremental", description: "Smaller steps" },
            { label: "Rewrite", description: "Start fresh" },
          ],
        },
      ],
    })
    expect(parseAskQuestionInput(input)).toEqual([
      {
        question: "Which approach?",
        header: "Approach",
        multiSelect: false,
        options: [
          { label: "Incremental", description: "Smaller steps" },
          { label: "Rewrite", description: "Start fresh" },
        ],
      },
    ])
  })

  it("also accepts the snake_case multi_select field", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "Pick many",
          header: "Multi",
          multi_select: true,
          options: [],
        },
      ],
    })
    expect(parseAskQuestionInput(input)[0].multiSelect).toBe(true)
  })

  it("tolerates missing options and missing descriptions", () => {
    const input = JSON.stringify({
      questions: [{ question: "Q", header: "H", options: [{ label: "A" }] }],
    })
    expect(parseAskQuestionInput(input)).toEqual([
      {
        question: "Q",
        header: "H",
        multiSelect: false,
        options: [{ label: "A", description: "" }],
      },
    ])
  })

  it("drops options without a label and entries that are entirely empty", () => {
    const input = JSON.stringify({
      questions: [
        {
          question: "Q",
          header: "H",
          options: [{ description: "no label" }, { label: "Keep" }],
        },
        { question: "", header: "", options: [] },
      ],
    })
    const result = parseAskQuestionInput(input)
    expect(result).toHaveLength(1)
    expect(result[0].options).toEqual([{ label: "Keep", description: "" }])
  })

  it("returns [] for malformed JSON, missing questions, or nullish input", () => {
    expect(parseAskQuestionInput("not json")).toEqual([])
    expect(parseAskQuestionInput(JSON.stringify({ foo: 1 }))).toEqual([])
    expect(parseAskQuestionInput(null)).toEqual([])
    expect(parseAskQuestionInput(undefined)).toEqual([])
  })
})

describe("parseAskQuestionOutcome", () => {
  it("returns null when there is no output yet (call in flight)", () => {
    expect(parseAskQuestionOutcome(null)).toBeNull()
    expect(parseAskQuestionOutcome("")).toBeNull()
    expect(parseAskQuestionOutcome("   ")).toBeNull()
  })

  it("detects a declined / dismissed outcome", () => {
    const output =
      "The user dismissed the question(s) without choosing an answer. " +
      "Proceed using your best judgment and reasonable defaults."
    expect(parseAskQuestionOutcome(output)).toEqual({
      declined: true,
      answers: [],
    })
  })

  it("parses answered questions with their selected labels in order", () => {
    const output =
      "The user answered your question(s):\n" +
      "1. [Approach] Which approach?\n" +
      "   → Incremental, Rewrite\n" +
      "2. [Format] Output format?\n" +
      "   → (no selection)\n"
    expect(parseAskQuestionOutcome(output)).toEqual({
      declined: false,
      answers: [
        {
          header: "Approach",
          question: "Which approach?",
          selected: "Incremental, Rewrite",
        },
        { header: "Format", question: "Output format?", selected: "" },
      ],
    })
  })

  it("treats (no selection) as an empty selection", () => {
    const output = "1. [H] Q\n   → (no selection)\n"
    expect(parseAskQuestionOutcome(output)?.answers[0].selected).toBe("")
  })
})

describe("matchSelections", () => {
  it("splits plain selections against the option labels", () => {
    expect(
      matchSelections("Incremental, Rewrite", ["Incremental", "Rewrite"])
    ).toEqual({ selected: ["Incremental", "Rewrite"], other: [] })
  })

  it("recovers an option label that itself contains a comma", () => {
    // Naive ", " splitting would break "Rewrite, then test" into two tokens;
    // matching whole option labels first keeps it intact.
    expect(
      matchSelections("Rewrite, then test, Incremental", [
        "Incremental",
        "Rewrite, then test",
      ])
    ).toEqual({ selected: ["Rewrite, then test", "Incremental"], other: [] })
  })

  it("returns unmatched tokens as free-text Other answers", () => {
    expect(matchSelections("Alpha, Custom thing", ["Alpha", "Beta"])).toEqual({
      selected: ["Alpha"],
      other: ["Custom thing"],
    })
  })

  it("treats empty / (no selection) text as nothing chosen", () => {
    expect(matchSelections("", ["A"])).toEqual({ selected: [], other: [] })
    expect(matchSelections("(no selection)", ["A"])).toEqual({
      selected: [],
      other: [],
    })
  })

  it("with no options, every token is an Other answer", () => {
    expect(matchSelections("foo, bar", [])).toEqual({
      selected: [],
      other: ["foo", "bar"],
    })
  })
})
