import { describe, expect, it } from "vitest"
import { buildJudgePrompt, mapPermissionToAgentMode } from "./use-pk-round"

describe("mapPermissionToAgentMode", () => {
  it("never maps full auto to Claude's deny-without-asking mode", () => {
    expect(
      mapPermissionToAgentMode("bypassPermissions", [
        "default",
        "acceptEdits",
        "dontAsk",
        "auto",
      ])
    ).toBe("auto")
  })
})

describe("buildJudgePrompt", () => {
  it("requires judge prose to follow the current interface locale", () => {
    const [block] = buildJudgePrompt(
      "实现一个页面",
      [{ slot: 2, agentType: "qoder", label: "Qwen3.8-Max", diff: "+hello" }],
      null,
      "zh-CN"
    )

    expect(block.type).toBe("text")
    if (block.type === "text") {
      expect(block.text).toContain("locale zh-CN")
      expect(block.text).toContain("Keep JSON property names unchanged")
      expect(block.text).toContain("Contestant slot 2: qoder · Qwen3.8-Max")
      expect(block.text).toContain('"slot":<number>')
    }
  })
})
