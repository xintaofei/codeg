import { afterEach, beforeEach, describe, expect, it } from "vitest"

import { detectEnvironment } from "./detect"

describe("detectEnvironment", () => {
  // jsdom-provided `window` is the only global we tinker with. Snapshot the
  // original `__TAURI_INTERNALS__` (likely undefined) and restore in afterEach.
  let hadInternals: boolean
  let originalInternals: unknown
  let originalUserAgent: PropertyDescriptor | undefined

  beforeEach(() => {
    hadInternals = "__TAURI_INTERNALS__" in window
    originalInternals = (window as unknown as Record<string, unknown>)
      .__TAURI_INTERNALS__
    originalUserAgent = Object.getOwnPropertyDescriptor(navigator, "userAgent")
  })

  afterEach(() => {
    const w = window as unknown as Record<string, unknown>
    if (hadInternals) {
      w.__TAURI_INTERNALS__ = originalInternals
    } else {
      delete w.__TAURI_INTERNALS__
    }
    if (originalUserAgent) {
      Object.defineProperty(navigator, "userAgent", originalUserAgent)
    }
    localStorage.clear()
  })

  it("returns 'browser-remote' by default in jsdom", () => {
    const w = window as unknown as Record<string, unknown>
    delete w.__TAURI_INTERNALS__
    expect(detectEnvironment()).toBe("browser-remote")
  })

  it("returns 'desktop-local' for a desktop Tauri runtime", () => {
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: () => {},
    }
    expect(detectEnvironment()).toBe("desktop-local")
  })

  it("returns an explicit mobile role for an Android Tauri runtime", () => {
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: () => {},
    }
    Object.defineProperty(navigator, "userAgent", {
      configurable: true,
      value: "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36",
    })

    expect(detectEnvironment()).toBe("mobile-direct")
    localStorage.setItem("codeg_mobile_connection_mode", "relay")
    expect(detectEnvironment()).toBe("mobile-relay")
  })
})
