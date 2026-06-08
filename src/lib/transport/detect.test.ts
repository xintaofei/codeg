import { afterEach, beforeEach, describe, expect, it } from "vitest"

import { detectEnvironment } from "./detect"

describe("detectEnvironment", () => {
  // jsdom-provided `window` is the only global we tinker with. Snapshot the
  // original `__TAURI_INTERNALS__` (likely undefined) and restore in afterEach.
  let hadInternals: boolean
  let originalInternals: unknown

  beforeEach(() => {
    hadInternals = "__TAURI_INTERNALS__" in window
    originalInternals = (window as unknown as Record<string, unknown>)
      .__TAURI_INTERNALS__
  })

  afterEach(() => {
    const w = window as unknown as Record<string, unknown>
    if (hadInternals) {
      w.__TAURI_INTERNALS__ = originalInternals
    } else {
      delete w.__TAURI_INTERNALS__
    }
  })

  it("returns 'web' by default in jsdom", () => {
    const w = window as unknown as Record<string, unknown>
    delete w.__TAURI_INTERNALS__
    expect(detectEnvironment()).toBe("web")
  })

  it("returns 'tauri' when __TAURI_INTERNALS__ is present", () => {
    ;(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
      invoke: () => {},
    }
    expect(detectEnvironment()).toBe("tauri")
  })
})
