import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const api = vi.hoisted(() => ({
  browserClose: vi.fn(() => Promise.resolve()),
  isDesktop: vi.fn(() => true),
}))
vi.mock("./browser-api", () => ({ browserClose: api.browserClose }))
vi.mock("@/lib/transport", () => ({ isDesktop: api.isDesktop }))

import {
  browserTabHiddenAt,
  browserWorkspaceTabId,
  claimSurfaceCreation,
  forgetSurfaceCreation,
  hasSurfaceClaim,
  runSurfaceOp,
  surfaceClaimIsCurrent,
  getBrowserTabState,
  markBrowserTabHidden,
  markBrowserTabShown,
  releaseBrowserTab,
  recordBrowserAgentActivity,
  removeBrowserTabState,
  resetBrowserTabStoreForTests,
  setBrowserTabState,
  subscribeBrowserTabs,
  useBrowserAgentActivity,
  useBrowserTabState,
} from "./browser-tab-store"
import type { AgentActivityPayload, AgentGrant, BrowserTabState } from "./types"

function state(over: Partial<BrowserTabState> = {}): BrowserTabState {
  return {
    tabId: "abc",
    ownerWindow: "main",
    kind: "page",
    surface: "child",
    channel: "native",
    channelError: null,
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
    profile: "default",
    agentGrant: null,
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

  /// The nested fields arrive as fresh objects in every event, so identity
  /// cannot be the test. A tab whose page is loading emits a stream of
  /// `browser://state`, and an unchanged grant in each of them must not make
  /// every one of them look like news.
  it("an unchanged nested field is not a change", () => {
    const grant: AgentGrant = {
      level: "read",
      origin: "https://example.com",
      grantedAt: 1,
    }
    const listener = vi.fn()
    subscribeBrowserTabs(listener)
    setBrowserTabState(state({ agentGrant: { ...grant } }))
    expect(listener).toHaveBeenCalledTimes(1)
    setBrowserTabState(state({ agentGrant: { ...grant } }))
    expect(listener).toHaveBeenCalledTimes(1)
    // …and a real one still is, in both directions.
    setBrowserTabState(state({ agentGrant: { ...grant, level: "control" } }))
    expect(listener).toHaveBeenCalledTimes(2)
    setBrowserTabState(state({ agentGrant: null }))
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

  // The ledger is what stops a StrictMode double effect (or a re-mounted
  // host) from creating a second webview for one tab.
  it("hands the surface-creation claim to exactly one caller until released", () => {
    const token = claimSurfaceCreation("abc")
    expect(token).not.toBeNull()
    expect(claimSurfaceCreation("abc")).toBeNull()
    expect(surfaceClaimIsCurrent("abc", token!)).toBe(true)

    forgetSurfaceCreation("abc")
    // The old holder can now tell that the surface it is building is nobody's.
    expect(surfaceClaimIsCurrent("abc", token!)).toBe(false)
    expect(hasSurfaceClaim("abc")).toBe(false)
    const next = claimSurfaceCreation("abc")
    expect(next).not.toBeNull()
    expect(next).not.toBe(token)
    // A claim taken meanwhile does not make the old token current again.
    expect(surfaceClaimIsCurrent("abc", token!)).toBe(false)
  })

  // Releasing a tab returns it to "not loaded": the next host that mounts
  // for it creates a fresh surface. This is what makes a suspended (or
  // restored) tab resumable through the same code path.
  it("releasing a tab frees its claim", () => {
    setBrowserTabState(state())
    const token = claimSurfaceCreation("abc")
    expect(token).not.toBeNull()
    releaseBrowserTab("browser:abc")
    expect(surfaceClaimIsCurrent("abc", token!)).toBe(false)
    expect(claimSurfaceCreation("abc")).not.toBeNull()
  })

  // A backend tab id is reused across generations (suspend, then show
  // again). Without ordering, the close issued for the old generation could
  // reach the backend after the new one registered and destroy it.
  it("runs the create and destroy calls of one tab id in order", async () => {
    const order: string[] = []
    const settle: Array<() => void> = []
    const op = (name: string) => () =>
      new Promise<void>((resolve) => {
        order.push(`${name}:start`)
        settle.push(() => {
          order.push(`${name}:done`)
          resolve()
        })
      })

    const first = runSurfaceOp("abc", op("close"))
    const second = runSurfaceOp("abc", op("open"))
    // The second has not even started: it is waiting on the first.
    expect(order).toEqual(["close:start"])

    settle[0]()
    await first
    await Promise.resolve()
    expect(order).toEqual(["close:start", "close:done", "open:start"])
    settle[1]()
    await second
    expect(order).toEqual([
      "close:start",
      "close:done",
      "open:start",
      "open:done",
    ])

    // A different tab id is an independent chain.
    let otherStarted = false
    void runSurfaceOp("xyz", () => {
      otherStarted = true
      return Promise.resolve()
    })
    await Promise.resolve()
    expect(otherStarted).toBe(true)
  })

  // A failed op must not stall everything queued behind it.
  it("keeps the chain moving after a failed op", async () => {
    const failed = runSurfaceOp("abc", () => Promise.reject(new Error("nope")))
    await expect(failed).rejects.toThrow("nope")
    await expect(
      runSurfaceOp("abc", () => Promise.resolve("ok"))
    ).resolves.toBe("ok")
  })

  describe("agent activity", () => {
    const read = (over: Partial<AgentActivityPayload> = {}) =>
      act(() =>
        recordBrowserAgentActivity({
          tabId: "abc",
          action: "read",
          outcome: "done",
          at: 1,
          ...over,
        })
      )

    it("gives a tab with no activity the same empty list every time", () => {
      const view = renderHook(() => useBrowserAgentActivity("browser:abc"))
      const first = view.result.current
      expect(first).toEqual([])
      // A fresh array each read would make `useSyncExternalStore` believe the
      // store had changed on every notification, forever.
      act(() =>
        recordBrowserAgentActivity({
          tabId: "other",
          action: "read",
          outcome: "done",
          at: 1,
        })
      )
      expect(view.result.current).toBe(first)
      view.unmount()
    })

    it("folds a run of identical attempts into one line and keeps the newest time", () => {
      const view = renderHook(() => useBrowserAgentActivity("browser:abc"))
      read({ at: 100 })
      read({ at: 200 })
      read({ at: 300 })
      expect(view.result.current).toEqual([
        { action: "read", outcome: "done", at: 300, count: 3 },
      ])
      // A different outcome is a different line, and goes on top.
      read({ at: 400, outcome: "refused" })
      read({ at: 500 })
      expect(view.result.current).toEqual([
        { action: "read", outcome: "done", at: 500, count: 1 },
        { action: "read", outcome: "refused", at: 400, count: 1 },
        { action: "read", outcome: "done", at: 300, count: 3 },
      ])
      view.unmount()
    })

    it("keeps only the most recent lines", () => {
      const view = renderHook(() => useBrowserAgentActivity("browser:abc"))
      // Alternating outcomes so nothing folds: 120 distinct lines offered.
      for (let i = 0; i < 120; i += 1) {
        read({ at: i, outcome: i % 2 === 0 ? "done" : "refused" })
      }
      expect(view.result.current).toHaveLength(50)
      expect(view.result.current[0]?.at).toBe(119)
      view.unmount()
    })

    it("forgets a tab's activity with the tab", () => {
      const view = renderHook(() => useBrowserAgentActivity("browser:abc"))
      read()
      expect(view.result.current).toHaveLength(1)
      act(() => removeBrowserTabState("browser:abc"))
      expect(view.result.current).toEqual([])
      view.unmount()
    })
  })

  it("stamps when a tab left the screen", () => {
    expect(browserTabHiddenAt("browser:abc")).toBeUndefined()
    markBrowserTabShown("browser:abc")
    expect(browserTabHiddenAt("browser:abc")).toBeNull()
    markBrowserTabHidden("browser:abc")
    expect(typeof browserTabHiddenAt("browser:abc")).toBe("number")
    // Forgetting the tab forgets the stamp with it.
    removeBrowserTabState("browser:abc")
    expect(browserTabHiddenAt("browser:abc")).toBeUndefined()
  })
})
