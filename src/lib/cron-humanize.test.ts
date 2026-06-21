import { describe, it, expect } from "vitest"
import { describeCron } from "./cron-humanize"

describe("describeCron", () => {
  it("recognizes every-N-minutes", () => {
    expect(describeCron("*/15 * * * *")).toEqual({
      kind: "everyMinutes",
      n: 15,
    })
    expect(describeCron("*/1 * * * *")).toEqual({ kind: "everyMinutes", n: 1 })
  })

  it("rejects out-of-range step values as raw", () => {
    expect(describeCron("*/0 * * * *")).toEqual({
      kind: "raw",
      cron: "*/0 * * * *",
    })
    expect(describeCron("*/90 * * * *")).toEqual({
      kind: "raw",
      cron: "*/90 * * * *",
    })
  })

  it("recognizes hourly for any in-range fixed minute", () => {
    expect(describeCron("0 * * * *")).toEqual({ kind: "hourly" })
    expect(describeCron("30 * * * *")).toEqual({ kind: "hourly" })
    expect(describeCron("59 * * * *")).toEqual({ kind: "hourly" })
  })

  it("falls back to raw for an out-of-range hourly minute", () => {
    expect(describeCron("60 * * * *")).toEqual({
      kind: "raw",
      cron: "60 * * * *",
    })
    expect(describeCron("99 * * * *")).toEqual({
      kind: "raw",
      cron: "99 * * * *",
    })
  })

  it("recognizes daily at a fixed time", () => {
    expect(describeCron("0 9 * * *")).toEqual({
      kind: "daily",
      hour: 9,
      minute: 0,
    })
    expect(describeCron("5 14 * * *")).toEqual({
      kind: "daily",
      hour: 14,
      minute: 5,
    })
  })

  it("recognizes weekdays (1-5)", () => {
    expect(describeCron("0 9 * * 1-5")).toEqual({
      kind: "weekdays",
      hour: 9,
      minute: 0,
    })
  })

  it("recognizes weekly on a single weekday and normalizes 7→0", () => {
    expect(describeCron("0 9 * * 1")).toEqual({
      kind: "weekly",
      dow: 1,
      hour: 9,
      minute: 0,
    })
    expect(describeCron("0 9 * * 0")).toEqual({
      kind: "weekly",
      dow: 0,
      hour: 9,
      minute: 0,
    })
    expect(describeCron("0 9 * * 7")).toEqual({
      kind: "weekly",
      dow: 0,
      hour: 9,
      minute: 0,
    })
  })

  it("recognizes monthly on a single day-of-month", () => {
    expect(describeCron("0 9 15 * *")).toEqual({
      kind: "monthly",
      dom: 15,
      hour: 9,
      minute: 0,
    })
  })

  it("falls back to raw for multi-value, yearly, or malformed expressions", () => {
    // day-of-week list
    expect(describeCron("0 9 * * 1,3,5")).toEqual({
      kind: "raw",
      cron: "0 9 * * 1,3,5",
    })
    // fixed month (yearly)
    expect(describeCron("0 0 1 1 *")).toEqual({
      kind: "raw",
      cron: "0 0 1 1 *",
    })
    // both dom and dow constrained
    expect(describeCron("0 9 15 * 1")).toEqual({
      kind: "raw",
      cron: "0 9 15 * 1",
    })
    // out-of-range hour
    expect(describeCron("0 25 * * *")).toEqual({
      kind: "raw",
      cron: "0 25 * * *",
    })
    // wrong field count
    expect(describeCron("0 9 * *")).toEqual({ kind: "raw", cron: "0 9 * *" })
    expect(describeCron("")).toEqual({ kind: "raw", cron: "" })
  })

  it("trims surrounding whitespace before parsing", () => {
    expect(describeCron("  0 9 * * *  ")).toEqual({
      kind: "daily",
      hour: 9,
      minute: 0,
    })
  })
})
