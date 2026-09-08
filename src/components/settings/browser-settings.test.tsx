import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  browserClearData: vi.fn(),
  browserCapabilitiesNow: vi.fn(),
  toast: { success: vi.fn(), error: vi.fn() },
}))

vi.mock("@/lib/browser/browser-api", () => ({
  browserClearData: mocks.browserClearData,
  browserCapabilitiesNow: mocks.browserCapabilitiesNow,
}))
vi.mock("@/lib/platform", () => ({ isDesktop: () => true }))
vi.mock("sonner", () => ({ toast: mocks.toast }))

import { BrowserSettingsSection } from "./browser-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  getBrowserPrefs,
  resetBrowserPrefsForTests,
  setDefaultLinkTarget,
} from "@/lib/browser/browser-prefs"

function renderSection() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BrowserSettingsSection />
    </NextIntlClientProvider>
  )
}

/** The section arrives folded; every knob lives under the heading. */
function expandSection() {
  fireEvent.click(screen.getByRole("button", { name: "Built-in browser" }))
}

function capabilitiesWith(proxy: {
  url: string | null
  applies: "live" | "next-tab" | "restart" | "unsupported"
  reason: string | null
}) {
  return {
    available: true,
    surface: "child",
    platform: "macos",
    channel: "native",
    reasons: [],
    isolatedStorage: true,
    proxy,
    downloadsDir: "/Users/dev/Downloads",
    policy: { enabled: true, managedRules: [], managedSource: null },
  }
}

beforeEach(() => {
  resetBrowserPrefsForTests()
  mocks.browserClearData.mockReset()
  mocks.browserCapabilitiesNow.mockReset()
  mocks.browserCapabilitiesNow.mockResolvedValue(
    capabilitiesWith({ url: null, applies: "live", reason: null })
  )
  mocks.toast.success.mockReset()
  mocks.toast.error.mockReset()
})

