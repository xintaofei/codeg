import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  getAgentColor,
  getAgentIconUrl,
  getAgentInitial,
  getAgentLabel,
  getCustomAgentDisplayVersion,
  setCustomAgentDisplay,
  subscribeCustomAgentDisplay,
} from "@/lib/custom-agents"

beforeEach(() => {
  setCustomAgentDisplay([])
})

describe("setCustomAgentDisplay", () => {
  it("publishes names and icons, and drops agents that are gone", () => {
    setCustomAgentDisplay([
      {
        agentType: "custom:goose",
        name: "goose",
        iconUrl: "data:image/svg+xml;base64,PHN2Zy8+",
      },
      { agentType: "custom:qwen-code", name: "Qwen Code", iconUrl: null },
    ])
    expect(getAgentLabel("custom:goose")).toBe("goose")
    expect(getAgentIconUrl("custom:goose")).toBe(
      "data:image/svg+xml;base64,PHN2Zy8+"
    )
    // Registered but iconless — the caller falls back to the initial glyph.
    expect(getAgentIconUrl("custom:qwen-code")).toBeNull()
    expect(getAgentInitial("custom:qwen-code")).toBe("Q")

    // A later publish replaces the whole set rather than merging into it.
    setCustomAgentDisplay([{ agentType: "custom:qwen-code", name: "Qwen" }])
    expect(getAgentIconUrl("custom:goose")).toBeNull()
    expect(getAgentLabel("custom:goose")).toBe("Goose")
  })

  it("leaves built-ins alone", () => {
    setCustomAgentDisplay([
      { agentType: "custom:goose", name: "goose", iconUrl: "data:image/png;," },
    ])
    // Built-ins ship compiled-in marks; an icon URL would shadow them.
    expect(getAgentIconUrl("claude_code")).toBeNull()
    expect(getAgentLabel("claude_code")).not.toBe("goose")
    expect(getAgentColor("claude_code")).toBeTruthy()
  })

  it("only notifies subscribers when what they render actually changed", () => {
    const listener = vi.fn()
    const unsubscribe = subscribeCustomAgentDisplay(listener)
    const entries = [
      { agentType: "custom:goose", name: "goose", iconUrl: "data:image/png;," },
    ]

    setCustomAgentDisplay(entries)
    expect(listener).toHaveBeenCalledTimes(1)
    const version = getCustomAgentDisplayVersion()

    // The agent list is refetched on unrelated events; an identical payload
    // must not re-render every AgentIcon in the app.
    setCustomAgentDisplay(entries)
    expect(listener).toHaveBeenCalledTimes(1)
    expect(getCustomAgentDisplayVersion()).toBe(version)

    setCustomAgentDisplay([
      { agentType: "custom:goose", name: "goose", iconUrl: "data:image/png;x" },
    ])
    expect(listener).toHaveBeenCalledTimes(2)
    expect(getCustomAgentDisplayVersion()).toBeGreaterThan(version)

    unsubscribe()
    setCustomAgentDisplay([])
    expect(listener).toHaveBeenCalledTimes(2)
  })
})
