import { describe, expect, it } from "vitest"
import {
  backgroundPositionFor,
  filmstripFrameCount,
  spriteBackgroundSize,
  spriteRowsFromHeight,
} from "./animation"

describe("spriteRowsFromHeight", () => {
  it("derives rows from the sheet height", () => {
    expect(spriteRowsFromHeight(1872)).toBe(9) // v1
    expect(spriteRowsFromHeight(2288)).toBe(11) // v2
  })

  it("never reports fewer than the 9 base states", () => {
    expect(spriteRowsFromHeight(1664)).toBe(9) // 8 rows → clamped up
    expect(spriteRowsFromHeight(0)).toBe(9)
    expect(spriteRowsFromHeight(null)).toBe(9)
    expect(spriteRowsFromHeight(undefined)).toBe(9)
    expect(spriteRowsFromHeight(-100)).toBe(9)
  })

  it("rounds near-multiples to the nearest whole row", () => {
    expect(spriteRowsFromHeight(2287)).toBe(11)
    expect(spriteRowsFromHeight(2289)).toBe(11)
  })
})

describe("spriteBackgroundSize", () => {
  it("scales height by the row count", () => {
    expect(spriteBackgroundSize(9)).toBe("800% 900%")
    expect(spriteBackgroundSize(11)).toBe("800% 1100%")
  })

  it("defaults to the base 9-row layout", () => {
    expect(spriteBackgroundSize()).toBe("800% 900%")
  })
})

describe("backgroundPositionFor", () => {
  it("places the top-left cell at the origin regardless of rows", () => {
    expect(backgroundPositionFor(0, 0)).toBe("0% 0%")
    expect(backgroundPositionFor(0, 0, 11)).toBe("0% 0%")
  })

  it("places the last cell at 100% for the given row count", () => {
    expect(backgroundPositionFor(8, 7, 9)).toBe("100% 100%")
    expect(backgroundPositionFor(10, 7, 11)).toBe("100% 100%")
  })

  it("keeps a shared state aligned across row counts", () => {
    // 'failed' is row 5 in both v1 and v2; its vertical position must differ
    // because the denominator (rows-1) differs.
    expect(backgroundPositionFor(5, 0, 9)).toBe(`0% ${(5 / 8) * 100}%`)
    expect(backgroundPositionFor(5, 0, 11)).toBe("0% 50%")
  })
})

describe("filmstripFrameCount", () => {
  it("counts frames from a horizontal preview strip", () => {
    expect(filmstripFrameCount(5472, 104)).toBe(57) // v1 filmstrip
    expect(filmstripFrameCount(7008, 104)).toBe(73) // v2 filmstrip
  })

  it("returns 0 when the size is unknown", () => {
    expect(filmstripFrameCount(0, 0)).toBe(0)
    expect(filmstripFrameCount(100, 0)).toBe(0)
    expect(filmstripFrameCount(-1, 104)).toBe(0)
  })
})
