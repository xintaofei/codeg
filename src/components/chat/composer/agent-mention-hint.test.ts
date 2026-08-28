import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const { listAgentsMock, getSettingsMock } = vi.hoisted(() => ({
  listAgentsMock: vi.fn(),
  getSettingsMock: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  acpListAgents: (...args: unknown[]) => listAgentsMock(...args),
  getDelegationSettings: (...args: unknown[]) => getSettingsMock(...args),
}))

import {
  extractMentionedAgentTypes,
  mentionedAgentTypesFromBlocks,
} from "./agent-mention-hint"

describe("extractMentionedAgentTypes", () => {
  it("returns empty for text without agent mentions", () => {
    expect(
      extractMentionedAgentTypes("plain @text and [a link](file://x)")
    ).toEqual([])
  })

  it("does not fire on free-standing @label prose without a routing uri", () => {
    expect(extractMentionedAgentTypes("ask @codex about it")).toEqual([])
  })

  it("extracts the agent_type from a serialized mention", () => {
    expect(
      extractMentionedAgentTypes("请 [@Codex](codeg://agent/codex) 看看")
    ).toEqual(["codex"])
  })

  it("deduplicates repeat mentions and keeps order of first appearance", () => {
    const text = [
      "[@Codex](codeg://agent/codex)",
      "[@Antigravity](codeg://agent/antigravity)",
      "[@Codex again](codeg://agent/codex)",
    ].join(" then ")
    expect(extractMentionedAgentTypes(text)).toEqual(["codex", "antigravity"])
  })

  it("matches the wire form embedded in larger prose", () => {
    expect(
      extractMentionedAgentTypes(
        "see [@Claude](codeg://agent/claude_code) and [@pi](codeg://agent/pi)"
      )
    ).toEqual(["claude_code", "pi"])
  })

  it("scans only text blocks when reading prompt blocks", () => {
    expect(
      mentionedAgentTypesFromBlocks([
        { type: "text", text: "check [@Codex](codeg://agent/codex)" },
        { type: "image", data: "codeg://agent/fake", mime_type: "image/png" },
        { type: "resource", uri: "codeg://agent/also-fake" },
      ])
    ).toEqual(["codex"])
  })
})

describe("findBlockedAgentMentions", () => {
  // Dynamic import per test: the module keeps a TTL cache in module state, and
  // vi.resetModules() only affects imports resolved AFTER the reset — so each
  // test needs its own fresh module instance to see its own mocked backend.
  let findBlockedAgentMentions: typeof import("./agent-mention-hint").findBlockedAgentMentions

  beforeEach(async () => {
    vi.resetModules()
    listAgentsMock.mockResolvedValue([
      { agent_type: "codex", name: "Codex", enabled: true },
      { agent_type: "qoder", name: "Qoder", enabled: false },
    ])
    getSettingsMock.mockResolvedValue({ enabled: true })
    ;({ findBlockedAgentMentions } = await import("./agent-mention-hint"))
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it("short-circuits without mentions and never calls the backend", async () => {
    const blocked = await findBlockedAgentMentions([])
    expect(blocked).toEqual({ delegationOff: false, disabledAgents: [] })
    expect(getSettingsMock).not.toHaveBeenCalled()
    expect(listAgentsMock).not.toHaveBeenCalled()
  })

  it("reports delegationOff alone when multi-agent delegation is disabled", async () => {
    getSettingsMock.mockResolvedValue({ enabled: false })
    const blocked = await findBlockedAgentMentions(["qoder", "codex"])
    expect(blocked.delegationOff).toBe(true)
    expect(blocked.disabledAgents).toEqual([])
  })

  it("lists only the mentioned agents that are disabled", async () => {
    const blocked = await findBlockedAgentMentions(["qoder", "codex"])
    expect(blocked).toEqual({
      delegationOff: false,
      disabledAgents: [{ type: "qoder", label: "Qoder" }],
    })
  })

  it("returns nothing when every mentioned agent is enabled", async () => {
    const blocked = await findBlockedAgentMentions(["codex"])
    expect(blocked).toEqual({ delegationOff: false, disabledAgents: [] })
  })
})
