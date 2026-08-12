import { beforeEach, describe, expect, it, vi } from "vitest"

const h = vi.hoisted(() => ({
  call: vi.fn(async () => undefined),
  desktop: true,
}))

vi.mock("./transport", () => ({
  getTransport: () => ({ call: h.call }),
  isDesktop: () => h.desktop,
}))

import { sendSystemNotification } from "./notification"

describe("sendSystemNotification", () => {
  beforeEach(() => {
    h.call.mockClear()
    h.desktop = true
    vi.unstubAllGlobals()
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    })
    vi.spyOn(document, "hasFocus").mockReturnValue(true)
  })

  it("uses the native notification even while Codeg is focused", async () => {
    await sendSystemNotification("Codeg", "done")

    expect(h.call).toHaveBeenCalledWith("send_notification", {
      title: "Codeg",
      body: "done",
    })
  })

  it("stays quiet in a focused web browser", async () => {
    h.desktop = false

    await sendSystemNotification("Codeg", "done")

    expect(h.call).not.toHaveBeenCalled()
  })

  it("uses the browser notification when the web page is in the background", async () => {
    h.desktop = false
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    })
    const notification = vi.fn()
    Object.assign(notification, {
      permission: "granted",
      requestPermission: vi.fn(),
    })
    vi.stubGlobal("Notification", notification)

    await sendSystemNotification("Codeg", "done")

    expect(notification).toHaveBeenCalledWith("Codeg", { body: "done" })
  })
})
