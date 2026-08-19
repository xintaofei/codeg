import { describe, expect, it } from "vitest"

import {
  levelsFromMap,
  reasoningFromModel,
  reasoningToMap,
  toggleLevel,
  type PiModelReasoning,
} from "./pi-thinking"

/**
 * The read half has to agree with pi's `getSupportedThinkingLevels` byte for byte —
 * a level codeg shows but pi refuses is exactly the "picker snaps back" bug this
 * whole feature exists to kill.
 */
describe("levelsFromMap", () => {
  it("offers everything but xhigh for an undeclared map", () => {
    expect(levelsFromMap(undefined)).toEqual([
      "off",
      "minimal",
      "low",
      "medium",
      "high",
    ])
  })

  it("drops levels pinned to null", () => {
    expect(levelsFromMap({ minimal: null, medium: null })).toEqual([
      "off",
      "low",
      "high",
    ])
  })

  it("needs an explicit entry before xhigh appears", () => {
    expect(levelsFromMap({})).not.toContain("xhigh")
    expect(levelsFromMap({ xhigh: "xhigh" })).toContain("xhigh")
    expect(levelsFromMap({ xhigh: null })).not.toContain("xhigh")
  })

  it("reads pi's own built-in gpt-5.5 declaration", () => {
    expect(
      levelsFromMap({ off: "none", xhigh: "xhigh", minimal: null })
    ).toEqual(["off", "low", "medium", "high", "xhigh"])
  })
})

describe("reasoningToMap", () => {
  const base: PiModelReasoning = { enabled: true, levels: [], wireValues: {} }

  it("spells out only what it must for the full set", () => {
    expect(
      reasoningToMap({
        ...base,
        levels: ["off", "minimal", "low", "medium", "high", "xhigh"],
      })
      // minimal/low/medium/high are omitted: absent already means "offered,
      // sent verbatim". off and xhigh cannot be left to the default.
    ).toEqual({ off: "none", xhigh: "xhigh" })
  })

  it("pins every unchecked level to null and omits the checked ones", () => {
    expect(reasoningToMap({ ...base, levels: ["low", "high"] })).toEqual({
      off: null,
      minimal: null,
      medium: null,
      xhigh: null,
    })
  })

  it("writes custom wire values for a Google-style backend", () => {
    expect(
      reasoningToMap({
        ...base,
        levels: ["low", "high"],
        wireValues: { low: "LOW", high: "HIGH" },
      })
    ).toMatchObject({ low: "LOW", high: "HIGH" })
  })

  it("lets a custom wire value override the off/xhigh defaults", () => {
    expect(
      reasoningToMap({
        ...base,
        levels: ["off", "xhigh"],
        wireValues: { off: "disabled", xhigh: "ultra" },
      })
    ).toMatchObject({ off: "disabled", xhigh: "ultra" })
  })

  it("ignores a whitespace-only override", () => {
    expect(
      reasoningToMap({ ...base, levels: ["low"], wireValues: { low: "  " } })
        .low
    ).toBeUndefined()
  })
})

/**
 * Load → save must not drift: the `"none"` / `"xhigh"` markers the writer is forced to
 * emit have to read back as "no override", or every reopen of the panel would rewrite
 * the file and the advanced editor would show noise instead of placeholders.
 */
describe("round trip", () => {
  const cases: Record<string, PiModelReasoning> = {
    "full set": {
      enabled: true,
      levels: ["off", "minimal", "low", "medium", "high", "xhigh"],
      wireValues: {},
    },
    "narrow set": {
      enabled: true,
      levels: ["low", "medium", "high"],
      wireValues: {},
    },
    "google style": {
      enabled: true,
      levels: ["low", "high"],
      wireValues: { low: "LOW", high: "HIGH" },
    },
    "reasoning off": { enabled: false, levels: ["low"], wireValues: {} },
  }

  for (const [name, reasoning] of Object.entries(cases)) {
    it(`is stable for the ${name}`, () => {
      const map = reasoningToMap(reasoning)
      expect(reasoningFromModel(reasoning.enabled, map)).toEqual(reasoning)
      // A second save must produce the identical map.
      expect(
        reasoningToMap(reasoningFromModel(reasoning.enabled, map))
      ).toEqual(map)
    })
  }

  it("keeps the remembered levels when reasoning is switched off", () => {
    // The panel only flips the flag, so re-enabling restores the same chips.
    const map = reasoningToMap({
      enabled: false,
      levels: ["low", "medium"],
      wireValues: {},
    })
    expect(reasoningFromModel(false, map).levels).toEqual(["low", "medium"])
    expect(reasoningFromModel(true, map).enabled).toBe(true)
  })
})

describe("reasoningFromModel", () => {
  it("treats an absent reasoning flag as disabled", () => {
    expect(reasoningFromModel(undefined, undefined).enabled).toBe(false)
    expect(reasoningFromModel(null, undefined).enabled).toBe(false)
  })

  it("surfaces a wire value that differs from the implicit default", () => {
    expect(reasoningFromModel(true, { low: "LOW" }).wireValues).toEqual({
      low: "LOW",
    })
  })

  it("folds away a wire value that restates the default", () => {
    expect(
      reasoningFromModel(true, { off: "none", low: "low", xhigh: "xhigh" })
        .wireValues
    ).toEqual({})
  })
})

describe("toggleLevel", () => {
  it("keeps the canonical order regardless of click order", () => {
    let levels = toggleLevel([], "high")
    levels = toggleLevel(levels, "off")
    levels = toggleLevel(levels, "medium")
    expect(levels).toEqual(["off", "medium", "high"])
  })

  it("removes an active level", () => {
    expect(toggleLevel(["low", "high"], "low")).toEqual(["high"])
  })
})
