import { beforeEach, describe, expect, it, vi } from "vitest"

const h = vi.hoisted(() => ({
  call: vi.fn(async () => undefined),
}))

vi.mock("./transport", () => ({
  getTransport: () => ({ call: h.call }),
  isDesktop: () => true,
}))

import { sendSystemNotification } from "./notification"

describe("sendSystemNotification", () => {
  beforeEach(() => {
    h.call.mockClear()
    vi.spyOn(document, "hasFocus").mockReturnValue(true)
  })

  it("stays quiet while Codeg is focused", async () => {
    await sendSystemNotification("Codeg", "done")

    expect(h.call).not.toHaveBeenCalled()
  })

  it("uses the native notification when Codeg loses focus", async () => {
    vi.spyOn(document, "hasFocus").mockReturnValue(false)

    await sendSystemNotification("Codeg", "done")

    expect(h.call).toHaveBeenCalledWith("send_notification", {
      title: "Codeg",
      body: "done",
    })
  })
})
