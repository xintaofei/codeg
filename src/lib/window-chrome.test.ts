import { describe, expect, it } from "vitest"

import {
  LEFT_CHROME_CLUSTER,
  MAC_TRAFFIC_LIGHT_INSET,
  RIGHT_EDGE_RAIL_WIDTH,
  WINDOW_CAPTION_WIDTH,
  captionOverhangPastRail,
  leftChromeReserve,
} from "./window-chrome"

// The app "zoom" scales the root font-size (rem), so the rem-sized chrome
// buttons grow with zoom. Their fixed-px containers must grow by the same
// factor or the buttons overflow/clip at high zoom (the 150% bug). These guard
// that only the DOM button CLUSTER / rail scale, while the native insets
// (macOS traffic-light clearance, Windows/Linux caption strip) stay fixed.
describe("window-chrome zoom scaling", () => {
  it("defaults to 100% (no scaling) and matches the pre-zoom baseline", () => {
    expect(captionOverhangPastRail(false)).toBe(0)
    // Caption strip (138) minus the rail's own width (40) = 98 still overhangs
    // the right-edge column on Windows/Linux.
    expect(captionOverhangPastRail(true)).toBe(
      WINDOW_CAPTION_WIDTH - RIGHT_EDGE_RAIL_WIDTH
    )
    expect(leftChromeReserve(false)).toBe(LEFT_CHROME_CLUSTER)
    expect(leftChromeReserve(true)).toBe(
      MAC_TRAFFIC_LIGHT_INSET + LEFT_CHROME_CLUSTER
    )
  })

  it("scales only the rem-sized parts at 150%, leaving native insets fixed", () => {
    // 80 → 120.
    expect(leftChromeReserve(false, 150)).toBe(120)
    // Native traffic-light inset stays 76; only the 80 cluster scales to 120.
    expect(leftChromeReserve(true, 150)).toBe(MAC_TRAFFIC_LIGHT_INSET + 120)
    // The rail scales to 60 but the caption strip stays a fixed 138, so the
    // overhang SHRINKS with zoom (138 − 60 = 78).
    expect(captionOverhangPastRail(true, 150)).toBe(
      WINDOW_CAPTION_WIDTH - Math.round((RIGHT_EDGE_RAIL_WIDTH * 150) / 100)
    )
    expect(captionOverhangPastRail(true, 150)).toBe(78)
  })

  it("scales down below 100% too, rounds to whole pixels, and clamps at 0", () => {
    // 80 * 0.5 = 40, plus the fixed 76 inset.
    expect(leftChromeReserve(true, 50)).toBe(MAC_TRAFFIC_LIGHT_INSET + 40)
    // High zoom: once the (scaled) rail covers the whole caption strip there
    // is no overhang left to reserve — clamps at 0, never negative.
    expect(captionOverhangPastRail(true, 400)).toBe(0)
  })
})
