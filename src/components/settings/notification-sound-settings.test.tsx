import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it } from "vitest"

import { NotificationSoundSettingsSection } from "./notification-sound-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  DEFAULT_NOTIFICATION_SOUND_PREFS,
  loadNotificationSoundPrefs,
  resetNotificationSoundPrefsCacheForTests,
  type NotificationSoundPrefs,
} from "@/lib/notification-sound-prefs"

const STORAGE_KEY = "settings:notification-sound:v1"

function renderSection() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <NotificationSoundSettingsSection />
    </NextIntlClientProvider>
  )
}

/** Write the key the way another window would, then announce it. */
function writeFromAnotherWindow(prefs: NotificationSoundPrefs) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs))
  act(() => {
    window.dispatchEvent(new StorageEvent("storage", { key: STORAGE_KEY }))
  })
}

beforeEach(() => {
  localStorage.clear()
  resetNotificationSoundPrefsCacheForTests()
})

describe("NotificationSoundSettingsSection", () => {
  it("hides the per-event rows until sounds are enabled", () => {
    renderSection()

    expect(
      screen.getByRole("switch", { name: /notification sounds/i })
    ).not.toBeChecked()
    // With sounds off the section IS the master switch: the heading labels it,
    // so there is no second row (nor a card around one) saying so again.
    expect(screen.getAllByRole("switch")).toHaveLength(3)
    // The event catalogue is only meaningful once something can play.
    expect(screen.queryByText("Turn Complete")).not.toBeInTheDocument()
  })

  it("persists the system notification switches", () => {
    renderSection()

    expect(
      screen.getByRole("switch", { name: /system notifications/i })
    ).toBeChecked()
    expect(
      screen.getByRole("switch", {
        name: /only when the Codeg window is not focused/i,
      })
    ).toBeChecked()
    fireEvent.click(
      screen.getByRole("switch", {
        name: /only when the Codeg window is not focused/i,
      })
    )

    expect(
      loadNotificationSoundPrefs().systemNotificationsOnlyWhenUnfocused
    ).toBe(false)
    fireEvent.click(
      screen.getByRole("switch", { name: /system notifications/i })
    )
    expect(loadNotificationSoundPrefs().systemNotificationsEnabled).toBe(false)
    expect(
      screen.queryByRole("switch", {
        name: /only when the Codeg window is not focused/i,
      })
    ).not.toBeInTheDocument()
  })

  it("persists the master switch and reveals the event catalogue", () => {
    renderSection()

    fireEvent.click(
      screen.getByRole("switch", { name: /notification sounds/i })
    )

    expect(loadNotificationSoundPrefs().enabled).toBe(true)
    // Same five triggers as the chat-channel Events tab, same wording.
    for (const label of [
      "Turn Complete",
      "Permission Request",
      "Agent Question",
      "Agent Error",
      "User Message",
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument()
    }
  })

  it("shows the default tone of every event, including the silent one", () => {
    writeFromAnotherWindow({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      enabled: true,
    })
    renderSection()

    expect(
      screen.getByRole("combobox", { name: "Turn Complete" })
    ).toHaveTextContent("Chime")
    expect(
      screen.getByRole("combobox", { name: "Permission Request" })
    ).toHaveTextContent("Alert")
    expect(
      screen.getByRole("combobox", { name: "Agent Question" })
    ).toHaveTextContent("Ding")
    expect(
      screen.getByRole("combobox", { name: "Agent Error" })
    ).toHaveTextContent("Descending")
    // user_prompt_sent echoes the user's own action — off out of the box.
    expect(
      screen.getByRole("combobox", { name: "User Message" })
    ).toHaveTextContent("Silent")
  })

  it("follows a change made in another window", () => {
    renderSection()
    expect(
      screen.getByRole("switch", { name: /notification sounds/i })
    ).not.toBeChecked()

    writeFromAnotherWindow({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      enabled: true,
      volume: 0.3,
    })

    expect(
      screen.getByRole("switch", { name: /notification sounds/i })
    ).toBeChecked()
    expect(screen.getByText("30%")).toBeInTheDocument()
  })

  it("silences an event when its preview button is disabled", () => {
    writeFromAnotherWindow({
      ...DEFAULT_NOTIFICATION_SOUND_PREFS,
      enabled: true,
    })
    renderSection()

    // A `none` tone has nothing to preview; every other row does.
    expect(
      screen.getByRole("button", { name: /preview the user message sound/i })
    ).toBeDisabled()
    expect(
      screen.getByRole("button", { name: /preview the turn complete sound/i })
    ).toBeEnabled()
  })
})
