import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  openBrowserTab: vi.fn(() => "browser:new"),
  viewerOpen: vi.fn(),
  openInSystemBrowser: vi.fn(() => Promise.resolve()),
  openWithOsHandler: vi.fn(() => Promise.resolve()),
  toast: Object.assign(vi.fn(), { error: vi.fn() }),
  remote: false,
  route: { isConversations: true } as { isConversations: boolean } | null,
  viewerHost: null as { open: (r: unknown) => void } | null,
  actions: null as { openBrowserTab: (url: string) => string | null } | null,
  /** The backend's answer to `browser_capabilities` for the deferral tests. */
  transportCall: vi.fn(() => Promise.resolve(undefined as unknown)),
}))

vi.mock("next-intl", () => ({ useTranslations: () => (key: string) => key }))
vi.mock("sonner", () => ({ toast: mocks.toast }))
vi.mock("@/contexts/workspace-context", () => ({
  useOptionalWorkspaceActions: () => mocks.actions,
}))
vi.mock("@/contexts/workbench-route-context", () => ({
  useOptionalWorkbenchRoute: () => mocks.route,
}))
vi.mock("@/components/message/session-viewer-host-context", () => ({
  useSessionViewerHost: () => mocks.viewerHost,
}))
vi.mock("@/lib/link-open", () => ({
  openInSystemBrowser: mocks.openInSystemBrowser,
  openWithOsHandler: mocks.openWithOsHandler,
}))
vi.mock("@/lib/transport", () => ({
  isRemoteDesktopMode: () => mocks.remote,
  isDesktop: () => true,
  getTransport: () => ({ call: mocks.transportCall }),
}))

import {
  resetBrowserCapabilitiesCacheForTests,
  setBrowserCapabilitiesForTests,
} from "@/lib/browser/browser-api"
import { resetBrowserPrefsForTests } from "@/lib/browser/browser-prefs"
import { isPrimaryModifier, useOpenUrlTarget } from "./use-open-url-target"

const AVAILABLE = {
  available: true,
  surface: "child" as const,
  platform: "macos",
  channel: "native" as const,
  reasons: [],
  isolatedStorage: true,
  proxy: { url: null, applies: "live" as const, reason: null },
  downloadsDir: "/Users/dev/Downloads",
  policy: { enabled: true, managedRules: [], managedSource: null },
}

