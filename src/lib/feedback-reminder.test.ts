import { describe, expect, it } from "vitest"

import {
  appendFeedbackReminder,
  FEEDBACK_REMINDER_SENTINEL,
  stripFeedbackReminder,
} from "@/lib/feedback-reminder"
import type { PromptInputBlock } from "@/lib/types"

const REMINDER = "Periodically call the check_user_feedback tool."
const MARKED = `${FEEDBACK_REMINDER_SENTINEL} ${REMINDER}`

describe("appendFeedbackReminder", () => {
  it("joins the sentinel-wrapped reminder onto a leading text block", () => {
    const blocks: PromptInputBlock[] = [{ type: "text", text: "fix the bug" }]
    expect(appendFeedbackReminder(blocks, REMINDER)).toEqual([
      { type: "text", text: `fix the bug\n\n${MARKED}` },
    ])
  })

  it("keeps trailing image/resource blocks after the text block", () => {
    const blocks: PromptInputBlock[] = [
      { type: "text", text: "look at this" },
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
    ]
    expect(appendFeedbackReminder(blocks, REMINDER)).toEqual([
      { type: "text", text: `look at this\n\n${MARKED}` },
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
    ])
  })

  it("adds a trailing text block for an attachments-only send (no text)", () => {
    const blocks: PromptInputBlock[] = [
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
    ]
    expect(appendFeedbackReminder(blocks, REMINDER)).toEqual([
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
      { type: "text", text: MARKED },
    ])
  })

  it("joins onto the LAST text block regardless of block order", () => {
    // Not reachable from today's text-first composer, but the helper must not
    // silently depend on ordering: a text block anywhere is found and the
    // reminder lands at the end of the prose, never misplaced.
    const blocks: PromptInputBlock[] = [
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
      { type: "text", text: "see above" },
    ]
    expect(appendFeedbackReminder(blocks, REMINDER)).toEqual([
      { type: "image", data: "abc", mime_type: "image/png", uri: null },
      { type: "text", text: `see above\n\n${MARKED}` },
    ])
  })

  it("returns a single text block when there are no blocks", () => {
    expect(appendFeedbackReminder([], REMINDER)).toEqual([
      { type: "text", text: MARKED },
    ])
  })

  it("does not mutate the input array or its blocks", () => {
    const head: PromptInputBlock = { type: "text", text: "fix the bug" }
    const blocks: PromptInputBlock[] = [head]
    const out = appendFeedbackReminder(blocks, REMINDER)
    expect(out).not.toBe(blocks)
    expect(blocks).toEqual([{ type: "text", text: "fix the bug" }])
    expect(head.text).toBe("fix the bug")
  })
})

describe("stripFeedbackReminder", () => {
  it("removes a sentinel-wrapped reminder and the whitespace that joined it", () => {
    expect(stripFeedbackReminder(`fix the bug\n\n${MARKED}`)).toBe(
      "fix the bug"
    )
  })

  it("round-trips: stripping an appended reminder restores the original text", () => {
    const original = "fix the bug"
    const [block] = appendFeedbackReminder(
      [{ type: "text", text: original }],
      REMINDER
    )
    expect(block.type).toBe("text")
    if (block.type === "text") {
      expect(stripFeedbackReminder(block.text)).toBe(original)
    }
  })

  it("leaves text that never carried a reminder unchanged", () => {
    expect(stripFeedbackReminder("just my prompt")).toBe("just my prompt")
  })

  it("collapses to empty when the text is only the reminder", () => {
    expect(stripFeedbackReminder(MARKED)).toBe("")
  })

  it("only strips from the sentinel onward, keeping the preceding prose", () => {
    expect(stripFeedbackReminder(`line one\nline two\n\n${MARKED}`)).toBe(
      "line one\nline two"
    )
  })
})
