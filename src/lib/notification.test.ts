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
import {
  DEFAULT_NOTIFICATION_SOUND_PREFS,
  resetNotificationSoundPrefsCacheForTests,
  saveNotificationSoundPrefs,
} from "./notification-sound-prefs"

describe("sendSystemNotification", () => {
  beforeEach(() => {
    h.call.mockClear()
    h.desktop = true
    localStorage.clear()
    resetNotificationSoundPrefsCacheForTests()
    vi.unstubAllGlobals()
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: false,
    })
    vi.spyOn(document, "hasFocus").mockReturnValue(true)
  })

  it("uses the native notification while Codeg is focused when background-only mode is off", async () => {
    saveNotificationSoundPrefs({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      systemNotificationsOnlyWhenUnfocused: false,
    })

    await sendSystemNotification("Codeg", "done")

    expect(h.call).toHaveBeenCalledWith("send_notification", {
      title: "Codeg",
      body: "done",
      target: undefined,
    })
  })

  it("stays quiet when system notifications are disabled", async () => {
    saveNotificationSoundPrefs({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      systemNotificationsEnabled: false,
    })

    await sendSystemNotification("Codeg", "done")

    expect(h.call).not.toHaveBeenCalled()
  })

  it("stays quiet while focused when background-only notifications are enabled", async () => {
    saveNotificationSoundPrefs({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      systemNotificationsOnlyWhenUnfocused: true,
    })

    await sendSystemNotification("Codeg", "done")

    expect(h.call).not.toHaveBeenCalled()
  })

  it("uses the native notification while hidden in background-only mode", async () => {
    saveNotificationSoundPrefs({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      systemNotificationsOnlyWhenUnfocused: true,
    })
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    })

    await sendSystemNotification("Codeg", "done")

    expect(h.call).toHaveBeenCalledOnce()
  })

  it("passes the conversation target to the native notification", async () => {
    const target = { folderId: 7, conversationId: 42, agent: "codex" }
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    })

    await sendSystemNotification("Codeg", "done", target)

    expect(h.call).toHaveBeenCalledWith("send_notification", {
      title: "Codeg",
      body: "done",
      target,
    })
  })

  it("stays quiet in a focused web browser", async () => {
    h.desktop = false
    saveNotificationSoundPrefs({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      systemNotificationsOnlyWhenUnfocused: true,
    })

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
