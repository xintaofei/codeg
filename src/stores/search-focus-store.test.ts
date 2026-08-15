import { beforeEach, describe, expect, it } from "vitest"

import { useSearchFocusStore } from "./search-focus-store"
import type { SearchMatchLocation } from "@/lib/types"

function contentMatch(turnId: string, charStart: number): SearchMatchLocation {
  return {
    kind: "content",
    turn_id: turnId,
    block_index: 0,
    char_start: charStart,
    char_end: charStart + 2,
  }
}

describe("search focus store", () => {
  beforeEach(() => {
    useSearchFocusStore.setState({ focus: null })
  })

  it("advances through content matches in a loop", () => {
    const contentMatches = [
      { ...contentMatch("turn-a", 0), occurrenceIndex: 0 },
      { ...contentMatch("turn-b", 0), occurrenceIndex: 0 },
    ]
    useSearchFocusStore.getState().setFocus({
      conversationId: 7,
      query: "key",
      titleMatches: [],
      contentMatches,
      activeMatchIndex: 0,
    })

    useSearchFocusStore.getState().advance()
    expect(useSearchFocusStore.getState().focus?.activeMatchIndex).toBe(1)
    useSearchFocusStore.getState().advance()
    expect(useSearchFocusStore.getState().focus?.activeMatchIndex).toBe(0)
  })

  it("keeps focus when there are no content matches", () => {
    useSearchFocusStore.getState().setFocus({
      conversationId: 7,
      query: "key",
      titleMatches: [
        {
          kind: "title",
          turn_id: null,
          block_index: null,
          char_start: 0,
          char_end: 3,
        },
      ],
      contentMatches: [],
      activeMatchIndex: 0,
    })

    useSearchFocusStore.getState().advance()
    expect(useSearchFocusStore.getState().focus?.activeMatchIndex).toBe(0)
  })
})
