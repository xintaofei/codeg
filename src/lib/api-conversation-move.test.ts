import { beforeEach, describe, expect, it, vi } from "vitest"

const h = vi.hoisted(() => ({ call: vi.fn() }))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: h.call }),
  getShellTransport: () => ({ call: vi.fn() }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
  notifyRemoteDesktopUnauthorized: vi.fn(),
}))

import { moveConversation } from "@/lib/api"

describe("moveConversation API", () => {
  beforeEach(() => h.call.mockReset())

  it("uses the shared command and camelCase transport payload", async () => {
    const result = { id: 41, folder_id: 9 }
    h.call.mockResolvedValue(result)

    await expect(moveConversation(41, 9)).resolves.toBe(result)
    expect(h.call).toHaveBeenCalledWith("move_conversation", {
      conversationId: 41,
      targetFolderId: 9,
    })
  })
})
