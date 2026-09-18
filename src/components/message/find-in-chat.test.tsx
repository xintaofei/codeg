import { describe, expect, it } from "vitest"
import { extractFindableText } from "./find-in-chat"
import type { ThreadRenderItem } from "./message-list-view"

function turnItem(
  parts: { type: string; text?: string; [k: string]: unknown }[],
  role: "user" | "assistant" = "assistant"
): ThreadRenderItem {
  return {
    key: `turn-${role}-${Math.random().toString(36).slice(2)}`,
    kind: "turn",
    group: {
      id: "g1",
      role,
      parts: parts as never,
      resources: [],
      images: [],
    },
    phase: "persisted",
    isResponseComplete: true,
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
    isLastAssistantRun: false,
    isThreadTail: false,
    sourceTurns: [],
  } as unknown as ThreadRenderItem
}

describe("extractFindableText", () => {
  it("joins text parts and ignores tool calls and reasoning", () => {
    const item = turnItem([
      { type: "text", text: "first paragraph" },
      { type: "tool-call", toolCallId: "t1", toolName: "bash" },
      { type: "reasoning", content: "thinking hard", isStreaming: false },
      { type: "text", text: "second paragraph" },
    ])
    expect(extractFindableText(item)).toBe("first paragraph\nsecond paragraph")
  })

  it("returns empty string for non-turn items", () => {
    expect(extractFindableText({ key: "typing", kind: "typing" })).toBe("")
    expect(
      extractFindableText({ key: "compaction", kind: "compaction", meta: null })
    ).toBe("")
  })

  it("returns empty string for a turn with no text parts", () => {
    const item = turnItem([{ type: "tool-call", toolCallId: "t1" }])
    expect(extractFindableText(item)).toBe("")
  })
})
