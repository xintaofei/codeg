import { describe, expect, it } from "vitest"

import { DEFAULT_ZOOM_LEVEL, stepZoom } from "./theme-presets"

describe("stepZoom", () => {
  it("walks the Settings rungs and stops at the ends", () => {
    expect(stepZoom(100, 1)).toBe(110)
    expect(stepZoom(110, -1)).toBe(100)
    expect(stepZoom(80, -1)).toBe(80)
    expect(stepZoom(150, 1)).toBe(175)
    expect(stepZoom(300, 1)).toBe(300)
    expect(stepZoom(DEFAULT_ZOOM_LEVEL, 1)).toBe(110)
  })
})
