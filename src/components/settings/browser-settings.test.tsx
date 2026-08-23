import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

const api = vi.hoisted(() => ({
  doctorBrowserRuntime: vi.fn(),
  getBrowserRuntimeSettings: vi.fn(),
  getBrowserRuntimeStatus: vi.fn(),
  recoverBrowserRuntime: vi.fn(),
  restartBrowserRuntime: vi.fn(),
  startBrowserRuntime: vi.fn(),
  updateBrowserRuntimeSettings: vi.fn(),
}))

const platform = vi.hoisted(() => ({
  openUrl: vi.fn(),
  subscribe: vi.fn(async () => () => undefined),
}))

vi.mock("@/lib/api", () => api)
vi.mock("@/lib/platform", () => platform)

import {
  browserAvailabilityFromDoctor,
  BrowserSettings,
} from "./browser-settings"
import enMessages from "@/i18n/messages/en.json"
import type { BrowserRuntimeSettings, BrowserRuntimeStatus } from "@/lib/api"

const embeddedSettings: BrowserRuntimeSettings = {
  enabled: false,
  autoStart: false,
  browserPath: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  backend: "embedded",
}

const embeddedStoppedStatus: BrowserRuntimeStatus = {
  state: "stopped",
  installed: true,
  enabled: false,
  autoStart: false,
  browserPath: embeddedSettings.browserPath,
  sidecarPid: null,
  browserPid: null,
  runtimeVersion: "0.1.0",
  backend: "embedded_webview2",
  browserName: null,
  browserVersion: null,
  profilePath: "C:\\Codeg\\browser\\profile",
  downloadPath: "C:\\Codeg\\browser\\downloads",
  recoveryAttempt: 0,
  lastErrorCode: null,
  recentLogs: [],
}

const chromeAvailable = {
  ok: false,
  checks: [
    { name: "browser", ok: true, detail: "chrome.exe" },
    { name: "process", ok: false, detail: "stopped" },
  ],
}

function renderSettings() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BrowserSettings />
    </NextIntlClientProvider>
  )
}

