import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  DEFAULT_BROWSER_PREFS,
  getBrowserPrefs,
  markBrowserFirstOpenSeen,
  resetBrowserPrefsForTests,
  setAllDefaultLinkTargets,
  setBrowserDevtools,
  setBrowserSurfaceOverride,
  setDefaultLinkTarget,
  subscribeBrowserPrefs,
  useBrowserPrefs,
} from "./browser-prefs"

describe("browser prefs", () => {
  beforeEach(() => {
    resetBrowserPrefsForTests()
  })

  afterEach(() => {
    resetBrowserPrefsForTests()
  })

  it("defaults every source to the built-in browser", () => {
    expect(getBrowserPrefs()).toEqual(DEFAULT_BROWSER_PREFS)
  })

  it("returns the same snapshot object until something changes", () => {
    const a = getBrowserPrefs()
    expect(getBrowserPrefs()).toBe(a)
    setDefaultLinkTarget("terminal", "system")
    const b = getBrowserPrefs()
    expect(b).not.toBe(a)
    expect(b.defaultTarget.terminal).toBe("system")
    expect(b.defaultTarget.transcript).toBe("builtin")
  })

  it("persists each setting under its own key", () => {
    setDefaultLinkTarget("transcript", "system")
    setBrowserDevtools(true)
    setBrowserSurfaceOverride("window")
    markBrowserFirstOpenSeen()
    expect(localStorage.getItem("browser:default-target:transcript")).toBe(
      "system"
    )
    expect(localStorage.getItem("browser:devtools")).toBe("true")
    expect(localStorage.getItem("browser:surface-override")).toBe("window")
    expect(localStorage.getItem("browser:first-open-seen")).toBe("true")
    expect(getBrowserPrefs()).toMatchObject({
      devtools: true,
      surfaceOverride: "window",
      firstOpenSeen: true,
    })
  })

  it("removes the surface override key when set back to auto", () => {
    setBrowserSurfaceOverride("child")
    setBrowserSurfaceOverride("auto")
    expect(localStorage.getItem("browser:surface-override")).toBeNull()
    expect(getBrowserPrefs().surfaceOverride).toBe("auto")
  })

  it("falls back to defaults for unknown stored values", () => {
    localStorage.setItem("browser:default-target:terminal", "popup")
    localStorage.setItem("browser:surface-override", "iframe")
    expect(getBrowserPrefs().defaultTarget.terminal).toBe("builtin")
    expect(getBrowserPrefs().surfaceOverride).toBe("auto")
  })

  it("setAllDefaultLinkTargets flips every source", () => {
    setAllDefaultLinkTargets("system")
    expect(Object.values(getBrowserPrefs().defaultTarget)).toEqual([
      "system",
      "system",
      "system",
      "system",
      "system",
    ])
  })

  it("notifies subscribers on same-window writes and cross-window storage events", () => {
    const listener = vi.fn()
    const unsubscribe = subscribeBrowserPrefs(listener)
    setBrowserDevtools(true)
    expect(listener).toHaveBeenCalledTimes(1)

    // Another window wrote directly to localStorage: the cache must drop.
    localStorage.setItem("browser:devtools", "false")
    window.dispatchEvent(
      new StorageEvent("storage", { key: "browser:devtools" })
    )
    expect(listener).toHaveBeenCalledTimes(2)
    expect(getBrowserPrefs().devtools).toBe(false)

    // Unrelated keys are ignored.
    window.dispatchEvent(new StorageEvent("storage", { key: "other:key" }))
    expect(listener).toHaveBeenCalledTimes(2)

    unsubscribe()
    setBrowserDevtools(true)
    expect(listener).toHaveBeenCalledTimes(2)
  })

  it("useBrowserPrefs re-renders on change", () => {
    const { result } = renderHook(() => useBrowserPrefs())
    expect(result.current.defaultTarget.toolCard).toBe("builtin")
    act(() => {
      setDefaultLinkTarget("toolCard", "system")
    })
    expect(result.current.defaultTarget.toolCard).toBe("system")
  })
})