describe("useOpenUrlTarget", () => {
  beforeEach(() => {
    resetBrowserPrefsForTests()
    resetBrowserCapabilitiesCacheForTests()
    setBrowserCapabilitiesForTests(AVAILABLE)
    mocks.transportCall.mockReset()
    mocks.transportCall.mockImplementation(() => Promise.resolve(AVAILABLE))
    mocks.toast.error.mockClear()
    mocks.openBrowserTab.mockClear()
    mocks.viewerOpen.mockClear()
    mocks.openInSystemBrowser.mockClear()
    mocks.openWithOsHandler.mockClear()
    mocks.toast.mockClear()
    mocks.remote = false
    mocks.route = { isConversations: true }
    mocks.viewerHost = null
    mocks.actions = { openBrowserTab: mocks.openBrowserTab }
  })
  afterEach(() => {
    resetBrowserPrefsForTests()
    setBrowserCapabilitiesForTests(null)
  })

  it("opens a web link in a built-in tab by default, synchronously, and toasts once", () => {
    const { result } = renderHook(() => useOpenUrlTarget())
    const action = result.current("https://example.com/docs", {
      source: "transcript",
    })
    expect(action).toMatchObject({ kind: "builtin", placement: "tab" })
    // Called inside the same call stack — no await in between.
    expect(mocks.openBrowserTab).toHaveBeenCalledWith(
      "https://example.com/docs"
    )
    expect(mocks.toast).toHaveBeenCalledTimes(1)
    result.current("https://example.com/2", { source: "transcript" })
    expect(mocks.toast).toHaveBeenCalledTimes(1)
  })

  it("⌘/Ctrl inverts to the system browser, still synchronously", () => {
    const { result } = renderHook(() => useOpenUrlTarget())
    const action = result.current("https://example.com/", {
      source: "terminal",
      modifier: true,
    })
    expect(action.kind).toBe("system")
    expect(mocks.openInSystemBrowser).toHaveBeenCalledWith(
      "https://example.com/"
    )
    expect(mocks.openBrowserTab).not.toHaveBeenCalled()
  })

  it("falls back to the system browser when no built-in browser exists", () => {
    setBrowserCapabilitiesForTests({ ...AVAILABLE, available: false })
    const { result } = renderHook(() => useOpenUrlTarget())
    expect(
      result.current("https://example.com/", { source: "toolCard" }).kind
    ).toBe("system")
    expect(mocks.openInSystemBrowser).toHaveBeenCalledTimes(1)
  })

  // The first moments after launch: the backend has not yet said what it can
  // do or which hosts the administrator blocks. A click then waits for the
  // answer instead of being routed on a guess — routed to the system browser
  // it would slip past a managed block.
  it("holds a click until the capabilities are known, then routes it — honouring a managed block", async () => {
    setBrowserCapabilitiesForTests(null)
    mocks.transportCall.mockImplementation(() =>
      Promise.resolve({
        ...AVAILABLE,
        policy: {
          enabled: true,
          managedRules: [{ pattern: "blocked.example", action: "block" }],
          managedSource: "/etc/codeg/policy.json",
        },
      })
    )
    const { result } = renderHook(() => useOpenUrlTarget())
    const outcome = result.current("https://blocked.example/", {
      source: "transcript",
    })
    expect(outcome.kind).toBe("deferred")
    expect(mocks.openInSystemBrowser).not.toHaveBeenCalled()
    expect(mocks.openBrowserTab).not.toHaveBeenCalled()
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })
    // Decided with the managed rules in hand: blocked, and said so.
    expect(mocks.openInSystemBrowser).not.toHaveBeenCalled()
    expect(mocks.openBrowserTab).not.toHaveBeenCalled()
    expect(mocks.toast.error).toHaveBeenCalledWith("blockedHost")

    // An ordinary link, once the answer is in, goes where it always goes.
    const later = result.current("https://example.com/", {
      source: "transcript",
    })
    expect(later.kind).toBe("builtin")
    expect(mocks.openBrowserTab).toHaveBeenCalledWith("https://example.com/")
  })

  it("uses the viewer drawer under a full-page route", () => {
    mocks.route = { isConversations: false }
    mocks.viewerHost = { open: mocks.viewerOpen }
    const { result } = renderHook(() => useOpenUrlTarget())
    const action = result.current("https://example.com/", {
      source: "transcript",
    })
    expect(action).toMatchObject({ kind: "builtin", placement: "drawer" })
    expect(mocks.viewerOpen).toHaveBeenCalledWith({
      kind: "browser",
      url: "https://example.com/",
    })
    expect(mocks.openBrowserTab).not.toHaveBeenCalled()
  })

  it("without a workspace provider the built-in target is unavailable", () => {
    mocks.actions = null
    const { result } = renderHook(() => useOpenUrlTarget())
    expect(
      result.current("https://example.com/", { source: "editor" }).kind
    ).toBe("system")
  })

  it("keeps a remote-workspace loopback address built-in even with the modifier", () => {
    mocks.remote = true
    const { result } = renderHook(() => useOpenUrlTarget())
    const action = result.current("http://localhost:3000/", {
      source: "transcript",
      modifier: true,
    })
    expect(action).toMatchObject({ kind: "builtin", remoteOverride: true })
    expect(mocks.openBrowserTab).toHaveBeenCalledWith("http://localhost:3000/")
  })

  it("routes mailto/tel to the OS handler and reports unsupported schemes", () => {
    const { result } = renderHook(() => useOpenUrlTarget())
    expect(result.current("mailto:a@b.c", { source: "transcript" }).kind).toBe(
      "os-handler"
    )
    expect(mocks.openWithOsHandler).toHaveBeenCalledWith("mailto:a@b.c")
    expect(
      result.current("vscode://x", { source: "transcript" })
    ).toMatchObject({
      kind: "reject",
      reason: "unsupported-scheme",
    })
  })

  it("explicit menu choices override the preference", () => {
    const { result } = renderHook(() => useOpenUrlTarget())
    expect(
      result.current("https://example.com/", {
        source: "transcript",
        forceTarget: "system",
      }).kind
    ).toBe("system")
  })
})

describe("isPrimaryModifier", () => {
  it("reads ⌘ on macOS and Ctrl elsewhere", () => {
    const platform = vi.spyOn(navigator, "platform", "get")
    platform.mockReturnValue("MacIntel")
    expect(isPrimaryModifier({ metaKey: true, ctrlKey: false })).toBe(true)
    expect(isPrimaryModifier({ metaKey: false, ctrlKey: true })).toBe(false)
    platform.mockReturnValue("Win32")
    expect(isPrimaryModifier({ metaKey: true, ctrlKey: false })).toBe(false)
    expect(isPrimaryModifier({ metaKey: false, ctrlKey: true })).toBe(true)
    platform.mockRestore()
  })
})
