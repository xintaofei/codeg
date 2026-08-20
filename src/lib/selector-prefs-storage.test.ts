import { afterEach, describe, expect, it } from "vitest"

import {
  getSavedModeId,
  saveModeIdPreference,
  saveModePreference,
} from "./selector-prefs-storage"

const STORAGE_KEY = "codeg:selector-prefs"

afterEach(() => {
  localStorage.removeItem(STORAGE_KEY)
})

describe("saveModeIdPreference", () => {
  it("overwrites a stickier last-used mode so Settings=default wins over saved bypass", () => {
    saveModeIdPreference("claude_code", "bypassPermissions")
    expect(getSavedModeId("claude_code")).toBe("bypassPermissions")
    saveModeIdPreference("claude_code", "default")
    expect(getSavedModeId("claude_code")).toBe("default")
  })

  it("overwrites the inverse: Settings=bypass wins over saved default", () => {
    saveModeIdPreference("claude_code", "default")
    saveModeIdPreference("claude_code", "bypassPermissions")
    expect(getSavedModeId("claude_code")).toBe("bypassPermissions")
  })

  it("ignores empty so Use default does not wipe the composer last-used", () => {
    saveModeIdPreference("claude_code", "acceptEdits")
    saveModeIdPreference("claude_code", "   ")
    expect(getSavedModeId("claude_code")).toBe("acceptEdits")
  })

  it("is the same store saveModePreference writes", () => {
    saveModePreference("claude_code", {
      current_mode_id: "auto",
      available_modes: [],
    })
    expect(getSavedModeId("claude_code")).toBe("auto")
  })
})
