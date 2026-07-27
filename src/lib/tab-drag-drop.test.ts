import { describe, expect, it } from "vitest"
import { clientPointFromDrag, dropIndexFromMidpoints } from "./tab-drag-drop"

describe("dropIndexFromMidpoints", () => {
  const midpoints = [100, 200, 300]

  it("drops before the first tab when the cursor is left of every midpoint", () => {
    expect(dropIndexFromMidpoints(50, midpoints)).toBe(0)
  })

  it("drops between tabs by midpoint comparison", () => {
    expect(dropIndexFromMidpoints(150, midpoints)).toBe(1)
    expect(dropIndexFromMidpoints(250, midpoints)).toBe(2)
  })

  it("appends when the cursor is right of every midpoint", () => {
    expect(dropIndexFromMidpoints(350, midpoints)).toBe(3)
  })

  it("returns 0 for an empty strip", () => {
    expect(dropIndexFromMidpoints(123, [])).toBe(0)
  })
})

describe("clientPointFromDrag", () => {
  it("prefers the event's own client coordinates", () => {
    expect(
      clientPointFromDrag(
        { clientX: 12, clientY: 34 },
        { point: { x: 999, y: 999 } }
      )
    ).toEqual({ x: 12, y: 34 })
  })

  it("falls back to page coords minus scroll when the event has none", () => {
    // jsdom: scrollX/scrollY are 0.
    expect(clientPointFromDrag({}, { point: { x: 40, y: 50 } })).toEqual({
      x: 40,
      y: 50,
    })
    expect(clientPointFromDrag(null, { point: { x: 7, y: 8 } })).toEqual({
      x: 7,
      y: 8,
    })
  })
})
