import { act, render } from "@testing-library/react"
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
        })
    ),
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
  }
})

vi.mock("@/lib/browser/browser-api", () => ({
  browserCapabilities: mocks.capabilities,
  browserClose: mocks.browserClose,
  browserListTabs: mocks.browserListTabs,
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
  getBrowserTabState,
  resetBrowserTabStoreForTests,
  setBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import { BrowserEventsBridge } from "./browser-events-bridge"

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
    resetBrowserTabStoreForTests()
  })
  afterEach(() => resetBrowserTabStoreForTests())

  it("subscribes to the streams once the capabilities say a browser exists, after sweeping orphans", async () => {
    const { unmount } = render(<BrowserEventsBridge />)
    await flush()
    // Surfaces left over from a previous document are closed first.
    expect(mocks.browserClose).toHaveBeenCalledWith("stale-1")
    expect(mocks.browserClose).toHaveBeenCalledWith("stale-2")
    expect([...mocks.handlers.keys()].sort()).toEqual([
      "browser://closed",
      "browser://open-request",
      "browser://popup",
      "browser://state",
    ])

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
      "browser://open-request",
      "browser://popup",
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
    })
    render(<BrowserEventsBridge />)
    await flush()
    expect(mocks.subscribe).not.toHaveBeenCalled()
  })
})
