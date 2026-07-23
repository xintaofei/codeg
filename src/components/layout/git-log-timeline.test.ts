import { describe, expect, it } from "vitest"
import { parseDate } from "./git-log-timeline"

describe("parseDate", () => {
  it("parses a valid ISO string into a Date", () => {
    const parsed = parseDate("2026-07-20T10:30:00.000Z")
    expect(parsed).toBeInstanceOf(Date)
    expect(parsed?.toISOString()).toBe("2026-07-20T10:30:00.000Z")
  })

  it("returns null for an unparseable string", () => {
    expect(parseDate("not a date")).toBeNull()
  })
})
