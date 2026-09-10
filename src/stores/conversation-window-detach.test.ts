import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { listOpenedTabs, saveOpenedTabs } from "@/lib/api"
import { saveLastActiveContext } from "@/lib/last-active-context-storage"
import { conversationWindowRoute } from "@/lib/conversation-window"
import { leafIds, ROOT_GROUP_ID } from "@/lib/tab-group-layout"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "./app-workspace-store"
import { resetTabStore, useTabStore } from "./tab-store"
import type { FolderDetail, TabsChanged } from "@/lib/types"

vi.mock("@/lib/api", () => ({
  listOpenedTabs: vi.fn(async () => ({ version: 1, items: [] })),
  saveOpenedTabs: vi.fn(async () => ({ accepted: true, version: 2, tabs: [] })),
  getFolderConversation: vi.fn(),
}))

vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(),
  onTransportReconnect: vi.fn(),
}))

vi.mock("@/lib/last-active-context-storage", () => ({
  loadLastActiveContext: vi.fn(() => null),
  saveLastActiveContext: vi.fn(),
  clearLastActiveContext: vi.fn(),
}))

const folder = { id: 4, name: "repo", path: "/repo" } as unknown as FolderDetail

/** Device-local split state, written by whichever window last had one. */
const TAB_GROUPS_KEY = "workspace:tab-groups:v1"
const SPLIT_BLOB = JSON.stringify({
  layout: {
    type: "split",
    id: "s-root",
    orientation: "horizontal",
    ratios: [0.5, 0.5],
    children: [
      { type: "group", id: ROOT_GROUP_ID },
      { type: "group", id: "g-second" },
    ],
  },
  assignments: {},
  selection: {},
  tileByGroup: {},
  drafts: [],
  activeDraft: null,
})

const TARGET = {
  folderId: 4,
  conversationId: 7,
  agentType: "claude_code",
} as const

/** What the WORKSPACE window has open: a different conversation, focused. */
const workspaceSnapshot: TabsChanged = {
  version: 9,
  origin: "workspace",
  tabs: [
    {
      folder_id: 4,
      conversation_id: 99,
      agent_type: "claude_code",
      is_pinned: false,
      is_active: true,
    },
  ] as TabsChanged["tabs"],
}

