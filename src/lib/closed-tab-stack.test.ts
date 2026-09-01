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

function conversation(id: number) {
  return snapshotConversationTab({
    id: `conv-${id}`,
    folderId: 1,
    conversationId: id,
    agentType: "claude_code",
    title: `t${id}`,
    isPinned: false,
  })
}

function drainKeys(): string[] {
  const keys: string[] = []
  while (true) {
    const next = popClosedTab()
    if (!next) return keys
    keys.push(next.key)
  }
}

describe("closed tab stack", () => {
  it("restores the most recently closed tab first", () => {
    pushClosedTab(
      snapshotConversationTab({
        id: "conv-10",
        folderId: 1,
        conversationId: 10,
        agentType: "claude_code",
        title: "first",
        isPinned: false,
      })
    )
    pushClosedTab(
      snapshotConversationTab({
        id: "conv-11",
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
          id: `conv-${i}`,
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

  // The file-tab closers record from inside a `setFileTabs` updater, which
  // React double-invokes under StrictMode and may replay when it discards a
  // render. A replayed pass must not cost a second press to walk past.
  it("keeps one entry per tab, replaying a close to the same stack", () => {
    pushClosedTab(conversation(1))
    pushClosedTab(conversation(1))
    expect(drainKeys()).toEqual(["conv-1"])
  })

  it("keeps a replayed close-all batch to one entry per tab, in order", () => {
    const batch = [conversation(1), conversation(2), conversation(3)]
    for (const tab of batch) pushClosedTab(tab)
    for (const tab of batch) pushClosedTab(tab)
    expect(drainKeys()).toEqual(["conv-3", "conv-2", "conv-1"])
  })

  it("moves a re-closed tab back to the top instead of duplicating it", () => {
    pushClosedTab(conversation(1))
    pushClosedTab(conversation(2))
    // Reopened from the sidebar (which does not pop), then closed again.
    pushClosedTab(conversation(1))
    expect(drainKeys()).toEqual(["conv-1", "conv-2"])
  })

  it("skips a file tab with no path", () => {
    expect(
      snapshotFileTab({ id: "f", kind: "file", path: null, folderId: 1 })
    ).toBeNull()
    expect(
      snapshotFileTab({
        id: "file:/repo/a.ts",
        kind: "file",
        path: "/repo/a.ts",
        folderId: 2,
      })
    ).toEqual({
      kind: "file",
      key: "file:/repo/a.ts",
      path: "/repo/a.ts",
      folderId: 2,
    })
  })

  // A diff tab carries the path it compares, but reopening goes through
  // `openFilePreview` — restoring one would silently swap the diff for the
  // source editor, so it is not recorded at all.
  it("skips a diff tab even though it carries a path", () => {
    for (const kind of ["diff", "rich-diff"]) {
      expect(
        snapshotFileTab({
          id: `${kind}:/repo/a.ts`,
          kind,
          path: "/repo/a.ts",
          folderId: 2,
        })
      ).toBeNull()
    }
  })
})