describe("BrowserSettingsSection", () => {
  it("arrives folded and shows one picker per link source once open", () => {
    renderSection()
    expect(
      screen.queryByRole("combobox", { name: "Conversation messages" })
    ).not.toBeInTheDocument()

    expandSection()
    for (const source of [
      "Conversation messages",
      "Tool results",
      "Terminal",
      "Editor",
      "Notifications",
    ]) {
      expect(screen.getByRole("combobox", { name: source })).toHaveTextContent(
        "Built-in browser"
      )
    }
    expect(screen.getByLabelText("Web inspector")).not.toBeChecked()
    expect(
      screen.getByRole("combobox", { name: "Tab surface" })
    ).toHaveTextContent("Automatic")
  })

  it("adds, validates and removes site rules, writing the preference at once", () => {
    renderSection()
    expandSection()
    expect(screen.getByText("No rules yet.")).toBeInTheDocument()

    const pattern = screen.getByRole("textbox", { name: "Site pattern" })
    fireEvent.change(pattern, { target: { value: " Blocked.Example " } })
    fireEvent.click(screen.getByRole("button", { name: "Add" }))
    // Stored normalized, with the picker's default action.
    expect(getBrowserPrefs().hostRules).toEqual([
      { pattern: "blocked.example", action: "system" },
    ])
    expect(screen.getByText("blocked.example")).toBeInTheDocument()
    expect(screen.queryByText("No rules yet.")).not.toBeInTheDocument()
    expect(pattern).toHaveValue("")

    fireEvent.change(pattern, { target: { value: "blocked.example" } })
    fireEvent.click(screen.getByRole("button", { name: "Add" }))
    expect(
      screen.getByText("There is already a rule for this pattern")
    ).toBeInTheDocument()
    expect(getBrowserPrefs().hostRules).toHaveLength(1)

    fireEvent.change(pattern, { target: { value: "https://not a pattern" } })
    fireEvent.click(screen.getByRole("button", { name: "Add" }))
    expect(screen.getByText("Not a valid pattern")).toBeInTheDocument()
    expect(getBrowserPrefs().hostRules).toHaveLength(1)

    fireEvent.click(screen.getByRole("button", { name: "Remove rule" }))
    expect(getBrowserPrefs().hostRules).toEqual([])
    expect(screen.getByText("No rules yet.")).toBeInTheDocument()
  })

  it("shows the administrator's rules read-only and says when the browser is turned off", async () => {
    mocks.browserCapabilitiesNow.mockResolvedValue({
      ...capabilitiesWith({ url: null, applies: "live", reason: null }),
      available: false,
      policy: {
        enabled: false,
        managedRules: [{ pattern: "*.internal.example", action: "block" }],
        managedSource: "/etc/codeg/policy.json",
      },
    })
    renderSection()
    expandSection()
    expect(await screen.findByText("*.internal.example")).toBeInTheDocument()
    expect(
      screen.getByText(/turned off by your administrator/)
    ).toBeInTheDocument()
    expect(
      screen.getAllByLabelText("Set by your administrator").length
    ).toBeGreaterThan(0)
    // Nothing to remove: it is not the user's row.
    expect(
      screen.queryByRole("button", { name: "Remove rule" })
    ).not.toBeInTheDocument()
  })

  it("persists the background-unload switch, which is off by default", () => {
    renderSection()
    expandSection()
    const toggle = screen.getByLabelText("Unload background tabs")
    expect(toggle).not.toBeChecked()

    fireEvent.click(toggle)
    expect(getBrowserPrefs().suspendBackgroundTabs).toBe(true)
    expect(screen.getByLabelText("Unload background tabs")).toBeChecked()

    fireEvent.click(screen.getByLabelText("Unload background tabs"))
    expect(getBrowserPrefs().suspendBackgroundTabs).toBe(false)
  })

  it("persists the inspector switch and follows a change made elsewhere", () => {
    renderSection()
    expandSection()

    fireEvent.click(screen.getByLabelText("Web inspector"))
    expect(getBrowserPrefs().devtools).toBe(true)
    expect(screen.getByLabelText("Web inspector")).toBeChecked()

    // The workspace window (the first-open toast) writes the same keys.
    act(() => setDefaultLinkTarget("terminal", "system"))
    expect(
      screen.getByRole("combobox", { name: "Terminal" })
    ).toHaveTextContent("System browser")
    expect(
      screen.getByRole("combobox", { name: "Conversation messages" })
    ).toHaveTextContent("Built-in browser")
  })

  it("clears browsing data only after confirmation", async () => {
    mocks.browserClearData.mockResolvedValue(undefined)
    renderSection()
    expandSection()

    fireEvent.click(screen.getByRole("button", { name: "Clear…" }))
    expect(mocks.browserClearData).not.toHaveBeenCalled()
    expect(
      screen.getByRole("heading", { name: "Clear browsing data?" })
    ).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Clear" }))
    await waitFor(() => expect(mocks.browserClearData).toHaveBeenCalledTimes(1))
    await waitFor(() =>
      expect(mocks.toast.success).toHaveBeenCalledWith("Browsing data cleared")
    )
  })

  it("shows the proxy browser tabs use, fetched when the section opens", async () => {
    mocks.browserCapabilitiesNow.mockResolvedValue(
      capabilitiesWith({
        url: "http://127.0.0.1:7890",
        applies: "live",
        reason: null,
      })
    )
    renderSection()
    expect(mocks.browserCapabilitiesNow).not.toHaveBeenCalled()
    expandSection()
    expect(
      await screen.findByText("Using http://127.0.0.1:7890")
    ).toBeInTheDocument()
    expect(screen.queryByText(/Restart codeg/)).not.toBeInTheDocument()
  })

  it("explains when the proxy needs a restart or is not usable", async () => {
    mocks.browserCapabilitiesNow.mockResolvedValue(
      capabilitiesWith({
        url: "socks5://10.0.0.1:1080",
        applies: "restart",
        reason: "restart codeg for browser tabs to use the new proxy",
      })
    )
    const { unmount } = renderSection()
    expandSection()
    expect(
      await screen.findByText("Using socks5://10.0.0.1:1080")
    ).toBeInTheDocument()
    expect(screen.getByText(/Restart codeg/)).toBeInTheDocument()
    unmount()

    mocks.browserCapabilitiesNow.mockResolvedValue(
      capabilitiesWith({
        url: null,
        applies: "live",
        reason: "the built-in browser cannot use a https:// proxy",
      })
    )
    renderSection()
    expandSection()
    expect(
      await screen.findByText(/not one browser tabs can use/)
    ).toBeInTheDocument()
  })

  it("keeps the dialog and reports the failure when clearing fails", async () => {
    mocks.browserClearData.mockRejectedValue(new Error("WebKit said no"))
    renderSection()
    expandSection()

    fireEvent.click(screen.getByRole("button", { name: "Clear…" }))
    fireEvent.click(screen.getByRole("button", { name: "Clear" }))
    await waitFor(() =>
      expect(mocks.toast.error).toHaveBeenCalledWith(
        "Could not clear browsing data: WebKit said no"
      )
    )
    expect(
      screen.getByRole("heading", { name: "Clear browsing data?" })
    ).toBeInTheDocument()
  })
})
