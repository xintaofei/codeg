import { describe, expect, it } from "vitest"

import { busyPromptStop, draftFromOptimisticUserTurn } from "@/lib/busy-prompt"

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
