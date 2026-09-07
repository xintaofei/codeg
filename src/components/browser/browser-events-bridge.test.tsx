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
  }
})

vi.mock("@/lib/browser/browser-api", () => ({
  browserCapabilities: mocks.capabilities,
  browserClose: vi.fn(() => Promise.resolve()),
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ subscribe: mocks.subscribe }),
  isDesktop: () => true,
}))
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({
    adoptBrowserTab: mocks.adoptBrowserTab,
    closeFileTab: mocks.closeFileTab,
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
    resetBrowserTabStoreForTests()
  })
  afterEach(() => resetBrowserTabStoreForTests())

  it("subscribes to the three streams once the capabilities say a browser exists", async () => {
    const { unmount } = render(<BrowserEventsBridge />)
    await flush()
    expect([...mocks.handlers.keys()].sort()).toEqual([
      "browser://closed",
      "browser://popup",
      "browser://state",
    ])

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
    })
    render(<BrowserEventsBridge />)
    await flush()
    expect(mocks.subscribe).not.toHaveBeenCalled()
  })
})
