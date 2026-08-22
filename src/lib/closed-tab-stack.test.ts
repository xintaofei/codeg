import { afterEach, describe, expect, it } from "vitest"

import {
  CLOSED_TAB_STACK_LIMIT,
  peekClosedTab,
  popClosedTab,
  pushClosedTab,
  resetClosedTabStackForTests,
  snapshotConversationTab,
  snapshotFileTab,
} from "./closed-tab-stack"

afterEach(() => {
  resetClosedTabStackForTests()
})

describe("closed tab stack", () => {
  it("restores the most recently closed tab first", () => {
    pushClosedTab(
      snapshotConversationTab({
        folderId: 1,
        conversationId: 10,
        agentType: "claude_code",
        title: "first",
        isPinned: false,
      })
    )
    pushClosedTab(
      snapshotConversationTab({
        folderId: 1,
        conversationId: 11,
        agentType: "codex",
        title: "second",
        isPinned: true,
      })
    )
    expect(peekClosedTab()?.kind).toBe("conversation")
    expect(popClosedTab()).toMatchObject({
      conversationId: 11,
      title: "second",
    })
    expect(popClosedTab()).toMatchObject({ conversationId: 10, title: "first" })
    expect(popClosedTab()).toBeNull()
  })

  it("drops the oldest entry past the browser-like cap", () => {
    for (let i = 0; i < CLOSED_TAB_STACK_LIMIT + 3; i += 1) {
      pushClosedTab(
        snapshotConversationTab({
          folderId: 1,
          conversationId: i,
          agentType: "grok",
          title: `t${i}`,
          isPinned: false,
        })
      )
    }
    const first = popClosedTab()
    expect(first).toMatchObject({ conversationId: CLOSED_TAB_STACK_LIMIT + 2 })
    let oldestKept: ReturnType<typeof popClosedTab> = null
    while (true) {
      const next = popClosedTab()
      if (!next) break
      oldestKept = next
    }
    expect(oldestKept).toMatchObject({ conversationId: 3 })
  })

  it("skips a file tab with no path", () => {
    expect(snapshotFileTab({ path: null, folderId: 1 })).toBeNull()
    expect(snapshotFileTab({ path: "/repo/a.ts", folderId: 2 })).toEqual({
      kind: "file",
      path: "/repo/a.ts",
      folderId: 2,
    })
  })
})
