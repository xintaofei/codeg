import { act, render, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BrowserCapabilities } from "@/lib/browser/types"

type Handler = (payload: unknown) => void

const mocks = vi.hoisted(() => {
  const handlers = new Map<string, Handler>()
  const unsubscribed: string[] = []
  return {
    handlers,
    unsubscribed,
    capabilities: vi.fn(
      (): Promise<BrowserCapabilities> =>
        Promise.resolve({
          available: true,
          surface: "child",
          platform: "macos",
          channel: "native",
          reasons: [],
          isolatedStorage: true,
          proxy: { url: null, applies: "live", reason: null },
          downloadsDir: "/Users/dev/Downloads",
          policy: { enabled: true, managedRules: [], managedSource: null },
        })
    ),
    browserSetHostRules: vi.fn(() => Promise.resolve()),
    subscribe: vi.fn((event: string, handler: Handler) => {
      handlers.set(event, handler)
      return Promise.resolve(() => {
        unsubscribed.push(event)
        handlers.delete(event)
      })
    }),
    adoptBrowserTab: vi.fn(() => "browser:opener-p1"),
    closeFileTab: vi.fn(),
    openBrowserTab: vi.fn(() => "browser:new"),
    browserClose: vi.fn(() => Promise.resolve()),
    browserListTabs: vi.fn(() =>
      Promise.resolve([{ tabId: "stale-1" }, { tabId: "stale-2" }])
    ),
    browserListDownloads: vi.fn(() =>
      Promise.resolve([
        {
          id: "dl-1",
          tabId: "t1",
          url: "https://example.com/a.bin",
          fileName: "a.bin",
          path: "/Users/dev/Downloads/a.bin",
          state: "completed",
        },
      ])
    ),
  }
})

vi.mock("@/lib/browser/browser-api", () => ({
  browserCapabilities: mocks.capabilities,
  browserClose: mocks.browserClose,
  browserListTabs: mocks.browserListTabs,
  browserListDownloads: mocks.browserListDownloads,
  browserSetHostRules: mocks.browserSetHostRules,
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ subscribe: mocks.subscribe }),
  isDesktop: () => true,
}))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({
    adoptBrowserTab: mocks.adoptBrowserTab,
    closeFileTab: mocks.closeFileTab,
    openBrowserTab: mocks.openBrowserTab,
  }),
}))

import {
  resetBrowserPrefsForTests,
  setBrowserHostRules,
} from "@/lib/browser/browser-prefs"
import {
  getBrowserTabState,
  resetBrowserTabStoreForTests,
  setBrowserTabState,
  useBrowserFindRequest,
  useBrowserTabNotice,
} from "@/lib/browser/browser-tab-store"
import {
  getBrowserDownloads,
  resetBrowserDownloadsForTests,
} from "@/lib/browser/browser-downloads-store"
import { BrowserEventsBridge } from "./browser-events-bridge"

/** The store's find counter, read the way a component would. */
function findRequestOf(workspaceTabId: string): number {
  const view = renderHook(() => useBrowserFindRequest(workspaceTabId))
  const seen = view.result.current
  view.unmount()
  return seen
}

