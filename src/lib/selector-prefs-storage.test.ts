import { beforeEach, describe, expect, it } from "vitest"

import {
  getSavedModeId,
  getSavedPrefsForConnect,
  saveConfigPreference,
  saveModePreference,
} from "./selector-prefs-storage"

const STORAGE_KEY = "codeg:selector-prefs"

beforeEach(() => {
  localStorage.clear()
})

describe("selector preference persistence", () => {
  it("round-trips mode and config preferences", () => {
    saveModePreference("codex", {
      current_mode_id: "plan",
      available_modes: [],
    })
    saveConfigPreference("codex", "reasoning_effort", "high")

    expect(getSavedModeId("codex")).toBe("plan")
    expect(getSavedPrefsForConnect("codex")).toEqual({
      modeId: "plan",
      configValues: { reasoning_effort: "high" },
    })
  })

  it.each(["{not json", "null", "[]", '"text"', "42"])(
    "falls back safely for an invalid top-level value: %s",
    (stored) => {
      localStorage.setItem(STORAGE_KEY, stored)

      expect(getSavedModeId("codex")).toBeNull()
      expect(getSavedPrefsForConnect("codex")).toEqual({
        modeId: null,
        configValues: null,
      })

      expect(() =>
        saveModePreference("codex", {
          current_mode_id: "default",
          available_modes: [],
        })
      ).not.toThrow()
      expect(getSavedModeId("codex")).toBe("default")
    }
  )

  it("keeps valid nested fields and drops values with the wrong type", () => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        codex: {
          modeId: 7,
          configValues: {
            reasoning_effort: "medium",
            approval_policy: false,
          },
        },
        claude_code: ["plan"],
      })
    )

    expect(getSavedPrefsForConnect("codex")).toEqual({
      modeId: null,
      configValues: { reasoning_effort: "medium" },
    })
    expect(getSavedPrefsForConnect("claude_code")).toEqual({
      modeId: null,
      configValues: null,
    })
  })
})