function enterWindow(search: string) {
  window.history.replaceState({}, "", search)
  // The group blob and the initial layout are read by `initialTabState`, so the
  // URL has to be in place before the store is rebuilt.
  resetTabStore()
  resetAppWorkspaceStore()
  useAppWorkspaceStore.setState({
    folders: [folder],
    allFolders: [folder],
    foldersHydrated: true,
  })
  useTabStore.getState().setLabels({
    loadingConversation: "Loading...",
    newConversation: "New conversation",
    untitledConversation: "Untitled",
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
})

afterEach(() => {
  window.history.replaceState({}, "", "/workspace")
  resetTabStore()
  resetAppWorkspaceStore()
})

describe("detached conversation window", () => {
  beforeEach(() => {
    enterWindow(conversationWindowRoute(TARGET))
  })

  it("seeds exactly the conversation it was opened for, without reading opened_tabs", () => {
    useTabStore.getState().hydrate()

    const state = useTabStore.getState()
    expect(state.tabsHydrated).toBe(true)
    expect(state.rawTabs).toHaveLength(1)
    expect(state.rawTabs[0]).toMatchObject({
      folderId: 4,
      conversationId: 7,
      agentType: "claude_code",
      isPinned: true,
    })
    expect(state.activeTabId).toBe(state.rawTabs[0].id)
    // The single group has to select it, or the shell renders an empty window.
    expect(state.groupSelection[ROOT_GROUP_ID]).toBe(state.rawTabs[0].id)
    expect(listOpenedTabs).not.toHaveBeenCalled()
  })

  it("never pushes its tab set at the workspace", async () => {
    useTabStore.getState().hydrate()
    useTabStore.getState().runSaveEffect()
    await vi.waitFor(() =>
      expect(useTabStore.getState().tabsHydrated).toBe(true)
    )
    // The save is debounced by 500ms; give it more than that to fire.
    await new Promise((resolve) => setTimeout(resolve, 700))
    expect(saveOpenedTabs).not.toHaveBeenCalled()
  })

  // The reason this window can exist at all. `opened_tabs` is one shared list
  // and focus is mirrored across clients, so adopting the workspace's snapshot
  // would replace this window's tab AND drag it onto whatever the workspace is
  // looking at.
  it("is not dragged onto the workspace's conversation when the workspace switches tabs", () => {
    useTabStore.getState().hydrate()
    const seeded = useTabStore.getState().activeTabId

    useTabStore.getState().handleTabsChanged(workspaceSnapshot)

    const state = useTabStore.getState()
    expect(state.rawTabs).toHaveLength(1)
    expect(state.rawTabs[0].conversationId).toBe(7)
    expect(state.activeTabId).toBe(seeded)
  })

  it("does not refetch the shared tab set", async () => {
    useTabStore.getState().hydrate()
    await useTabStore.getState().refetchTabs()
    expect(listOpenedTabs).not.toHaveBeenCalled()
  })

  // localStorage is shared with the workspace window, so the detached view must
  // neither restore the workspace's split tree (it would render empty groups
  // around its single tab) nor write its own state back over it.
  it("keeps the workspace's device-local state to itself", async () => {
    localStorage.setItem(TAB_GROUPS_KEY, SPLIT_BLOB)
    enterWindow(conversationWindowRoute(TARGET))

    useTabStore.getState().hydrate()
    useTabStore.getState().persistLastActiveContext()

    expect(leafIds(useTabStore.getState().groupLayout)).toEqual([ROOT_GROUP_ID])
    expect(saveLastActiveContext).not.toHaveBeenCalled()
    // Longer than `schedulePersistGroupState`'s 300ms trailing debounce.
    await new Promise((resolve) => setTimeout(resolve, 500))
    expect(localStorage.getItem(TAB_GROUPS_KEY)).toBe(SPLIT_BLOB)
  })
})

// Same calls on the ordinary workspace URL: every gate above has to be keyed on
// the marker, not simply switched off.
describe("ordinary workspace window", () => {
  beforeEach(() => {
    enterWindow("/workspace")
  })

  it("restores the split tree from device-local state", () => {
    localStorage.setItem(TAB_GROUPS_KEY, SPLIT_BLOB)
    enterWindow("/workspace")

    expect(leafIds(useTabStore.getState().groupLayout)).toEqual([
      ROOT_GROUP_ID,
      "g-second",
    ])
  })

  it("hydrates from opened_tabs", async () => {
    vi.mocked(listOpenedTabs).mockResolvedValueOnce({
      version: 1,
      items: [
        {
          folder_id: 4,
          conversation_id: 7,
          agent_type: "claude_code",
          is_pinned: false,
          is_active: true,
        },
      ],
    } as Awaited<ReturnType<typeof listOpenedTabs>>)

    useTabStore.getState().hydrate()
    await vi.waitFor(() =>
      expect(useTabStore.getState().tabsHydrated).toBe(true)
    )
    expect(listOpenedTabs).toHaveBeenCalled()
  })

  it("adopts the workspace snapshot and mirrors its focus", async () => {
    useTabStore.getState().hydrate()
    await vi.waitFor(() =>
      expect(useTabStore.getState().tabsHydrated).toBe(true)
    )

    useTabStore.getState().handleTabsChanged(workspaceSnapshot)

    const state = useTabStore.getState()
    expect(state.rawTabs.map((t) => t.conversationId)).toContain(99)
    expect(
      state.rawTabs.find((t) => t.id === state.activeTabId)?.conversationId
    ).toBe(99)
  })

  it("saves its tab set", async () => {
    useTabStore.getState().hydrate()
    await vi.waitFor(() =>
      expect(useTabStore.getState().tabsHydrated).toBe(true)
    )
    useTabStore.getState().openTab(4, 7, "claude_code", true)
    useTabStore.getState().runSaveEffect()
    await vi.waitFor(() => expect(saveOpenedTabs).toHaveBeenCalled(), {
      timeout: 2000,
    })
  })
})
