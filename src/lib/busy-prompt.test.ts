import { describe, expect, it } from "vitest"

import {
  busyPromptStop,
  draftFromOptimisticUserTurn,
  planBusyAbsorb,
  shouldNotifyTurnComplete,
} from "@/lib/busy-prompt"

describe("busyPromptStop", () => {
  it("requeues a lost prompt and keeps one the agent accepted", () => {
    expect(busyPromptStop("busy")).toBe("requeue")
    expect(busyPromptStop("deferred")).toBe("keep")
  })

  it("leaves real turn endings alone, including a native-steering end_turn", () => {
    expect(busyPromptStop("end_turn")).toBeNull()
    expect(busyPromptStop("empty")).toBeNull()
    expect(busyPromptStop("cancelled")).toBeNull()
    expect(busyPromptStop("rejected")).toBeNull()
  })
})

describe("shouldNotifyTurnComplete", () => {
  it("stays quiet for an absorb and fires for a real stop", () => {
    expect(shouldNotifyTurnComplete("busy")).toBe(false)
    expect(shouldNotifyTurnComplete("deferred")).toBe(false)
    expect(shouldNotifyTurnComplete("end_turn")).toBe(true)
    expect(shouldNotifyTurnComplete("refusal")).toBe(true)
    expect(shouldNotifyTurnComplete("cancelled")).toBe(true)
    expect(shouldNotifyTurnComplete("empty")).toBe(true)
  })
})

describe("planBusyAbsorb", () => {
  const resourceDraft = {
    blocks: [
      { type: "text" as const, text: "fix the launch" },
      {
        type: "resource" as const,
        uri: "file:///tmp/main.rs",
        mime_type: "text/x-rust",
        text: "fn main() {}",
      },
      {
        type: "resource_link" as const,
        uri: "file:///tmp/Cargo.toml",
        name: "Cargo.toml",
      },
    ],
    displayText: "fix the launch",
  }

  it("requeues the submitted draft and its mode once", () => {
    expect(
      planBusyAbsorb({
        stopReason: "busy",
        submitted: { draft: resourceDraft, modeId: "plan" },
        optimisticTurns: [
          {
            role: "user",
            blocks: [{ type: "text", text: "fix the launch" }],
          },
        ],
      })
    ).toEqual({
      action: "requeue",
      draft: resourceDraft,
      modeId: "plan",
    })
  })

  it("keeps a deferred prompt without promoting a second send", () => {
    expect(
      planBusyAbsorb({
        stopReason: "deferred",
        submitted: { draft: resourceDraft, modeId: "plan" },
      })
    ).toEqual({ action: "keep" })
  })

  it("ignores a real turn end", () => {
    expect(
      planBusyAbsorb({
        stopReason: "end_turn",
        submitted: { draft: resourceDraft, modeId: "plan" },
      })
    ).toEqual({ action: "ignore" })
  })

  it("falls back to the optimistic bubble when the send was not remembered", () => {
    expect(
      planBusyAbsorb({
        stopReason: "busy",
        submitted: null,
        optimisticTurns: [
          {
            role: "user",
            blocks: [{ type: "text", text: "fix the launch" }],
          },
        ],
      })
    ).toEqual({
      action: "requeue",
      draft: {
        blocks: [{ type: "text", text: "fix the launch" }],
        displayText: "fix the launch",
      },
      modeId: null,
    })
  })
})

describe("draftFromOptimisticUserTurn", () => {
  it("rebuilds text and images from the optimistic user turn", () => {
    expect(
      draftFromOptimisticUserTurn({
        role: "user",
        blocks: [
          {
            type: "image",
            data: "aaa",
            mime_type: "image/png",
            uri: "file:///tmp/a.png",
          },
          { type: "text", text: "fix the launch" },
        ],
      })
    ).toEqual({
      blocks: [
        {
          type: "image",
          data: "aaa",
          mime_type: "image/png",
          uri: "file:///tmp/a.png",
        },
        { type: "text", text: "fix the launch" },
      ],
      displayText: "fix the launch",
    })
  })

  it("ignores assistant turns and empty text", () => {
    expect(
      draftFromOptimisticUserTurn({
        role: "assistant",
        blocks: [{ type: "text", text: "Redirected the active turn" }],
      })
    ).toBeNull()
    expect(
      draftFromOptimisticUserTurn({
        role: "user",
        blocks: [{ type: "text", text: "   " }],
      })
    ).toBeNull()
  })
})
