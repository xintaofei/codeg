import { describe, expect, it } from "vitest"
import type { PkRound } from "@/stores/pk-arena-store"
import { getArenaPillRound, getEffortControl } from "./pk-arena-policy"

describe("PK arena lifecycle policy", () => {
  it.each(["finished", "canceled", "interrupted"] as const)(
    "does not show a %s round in the minimized entry",
    (status) => {
      const terminal = { id: "7", status } as PkRound
      expect(getArenaPillRound([terminal], "7")).toBeNull()
    }
  )

  it("prefers a live round when the active round is terminal", () => {
    const finished = { id: "7", status: "finished" } as PkRound
    const running = { id: "8", status: "running" } as PkRound
    expect(getArenaPillRound([finished, running], "7")).toBe(running)
  })
})

describe("PK contestant reasoning capability", () => {
  it("shows the exact levels advertised by Qoder", () => {
    expect(getEffortControl(["low", "medium"], "reasoning_effort")).toEqual({
      kind: "select",
      configId: "reasoning_effort",
      options: ["low", "medium"],
    })
  })

  it("shows an unsupported state instead of silently hiding the field", () => {
    expect(getEffortControl([], null)).toEqual({ kind: "unsupported" })
  })
})