async function flush() {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

describe("BrowserEventsBridge", () => {
  beforeEach(() => {
    mocks.handlers.clear()
    mocks.unsubscribed.length = 0
    mocks.subscribe.mockClear()
    mocks.adoptBrowserTab.mockClear()
    mocks.closeFileTab.mockClear()
    mocks.openBrowserTab.mockClear()
    mocks.browserClose.mockClear()
    mocks.browserListDownloads.mockClear()
    mocks.browserSetHostRules.mockClear()
    resetBrowserTabStoreForTests()
    resetBrowserDownloadsForTests()
    resetBrowserPrefsForTests()
  })
  afterEach(() => {
    resetBrowserTabStoreForTests()
    resetBrowserDownloadsForTests()
    resetBrowserPrefsForTests()
  })

  it("subscribes to the streams once the capabilities say a browser exists, after sweeping orphans", async () => {
    const { unmount } = render(<BrowserEventsBridge />)
    await flush()
    // Surfaces left over from a previous document are closed first.
    expect(mocks.browserClose).toHaveBeenCalledWith("stale-1")
    expect(mocks.browserClose).toHaveBeenCalledWith("stale-2")
    expect([...mocks.handlers.keys()].sort()).toEqual([
      "browser://closed",
      "browser://download",
      "browser://navigation-blocked",
      "browser://open-request",
      "browser://popup",
      "browser://shortcut",
      "browser://state",
    ])
    // Downloads already running when this document mounted are shown again.
    expect(getBrowserDownloads().map((d) => d.id)).toEqual(["dl-1"])
    mocks.handlers.get("browser://download")!({
      id: "dl-2",
      tabId: "t1",
      url: "https://example.com/b.bin",
      fileName: "b.bin",
      path: "/Users/dev/Downloads/b.bin",
      state: "started",
    })
    expect(getBrowserDownloads().map((d) => d.id)).toEqual(["dl-2", "dl-1"])

    // ⌘F inside the page reaches the tab's find bar; anything else the page
    // claims is dropped by the host, and an unknown name changes nothing.
    expect(findRequestOf("browser:abc")).toBe(0)
    mocks.handlers.get("browser://shortcut")!({
      tabId: "abc",
      shortcut: "find",
    })
    expect(findRequestOf("browser:abc")).toBe(1)
    mocks.handlers.get("browser://shortcut")!({
      tabId: "abc",
      shortcut: "quit",
    })
    expect(findRequestOf("browser:abc")).toBe(1)

    mocks.handlers.get("browser://open-request")!({
      url: "https://example.com/from-agent",
      source: "agent",
      activate: false,
      ownerWindow: null,
    })
    expect(mocks.openBrowserTab).toHaveBeenCalledWith(
      "https://example.com/from-agent",
      { activate: false }
    )
    mocks.handlers.get("browser://open-request")!({
      url: "https://example.com/other-window",
      source: "agent",
      activate: true,
      ownerWindow: "remote-workspace-3",
    })
    expect(mocks.openBrowserTab).toHaveBeenCalledTimes(1)
    // A modifier-click names its opener; the workspace id is derived here so
    // the context can place the new tab right after it.
    mocks.handlers.get("browser://open-request")!({
      url: "https://example.com/next-to-opener",
      source: "modifier-click",
      activate: false,
      ownerWindow: "main",
      openerTabId: "abc",
    })
    expect(mocks.openBrowserTab).toHaveBeenCalledTimes(2)
    expect(mocks.openBrowserTab).toHaveBeenLastCalledWith(
      "https://example.com/next-to-opener",
      { activate: false, openerTabId: "browser:abc" }
    )

    mocks.handlers.get("browser://state")!({
      tabId: "abc",
      ownerWindow: "main",
      surface: "child",
      channel: "native",
      url: "https://example.com/",
      requestedUrl: "https://example.com/",
      title: "Example",
      favicon: null,
      loading: false,
      canGoBack: false,
      canGoForward: false,
      origin: "https://example.com",
      zoom: 1,
      error: null,
      remoteHost: null,
      openerTabId: null,
    })
    expect(getBrowserTabState("browser:abc")?.title).toBe("Example")

    mocks.handlers.get("browser://popup")!({
      presentation: "adopted",
      openerTabId: "abc",
      tabId: "abc-p1",
      url: "https://example.com/popup",
      requestedSize: null,
      reason: null,
    })
    expect(mocks.adoptBrowserTab).toHaveBeenCalledWith({
      backendTabId: "abc-p1",
      url: "https://example.com/popup",
      openerBackendTabId: "abc",
    })
    mocks.handlers.get("browser://popup")!({
      presentation: "denied",
      openerTabId: "abc",
      tabId: null,
      url: "https://example.com/blocked",
      requestedSize: null,
      reason: "no-gesture",
    })
    expect(mocks.adoptBrowserTab).toHaveBeenCalledTimes(1)

    setBrowserTabState({
      tabId: "abc-p1",
      ownerWindow: "main",
      surface: "child",
      channel: "native",
      url: "",
      requestedUrl: "https://example.com/popup",
      title: "",
      favicon: null,
      loading: true,
      canGoBack: false,
      canGoForward: false,
      origin: null,
      zoom: 1,
      error: null,
      remoteHost: null,
      openerTabId: "abc",
    })
    mocks.handlers.get("browser://closed")!({
      tabId: "abc-p1",
      ownerWindow: "main",
    })
    expect(getBrowserTabState("browser:abc-p1")).toBeNull()
    expect(mocks.closeFileTab).toHaveBeenCalledWith("browser:abc-p1")

    unmount()
    expect(mocks.unsubscribed.sort()).toEqual([
      "browser://closed",
      "browser://download",
      "browser://navigation-blocked",
      "browser://open-request",
      "browser://popup",
      "browser://shortcut",
      "browser://state",
    ])
  })

  it("stays silent when no built-in browser is available", async () => {
    mocks.capabilities.mockResolvedValueOnce({
      available: false,
      surface: null,
      platform: "web",
      channel: "degraded",
      reasons: ["web"],
      isolatedStorage: false,
      proxy: { url: null, applies: "unsupported", reason: null },
      downloadsDir: "/Users/dev/Downloads",
      policy: { enabled: true, managedRules: [], managedSource: null },
    })
    render(<BrowserEventsBridge />)
    await flush()
    expect(mocks.subscribe).not.toHaveBeenCalled()
  })

  it("pushes the user's site rules to the backend at start and whenever they change", async () => {
    const { unmount } = render(<BrowserEventsBridge />)
    await flush()
    expect(mocks.browserSetHostRules).toHaveBeenCalledWith([])
    await act(async () => {
      setBrowserHostRules([{ pattern: "blocked.example", action: "block" }])
    })
    expect(mocks.browserSetHostRules).toHaveBeenLastCalledWith([
      { pattern: "blocked.example", action: "block" },
    ])
    unmount()
    // After unmount the preference subscription is gone with the rest.
    mocks.browserSetHostRules.mockClear()
    await act(async () => {
      setBrowserHostRules([])
    })
    expect(mocks.browserSetHostRules).not.toHaveBeenCalled()
  })

  it("turns a refused navigation into a notice on its tab", async () => {
    render(<BrowserEventsBridge />)
    await flush()
    mocks.handlers.get("browser://navigation-blocked")!({
      tabId: "abc",
      url: "https://blocked.example/",
      reason: "host-rule",
    })
    const view = renderHook(() => useBrowserTabNotice("browser:abc"))
    expect(view.result.current).toEqual({
      kind: "navigation-blocked",
      url: "https://blocked.example/",
      reason: "host-rule",
    })
    view.unmount()
  })
})
