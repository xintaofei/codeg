import { beforeEach, describe, expect, it, vi } from "vitest"

import type { PromptInputBlock } from "@/lib/types"

// acpPrompt reads the environment to decide payload stripping; pin it to the
// plain-web shape (no remote workspace attached) so blocks pass through.
vi.mock("@/lib/platform", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  isDesktop: () => false,
}))

const callMock = vi.fn()

vi.mock("./transport", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  getTransport: () => ({ call: callMock }),
  getActiveRemoteConnectionId: () => null,
}))

import { acpPrompt } from "./api"

const blocks: PromptInputBlock[] = [{ type: "text", text: "@claude_code hi" }]

beforeEach(() => {
  callMock.mockReset()
  callMock.mockResolvedValue(undefined)
})

describe("acpPrompt delegationOverrides serialization", () => {
  it("ships a populated override map as the camelCase payload field", async () => {
    await acpPrompt("conn-1", blocks, 3, 7, "msg-1", {
      claude_code: {
        mode_id: "plan",
        config_values: { model: "claude-opus-4-1" },
      },
    })
    expect(callMock).toHaveBeenCalledTimes(1)
    const [command, payload] = callMock.mock.calls[0]
    expect(command).toBe("acp_prompt")
    expect(payload.delegationOverrides).toEqual({
      claude_code: {
        mode_id: "plan",
        config_values: { model: "claude-opus-4-1" },
      },
    })
    // The rest of the payload keeps its existing shape.
    expect(payload.connectionId).toBe("conn-1")
    expect(payload.blocks).toEqual(blocks)
    expect(payload.folderId).toBe(3)
    expect(payload.conversationId).toBe(7)
    expect(payload.clientMessageId).toBe("msg-1")
  })

  it("omits the field from the wire payload when no overrides are passed (compat)", async () => {
    await acpPrompt("conn-1", blocks)
    const [, payload] = callMock.mock.calls[0]
    // The in-memory payload object carries the key as `undefined`; JSON — the
    // actual wire form for both HTTP bodies and Tauri invoke args — drops it.
    expect(JSON.parse(JSON.stringify(payload))).not.toHaveProperty(
      "delegationOverrides"
    )
  })

  it("omits the field from the wire payload for an undefined override map", async () => {
    await acpPrompt("conn-1", blocks, null, null, null, undefined)
    const [, payload] = callMock.mock.calls[0]
    expect(JSON.parse(JSON.stringify(payload))).not.toHaveProperty(
      "delegationOverrides"
    )
  })
})
