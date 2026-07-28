import { describe, expect, it } from "vitest"
import {
  MAX_REPEAT_COUNT,
  MIN_REPEAT_COUNT,
  applyRepeatBaseText,
  parseRepeatIntent,
} from "./repeat-intent"
import type { PromptDraft } from "@/lib/types"

describe("parseRepeatIntent", () => {
  it.each([
    ["继续X10", "继续", 10],
    ["继续x10", "继续", 10],
    ["继续 X10", "继续", 10],
    ["继续 x 10", "继续", 10],
    ["continue X 3", "continue", 3],
    ["请继续修复x10", "请继续修复", 10],
    ["fix this\nplease x2", "fix this\nplease", 2],
    ["继续x2   ", "继续", 2],
    ["继续x50", "继续", 50],
  ] as const)("parses %j", (input, baseText, count) => {
    expect(parseRepeatIntent(input)).toEqual({ baseText, count })
  })

  it.each([
    ["继续x1"],
    ["继续x51"],
    ["x10"],
    ["继续X10 请"],
    ["继续X10a"],
    ["继续*10"],
    [""],
    ["   "],
    ["继续"],
  ] as const)("rejects %j", (input) => {
    expect(parseRepeatIntent(input)).toBeNull()
  })

  it("exposes bounds constants", () => {
    expect(MIN_REPEAT_COUNT).toBe(2)
    expect(MAX_REPEAT_COUNT).toBe(50)
  })
})

describe("applyRepeatBaseText", () => {
  it("rewrites displayText and trailing text block only", () => {
    const draft: PromptDraft = {
      displayText: "继续x10",
      blocks: [
        { type: "text", text: "继续x10" },
        {
          type: "image",
          data: "abc",
          mime_type: "image/png",
        },
      ],
    }
    expect(applyRepeatBaseText(draft, "继续")).toEqual({
      displayText: "继续",
      blocks: [
        { type: "text", text: "继续" },
        {
          type: "image",
          data: "abc",
          mime_type: "image/png",
        },
      ],
    })
  })
})
