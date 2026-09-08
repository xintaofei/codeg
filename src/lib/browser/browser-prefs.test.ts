import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  DEFAULT_BROWSER_PREFS,
  getBrowserPrefs,
  markBrowserFirstOpenSeen,
  resetBrowserPrefsForTests,
  setAllDefaultLinkTargets,
  setBrowserDevtools,
  setBrowserHostRules,
  setBrowserSurfaceOverride,
  setBrowserTerminalClickMenu,
  setDefaultLinkTarget,
  subscribeBrowserPrefs,
  useBrowserPrefs,
} from "./browser-prefs"

/** Invalidate the in-memory snapshot the way a cross-window change does,
 *  without touching storage. The subscription that clears the cache only
 *  exists while someone listens, hence the throwaway subscriber. */
function resetCacheOnly() {
  const unsubscribe = subscribeBrowserPrefs(() => {})
  window.dispatchEvent(
    new StorageEvent("storage", { key: "browser:host-rules" })
  )
  unsubscribe()
}

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

  it("stores the site-rule table under one key and drops junk on read", () => {
    expect(getBrowserPrefs().hostRules).toEqual([])
    setBrowserHostRules([
      { pattern: "*.corp.example", action: "builtin" },
      { pattern: "blocked.example", action: "block" },
    ])
    expect(getBrowserPrefs().hostRules).toEqual([
      { pattern: "*.corp.example", action: "builtin" },
      { pattern: "blocked.example", action: "block" },
    ])
    expect(localStorage.getItem("browser:host-rules")).not.toBeNull()

    // A hand-edited or corrupt entry costs that entry, not the table.
    localStorage.setItem(
      "browser:host-rules",
      JSON.stringify([
        { pattern: "ok.example", action: "system" },
        { pattern: "", action: "block" },
        { pattern: "x.example", action: "explode" },
        "junk",
      ])
    )
    resetCacheOnly()
    expect(getBrowserPrefs().hostRules).toEqual([
      { pattern: "ok.example", action: "system" },
    ])
    localStorage.setItem("browser:host-rules", "{ not json")
    resetCacheOnly()
    expect(getBrowserPrefs().hostRules).toEqual([])

    // An empty table removes the key rather than storing `[]`.
    setBrowserHostRules([])
    expect(localStorage.getItem("browser:host-rules")).toBeNull()
  })

  it("stores the terminal link-menu switch under its own key, off by default", () => {
    expect(getBrowserPrefs().terminalClickMenu).toBe(false)
    setBrowserTerminalClickMenu(true)
    expect(localStorage.getItem("browser:terminal-click-menu")).toBe("true")
    expect(getBrowserPrefs().terminalClickMenu).toBe(true)
    // Off is the default, so off removes the key rather than storing it.
    setBrowserTerminalClickMenu(false)
    expect(localStorage.getItem("browser:terminal-click-menu")).toBeNull()
    expect(getBrowserPrefs().terminalClickMenu).toBe(false)
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
