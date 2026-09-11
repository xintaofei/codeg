import { describe, expect, it } from "vitest"

import {
  canEditUserTurn,
  contentBlocksToPromptInput,
  filterHiddenTurns,
  timestampsToHideFrom,
  turnTimestampMs,
} from "./edit-user-message"
import type { MessageTurn } from "@/lib/types"

function turn(
  id: string,
  timestamp: string,
  role: MessageTurn["role"] = "user"
): MessageTurn {
  return { id, role, blocks: [{ type: "text", text: id }], timestamp }
}

describe("turnTimestampMs", () => {
  it("parses an ISO timestamp", () => {
    expect(turnTimestampMs({ timestamp: "2026-08-15T12:00:00.000Z" })).toBe(
      Date.parse("2026-08-15T12:00:00.000Z")
    )
  })

  it("returns null for garbage", () => {
    expect(turnTimestampMs({ timestamp: "not-a-date" })).toBeNull()
  })
})

describe("timestampsToHideFrom", () => {
  const turns = [
    turn("u1", "2026-08-15T12:00:00.000Z"),
    turn("a1", "2026-08-15T12:00:01.000Z", "assistant"),
    turn("u2", "2026-08-15T12:00:02.000Z"),
    turn("a2", "2026-08-15T12:00:03.000Z", "assistant"),
  ]

  it("hides the edited user turn and everything after it", () => {
    expect(timestampsToHideFrom(turns, "u2")).toEqual([
      Date.parse("2026-08-15T12:00:02.000Z"),
      Date.parse("2026-08-15T12:00:03.000Z"),
    ])
  })

  it("hides the whole tail when the first user message is edited", () => {
    expect(timestampsToHideFrom(turns, "u1")).toHaveLength(4)
  })

  it("returns empty when the turn is missing", () => {
    expect(timestampsToHideFrom(turns, "nope")).toEqual([])
  })
})

describe("filterHiddenTurns", () => {
  const turns = [
    turn("u1", "2026-08-15T12:00:00.000Z"),
    turn("a1", "2026-08-15T12:00:01.000Z", "assistant"),
    turn("u2", "2026-08-15T12:00:02.000Z"),
  ]

  it("drops only the hidden timestamps and keeps order", () => {
    const hidden = [Date.parse("2026-08-15T12:00:01.000Z")]
    expect(filterHiddenTurns(turns, hidden).map((t) => t.id)).toEqual([
      "u1",
      "u2",
    ])
  })

  it("is a no-op on an empty hide set", () => {
    expect(filterHiddenTurns(turns, [])).toBe(turns)
  })

  it("keeps a turn whose timestamp cannot be parsed", () => {
    const messy = [turn("bad", "???")]
    expect(filterHiddenTurns(messy, [1])).toEqual(messy)
  })
})

describe("contentBlocksToPromptInput", () => {
  it("keeps text and image, drops everything else", () => {
    expect(
      contentBlocksToPromptInput([
        { type: "text", text: "fix the build" },
        { type: "thinking", text: "nope" },
        {
          type: "image",
          data: "abc",
          mime_type: "image/png",
          uri: "file:///a.png",
        },
        { type: "text", text: "" },
      ])
    ).toEqual([
      { type: "text", text: "fix the build" },
      {
        type: "image",
        data: "abc",
        mime_type: "image/png",
        uri: "file:///a.png",
      },
    ])
  })
})

describe("canEditUserTurn", () => {
  it("allows a persisted user turn", () => {
    expect(canEditUserTurn({ role: "user", phase: "persisted" })).toBe(true)
  })

  it("rejects optimistic, streaming, assistant, and read-only turns", () => {
    expect(canEditUserTurn({ role: "user", phase: "optimistic" })).toBe(false)
    expect(canEditUserTurn({ role: "user", phase: "streaming" })).toBe(false)
    expect(canEditUserTurn({ role: "assistant", phase: "persisted" })).toBe(
      false
    )
    expect(
      canEditUserTurn({ role: "user", phase: "persisted", readOnly: true })
    ).toBe(false)
  })
})
