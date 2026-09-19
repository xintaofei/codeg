import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  peekClosedTab,
  popClosedTab,
  resetClosedTabStackForTests,
} from "@/lib/closed-tab-stack"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "./app-workspace-store"
import { resetTabStore, useTabStore } from "./tab-store"
import type { DbConversationSummary, FolderDetail } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  listOpenedTabs: vi.fn(),
  saveOpenedTabs: vi.fn(),
  getFolderConversation: vi.fn(),
}))

vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(),
  onTransportReconnect: vi.fn(),
}))

const folder = {
  id: 1,
  name: "repo",
  path: "/repo",
} as unknown as FolderDetail

function conversationTab(id: string, conversationId: number, isPinned = true) {
  return {
    id,
    kind: "conversation" as const,
    folderId: 1,
    conversationId,
    agentType: "claude_code" as const,
    title: `conv-${conversationId}`,
    isPinned,
  }
}

function summaryOf(
  id: number,
  over: Partial<DbConversationSummary> = {}
): DbConversationSummary {
  return {
    id,
    folder_id: 1,
    title: `conv-${id}`,
    title_locked: false,
    agent_type: "claude_code",
    status: "in_progress",
    kind: "delegate",
    model: null,
    git_branch: null,
    external_id: `child-${id}`,
    message_count: 0,
    child_count: 0,
    created_at: "2026-06-10T10:00:00.000Z",
    updated_at: "2026-06-10T10:00:00.000Z",
    pinned_at: null,
    ...over,
  }
}

function seed(parentConvId = 7) {
  useAppWorkspaceStore.setState({ folders: [folder], allFolders: [folder] })
  useTabStore.setState({
    rawTabs: [
      conversationTab("tab-parent", parentConvId),
      conversationTab("tab-child", 42),
      conversationTab("tab-other", 9),
    ],
    activeTabId: "tab-other",
  })
}

beforeEach(() => {
  resetTabStore()
  resetAppWorkspaceStore()
  resetClosedTabStackForTests()
})

describe("native child tabs close with their parent", () => {
  it("registerNativeChildTab links the pair; closing the parent closes the child", () => {
    seed()
    const store = useTabStore.getState()
    store.registerNativeChildTab(42, 7)
    expect(useTabStore.getState().childTabParents.get(42)).toBe(7)

    store.closeTab("tab-parent")

    const remaining = useTabStore
      .getState()
      .rawTabs.filter((t) => t.conversationId != null)
    expect(remaining.map((t) => t.conversationId)).toEqual([9])
    // The child's link is pruned along with its tab.
    expect(useTabStore.getState().childTabParents.size).toBe(0)
  })

  it("the cascade does not offer the child to reopen, only the parent", () => {
    seed()
    const store = useTabStore.getState()
    store.registerNativeChildTab(42, 7)
    store.closeTab("tab-parent")

    // The user closed the parent — that one is reopenable. The cascade-closed
    // child was never asked for, so it must not shadow it on the stack.
    expect(popClosedTab()).toMatchObject({ conversationId: 7 })
    expect(popClosedTab()).toBeNull()
    expect(peekClosedTab()).toBeNull()
  })

  it("a restored session (empty link map) cascades from the seeded child summary", () => {
    seed()
    // After a restart `childTabParents` is empty — the child tab was rehydrated
    // and its summary seeded from the DB (parent_id = 7). Closing the parent
    // must still take it down.
    useTabStore.setState({
      childSummaries: new Map([[42, summaryOf(42, { parent_id: 7 })]]),
    })
    useTabStore.getState().closeTab("tab-parent")

    const remaining = useTabStore
      .getState()
      .rawTabs.filter((t) => t.conversationId != null)
    expect(remaining.map((t) => t.conversationId)).toEqual([9])
  })

  it("closing the child leaves the parent alone and prunes the link", () => {
    seed()
    const store = useTabStore.getState()
    store.registerNativeChildTab(42, 7)
    store.closeTab("tab-child")

    const remaining = useTabStore
      .getState()
      .rawTabs.filter((t) => t.conversationId != null)
    expect(remaining.map((t) => t.conversationId)).toEqual([7, 9])
    expect(useTabStore.getState().childTabParents.size).toBe(0)
  })

  it("the cascade recurses through grandchild rows", () => {
    seed()
    useTabStore.setState({
      rawTabs: [
        ...useTabStore.getState().rawTabs,
        conversationTab("tab-grandchild", 43),
      ],
    })
    const store = useTabStore.getState()
    store.registerNativeChildTab(42, 7)
    store.registerNativeChildTab(43, 42)
    store.closeTab("tab-parent")

    const remaining = useTabStore
      .getState()
      .rawTabs.filter((t) => t.conversationId != null)
    expect(remaining.map((t) => t.conversationId)).toEqual([9])
  })

  it("an unregistered child tab survives its parent's close", () => {
    seed()
    // A tab opened from the sidebar (or a codeg delegation child opened some
    // other way) is not linked — the parent's close must not evict it.
    useTabStore.getState().closeTab("tab-parent")
    const remaining = useTabStore
      .getState()
      .rawTabs.filter((t) => t.conversationId != null)
    expect(remaining.map((t) => t.conversationId)).toEqual([42, 9])
  })

  it("registerNativeChildTab is idempotent and ignores self-links", () => {
    seed()
    const store = useTabStore.getState()
    store.registerNativeChildTab(42, 7)
    const after = useTabStore.getState().childTabParents
    store.registerNativeChildTab(42, 7)
    expect(useTabStore.getState().childTabParents).toBe(after)
    store.registerNativeChildTab(7, 7)
    expect(useTabStore.getState().childTabParents.has(7)).toBe(false)
  })
})
