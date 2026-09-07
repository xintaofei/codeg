import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const api = vi.hoisted(() => ({
  browserClose: vi.fn(() => Promise.resolve()),
  isDesktop: vi.fn(() => true),
}))
vi.mock("./browser-api", () => ({ browserClose: api.browserClose }))
vi.mock("@/lib/transport", () => ({ isDesktop: api.isDesktop }))

import {
  browserWorkspaceTabId,
  getBrowserTabState,
  releaseBrowserTab,
  removeBrowserTabState,
  resetBrowserTabStoreForTests,
  setBrowserTabState,
  subscribeBrowserTabs,
  useBrowserTabState,
} from "./browser-tab-store"
import type { BrowserTabState } from "./types"

function state(over: Partial<BrowserTabState> = {}): BrowserTabState {
  return {
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
    ...over,
  }
}

describe("browser tab store", () => {
  beforeEach(() => {
    resetBrowserTabStoreForTests()
    api.browserClose.mockClear()
    api.isDesktop.mockReturnValue(true)
  })
  afterEach(() => resetBrowserTabStoreForTests())

  it("keys state by the workspace tab id derived from the backend id", () => {
    setBrowserTabState(state())
    expect(browserWorkspaceTabId("abc")).toBe("browser:abc")
    expect(getBrowserTabState("browser:abc")?.title).toBe("Example")
    expect(getBrowserTabState("browser:zzz")).toBeNull()
  })

  it("notifies subscribers only when something actually changed", () => {
    const listener = vi.fn()
    subscribeBrowserTabs(listener)
    setBrowserTabState(state())
    setBrowserTabState(state())
    expect(listener).toHaveBeenCalledTimes(1)
    setBrowserTabState(state({ loading: true }))
    expect(listener).toHaveBeenCalledTimes(2)
    const first = getBrowserTabState("browser:abc")
    setBrowserTabState(state({ loading: true }))
    expect(getBrowserTabState("browser:abc")).toBe(first)
    removeBrowserTabState("browser:abc")
    expect(listener).toHaveBeenCalledTimes(3)
    removeBrowserTabState("browser:abc")
    expect(listener).toHaveBeenCalledTimes(3)
  })

  it("useBrowserTabState re-renders for its own tab only", () => {
    const { result, rerender } = renderHook(
      ({ id }: { id: string | null }) => useBrowserTabState(id),
      { initialProps: { id: "browser:abc" as string | null } }
    )
    expect(result.current).toBeNull()
    act(() => setBrowserTabState(state()))
    expect(result.current?.url).toBe("https://example.com/")
    act(() => setBrowserTabState(state({ tabId: "other", url: "https://o/" })))
    expect(result.current?.url).toBe("https://example.com/")
    rerender({ id: null })
    expect(result.current).toBeNull()
  })

  it("releaseBrowserTab forgets the state and closes the backend surface on desktop", () => {
    setBrowserTabState(state())
    releaseBrowserTab("browser:abc")
    expect(getBrowserTabState("browser:abc")).toBeNull()
    expect(api.browserClose).toHaveBeenCalledWith("abc")
    api.isDesktop.mockReturnValue(false)
    releaseBrowserTab("browser:abc")
    expect(api.browserClose).toHaveBeenCalledTimes(1)
    releaseBrowserTab("file:%2Fx")
    expect(api.browserClose).toHaveBeenCalledTimes(1)
  })
})