describe("Browser settings", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.getBrowserRuntimeSettings.mockResolvedValue(embeddedSettings)
    api.getBrowserRuntimeStatus.mockResolvedValue(embeddedStoppedStatus)
    api.doctorBrowserRuntime.mockResolvedValue(chromeAvailable)
    api.updateBrowserRuntimeSettings.mockImplementation(async (settings) =>
      Promise.resolve(settings)
    )
    api.startBrowserRuntime.mockResolvedValue(embeddedStoppedStatus)
    api.restartBrowserRuntime.mockResolvedValue(embeddedStoppedStatus)
    api.recoverBrowserRuntime.mockResolvedValue(embeddedStoppedStatus)
  })

  it("waits for settings, backend, and health truth before rendering the card", async () => {
    let resolveDoctor: (value: unknown) => void = () => undefined
    api.getBrowserRuntimeSettings.mockResolvedValue({
      ...embeddedSettings,
      backend: "external",
    })
    api.getBrowserRuntimeStatus.mockResolvedValue({
      ...embeddedStoppedStatus,
      backend: "external_chromium_cdp",
    })
    api.doctorBrowserRuntime.mockReturnValue(
      new Promise((resolve) => {
        resolveDoctor = resolve
      })
    )

    renderSettings()

    expect(screen.getByText("Loading Browser runtime…")).toBeInTheDocument()
    expect(
      screen.queryByTestId("browser-automation-block")
    ).not.toBeInTheDocument()
    await waitFor(() => expect(api.doctorBrowserRuntime).toHaveBeenCalledOnce())
    resolveDoctor(chromeAvailable)

    expect(
      await screen.findByTestId("browser-automation-block")
    ).toBeInTheDocument()
  })

  it("renders one Cindy-aligned card without operational details", async () => {
    renderSettings()

    expect(
      await screen.findByRole("heading", { name: "Browser" })
    ).toBeInTheDocument()
    expect(screen.getAllByTestId("browser-automation-block")).toHaveLength(1)
    expect(
      screen.getByRole("switch", { name: "Enable Browser MCP" })
    ).not.toBeChecked()
    expect(
      screen.getByRole("tab", { name: "Embedded browser" })
    ).toHaveAttribute("aria-selected", "true")
    expect(
      screen.getByText("Embedded browser is not connected")
    ).toBeInTheDocument()
    expect(
      screen.getByText(
        "Changes to Browser access apply to new Agent sessions. Running sessions keep the capability they started with."
      )
    ).toBeInTheDocument()

    for (const removedText of [
      "Runtime version",
      "Managed profile",
      "Browser executable",
      "Start with Codeg",
      "Start",
      "Stop",
      "Run doctor",
      "Test connection",
      "View diagnostics",
      "Recent runtime codes",
      embeddedStoppedStatus.profilePath,
    ]) {
      expect(screen.queryByText(removedText)).not.toBeInTheDocument()
    }
  })

  it("changes the future-session toggle only after persistence succeeds", async () => {
    let confirmUpdate: (settings: BrowserRuntimeSettings) => void = () =>
      undefined
    api.updateBrowserRuntimeSettings.mockReturnValue(
      new Promise((resolve) => {
        confirmUpdate = resolve
      })
    )
    renderSettings()
    const enabled = await screen.findByRole("switch", {
      name: "Enable Browser MCP",
    })

    fireEvent.click(enabled)

    expect(api.updateBrowserRuntimeSettings).toHaveBeenCalledWith({
      ...embeddedSettings,
      enabled: true,
      autoStart: true,
    })
    expect(enabled).not.toBeChecked()
    confirmUpdate({ ...embeddedSettings, enabled: true, autoStart: true })

    await waitFor(() => expect(enabled).toBeChecked())
  })

  it("keeps the old future-session value when persistence fails", async () => {
    api.updateBrowserRuntimeSettings.mockRejectedValueOnce(
      new Error("settings write failed")
    )
    renderSettings()
    const enabled = await screen.findByRole("switch", {
      name: "Enable Browser MCP",
    })

    fireEvent.click(enabled)

    expect(
      await screen.findByText("Browser operation failed: settings write failed")
    ).toBeInTheDocument()
    expect(enabled).not.toBeChecked()
  })

  it("switches backend immediately and rolls back to Tauri truth on failure", async () => {
    api.updateBrowserRuntimeSettings.mockRejectedValueOnce(
      new Error("restart failed")
    )
    renderSettings()
    const embedded = await screen.findByRole("tab", {
      name: "Embedded browser",
    })
    const external = screen.getByRole("tab", { name: "Separate browser" })

    fireEvent.click(external)
    expect(external).toHaveAttribute("aria-selected", "true")

    await waitFor(() => {
      expect(api.updateBrowserRuntimeSettings).toHaveBeenCalledWith({
        ...embeddedSettings,
        backend: "external",
      })
      expect(embedded).toHaveAttribute("aria-selected", "true")
    })
    expect(
      screen.getByText("Browser operation failed: restart failed")
    ).toBeInTheDocument()
  })

  it("shows only embedded health and uses reconnect or recover for its real state", async () => {
    const readyStatus = {
      ...embeddedStoppedStatus,
      state: "ready" as const,
      enabled: true,
    }
    api.getBrowserRuntimeSettings.mockResolvedValue({
      ...embeddedSettings,
      enabled: true,
    })
    api.getBrowserRuntimeStatus.mockResolvedValue(readyStatus)
    api.restartBrowserRuntime.mockResolvedValue(readyStatus)
    renderSettings()

    fireEvent.click(await screen.findByRole("button", { name: "Reconnect" }))
    await waitFor(() =>
      expect(api.restartBrowserRuntime).toHaveBeenCalledOnce()
    )
    expect(
      screen.queryByText("Google Chrome is available")
    ).not.toBeInTheDocument()
  })

  it("uses recover for an embedded runtime error", async () => {
    const failedStatus = {
      ...embeddedStoppedStatus,
      state: "error" as const,
      enabled: true,
      lastErrorCode: "BROWSER_CRASHED",
    }
    api.getBrowserRuntimeSettings.mockResolvedValue({
      ...embeddedSettings,
      enabled: true,
    })
    api.getBrowserRuntimeStatus.mockResolvedValue(failedStatus)
    api.recoverBrowserRuntime.mockResolvedValue({
      ...failedStatus,
      state: "ready",
      lastErrorCode: null,
    })
    renderSettings()

    expect(
      await screen.findByText("Embedded browser error: BROWSER_CRASHED")
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Recover" }))

    await waitFor(() =>
      expect(api.recoverBrowserRuntime).toHaveBeenCalledOnce()
    )
  })

  it("shows real external availability and opens the managed Agent browser", async () => {
    const externalSettings = {
      ...embeddedSettings,
      enabled: true,
      backend: "external" as const,
    }
    const externalStatus = {
      ...embeddedStoppedStatus,
      enabled: true,
      backend: "external_chromium_cdp",
    }
    api.getBrowserRuntimeSettings.mockResolvedValue(externalSettings)
    api.getBrowserRuntimeStatus.mockResolvedValue(externalStatus)
    api.startBrowserRuntime.mockResolvedValue({
      ...externalStatus,
      state: "ready",
    })
    renderSettings()

    expect(
      await screen.findByText("Google Chrome is available")
    ).toBeInTheDocument()
    expect(
      screen.queryByText("Embedded browser is not connected")
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Open Agent browser" }))

    await waitFor(() => expect(api.startBrowserRuntime).toHaveBeenCalledOnce())
  })

  it("offers only the official Chrome link when no executable is detected", async () => {
    api.getBrowserRuntimeSettings.mockResolvedValue({
      ...embeddedSettings,
      backend: "external",
    })
    api.getBrowserRuntimeStatus.mockResolvedValue({
      ...embeddedStoppedStatus,
      backend: "external_chromium_cdp",
    })
    api.doctorBrowserRuntime.mockResolvedValue({
      ok: false,
      checks: [{ name: "browser", ok: false, detail: "not_found" }],
    })
    renderSettings()

    expect(
      await screen.findByText("No supported browser detected")
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Get Chrome" }))

    expect(platform.openUrl).toHaveBeenCalledWith(
      "https://www.google.com/chrome/"
    )
    expect(api.startBrowserRuntime).not.toHaveBeenCalled()
  })
})

describe("browserAvailabilityFromDoctor", () => {
  it("accepts the Tauri command envelope and normalizes Edge", () => {
    expect(
      browserAvailabilityFromDoctor({
        data: {
          checks: [{ name: "browser", ok: true, detail: "msedge.exe" }],
        },
      })
    ).toEqual({ detected: true, browser: "Microsoft Edge" })
  })
})
