import { act, render, screen, waitFor } from "@testing-library/react"
import { useEffect } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { TabProvider, useTabContext } from "@/contexts/tab-context"
import { TABS_CHANGED_EVENT } from "@/lib/types"
import type {
  AgentType,
  DbConversationSummary,
  FolderDetail,
  OpenedTab,
  SaveTabsOutcome,
  TabsChanged,
} from "@/lib/types"

const listOpenedTabsMock = vi.fn()
const saveOpenedTabsMock = vi.fn()
const setActiveFolderIdMock = vi.fn()
const activateConversationPaneMock = vi.fn()
const disconnectMock = vi.fn()
const subscribeMock = vi.fn()
const onTransportReconnectMock = vi.fn()
// Captured `tabs://changed` handler so tests can simulate inbound broadcasts.
let tabsChangedHandler: ((change: TabsChanged) => void) | null = null

vi.mock("next-intl", () => {
  // Return a STABLE function instance across renders, mirroring next-intl's
  // real behavior. An unstable `t` would re-run effects that depend on it
  // (e.g. the hydrate effect) on every render.
  const t = (key: string) => key
  return { useTranslations: () => t }
})

vi.mock("@/lib/api", () => ({
  listOpenedTabs: (...args: unknown[]) => listOpenedTabsMock(...args),
  saveOpenedTabs: (...args: unknown[]) => saveOpenedTabsMock(...args),
}))

vi.mock("@/lib/platform", () => ({
  subscribe: (...args: unknown[]) => subscribeMock(...args),
  onTransportReconnect: (...args: unknown[]) =>
    onTransportReconnectMock(...args),
}))

vi.mock("@/contexts/app-workspace-context", () => ({
  useAppWorkspace: () => ({
    conversations: conversationsMock,
    folders: foldersMock,
    foldersHydrated: true,
    setActiveFolderId: setActiveFolderIdMock,
  }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceContext: () => ({
    activateConversationPane: activateConversationPaneMock,
  }),
}))

vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpActions: () => ({
    disconnect: disconnectMock,
  }),
}))

vi.mock("@/hooks/use-sorted-available-agents", () => ({
  useSortedAvailableAgents: () => ({
    sortedTypes: ["codex" satisfies AgentType],
    fresh: true,
  }),
}))

const defaultFoldersMock: FolderDetail[] = [
  {
    id: 1,
    name: "repo",
    path: "/repo",
    git_branch: null,
    default_agent_type: "codex",
    last_opened_at: "2026-05-24T00:00:00Z",
    sort_order: 0,
    color: "blue",
    parent_id: null,
  },
  {
    id: 2,
    name: "other",
    path: "/other",
    git_branch: null,
    default_agent_type: "codex",
    last_opened_at: "2026-05-24T00:00:00Z",
    sort_order: 1,
    color: "green",
    parent_id: null,
  },
]

let foldersMock: FolderDetail[] = defaultFoldersMock

const conversationsMock: DbConversationSummary[] = [
  {
    id: 1,
    folder_id: 1,
    title: "First",
    agent_type: "codex",
    status: "in_progress",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 1,
    created_at: "2026-05-24T00:00:00Z",
    updated_at: "2026-05-24T00:00:00Z",
  },
  {
    id: 2,
    folder_id: 1,
    title: "Second",
    agent_type: "codex",
    status: "in_progress",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 1,
    created_at: "2026-05-24T00:00:00Z",
    updated_at: "2026-05-24T00:00:00Z",
  },
  {
    id: 3,
    folder_id: 2,
    title: "Third",
    agent_type: "codex",
    status: "in_progress",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 1,
    created_at: "2026-05-24T00:00:00Z",
    updated_at: "2026-05-24T00:00:00Z",
  },
]

let latestContext: ReturnType<typeof useTabContext> | null = null

function Probe() {
  const ctx = useTabContext()
  const activeTab = ctx.tabs.find((tab) => tab.id === ctx.activeTabId)

  useEffect(() => {
    latestContext = ctx
  }, [ctx])

  return (
    <div>
      <output data-testid="active">{ctx.activeTabId ?? "none"}</output>
      <output data-testid="tabs">
        {ctx.tabs.map((tab) => tab.id).join(",")}
      </output>
      <output data-testid="active-folder">
        {activeTab?.folderId ?? "none"}
      </output>
    </div>
  )
}

function renderTabs() {
  latestContext = null
  return render(
    <TabProvider>
      <Probe />
    </TabProvider>
  )
}

function openConversationTab(
  folderId: number,
  conversationId: number,
  title: string
) {
  act(() => {
    latestContext?.openTab(folderId, conversationId, "codex", true, title)
  })
}

describe("TabProvider tab state transitions", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    foldersMock = defaultFoldersMock
    listOpenedTabsMock.mockReturnValue(new Promise(() => {}))
    saveOpenedTabsMock.mockResolvedValue({
      accepted: true,
      version: 1,
      tabs: [],
    })
    tabsChangedHandler = null
    subscribeMock.mockImplementation(
      (event: string, handler: (change: TabsChanged) => void) => {
        if (event === TABS_CHANGED_EVENT) tabsChangedHandler = handler
        return Promise.resolve(() => {})
      }
    )
    onTransportReconnectMock.mockReturnValue(() => {})
  })

  it("activates the neighboring tab when another tab update is already queued", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")
    openConversationTab(1, 2, "Second")
    act(() => {
      latestContext?.switchTab("conv-1-codex-1")
    })

    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")

    act(() => {
      latestContext?.setTabRuntimeConversationId("conv-1-codex-1", -1)
      latestContext?.closeTab("conv-1-codex-1")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-2")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
  })

  it("keeps the current active tab when closing an inactive tab", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")
    openConversationTab(1, 2, "Second")
    act(() => {
      latestContext?.switchTab("conv-1-codex-1")
    })

    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")

    act(() => {
      latestContext?.closeTab("conv-1-codex-2")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")
  })

  it("creates and activates a replacement draft when closing the last tab with folders available", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")

    act(() => {
      latestContext?.closeTab("conv-1-codex-1")
    })

    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toMatch(/^new-/)
    expect(screen.getByTestId("active")).toHaveTextContent(tabsText)
  })

  it("clears the active tab when closing the last tab with no folders available", () => {
    foldersMock = []
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")

    act(() => {
      latestContext?.closeTab("conv-1-codex-1")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("")
    expect(screen.getByTestId("active")).toHaveTextContent("none")
  })

  it("activates a remaining tab when closing a folder after switching to one of its tabs in the same batch", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")
    openConversationTab(1, 2, "Second")
    openConversationTab(2, 3, "Third")
    act(() => {
      latestContext?.switchTab("conv-2-codex-3")
    })

    expect(screen.getByTestId("active")).toHaveTextContent("conv-2-codex-3")

    act(() => {
      latestContext?.switchTab("conv-1-codex-1")
      latestContext?.closeTabsByFolder(1)
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-2-codex-3")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-2-codex-3")
  })

  it("ignores closeOtherTabs when its target was removed earlier in the same batch", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")
    openConversationTab(1, 2, "Second")

    act(() => {
      latestContext?.closeTab("conv-1-codex-1")
      latestContext?.closeOtherTabs("conv-1-codex-1")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-2")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
  })

  it("keeps an existing draft active when reopening a draft after closing it in the same batch", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })

    const draftTabId = latestContext?.activeTabId
    expect(draftTabId).toMatch(/^new-/)

    act(() => {
      latestContext?.closeTab(draftTabId!)
      latestContext?.openNewConversationTab(1, "/repo")
    })

    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toMatch(/^new-/)
    expect(screen.getByTestId("active")).toHaveTextContent(tabsText)
  })

  it("retargets the replacement draft when reopening a closed draft for another folder in the same batch", async () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })

    const draftTabId = latestContext?.activeTabId
    expect(draftTabId).toMatch(/^new-/)

    act(() => {
      latestContext?.closeTab(draftTabId!)
      latestContext?.openNewConversationTab(2, "/other")
    })

    const replacementTabId = screen.getByTestId("tabs").textContent ?? ""
    expect(replacementTabId).toMatch(/^new-/)
    expect(replacementTabId).not.toBe(draftTabId)
    expect(screen.getByTestId("active")).toHaveTextContent(replacementTabId)

    await waitFor(() => {
      expect(disconnectMock).toHaveBeenCalledWith(replacementTabId)
      expect(screen.getByTestId("active-folder")).toHaveTextContent("2")
    })
  })

  it("applies the supplied folderDefaultAgent for a folder not in the open list", () => {
    // Regression: navigating a branch switch to a just-reopened (closed) folder
    // passes that folder's saved default agent explicitly, because `foldersRef`
    // only catches up on the next render. Folder 999 is absent from the
    // provider's folders, so without the override the draft would fall back to
    // sortedTypes[0] ("codex"); the override must win.
    renderTabs()
    expect(latestContext).not.toBeNull()

    act(() => {
      latestContext?.openNewConversationTab(999, "/closed-wt", {
        folderDefaultAgent: "claude_code",
      })
    })

    const activeId = latestContext?.activeTabId
    const draft = latestContext?.tabs.find((tab) => tab.id === activeId)
    expect(draft?.folderId).toBe(999)
    expect(draft?.agentType).toBe("claude_code")
  })

  it("activates an opened tab when another tab update is already queued", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")

    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")

    act(() => {
      latestContext?.setTabRuntimeConversationId("conv-1-codex-1", -1)
      latestContext?.openTab(1, 2, "codex", true, "Second")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-2")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
  })

  it("keeps the retained draft tab active when binding it over an existing duplicate conversation tab", () => {
    renderTabs()

    expect(latestContext).not.toBeNull()

    openConversationTab(1, 1, "First")
    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })

    const draftTabId = latestContext?.activeTabId
    expect(draftTabId).toMatch(/^new-/)

    act(() => {
      latestContext?.setTabRuntimeConversationId(draftTabId!, -1)
      latestContext?.bindConversationTab(draftTabId!, 1, "codex", "First", -1)
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent(draftTabId!)
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-1"
    )
    expect(screen.getByTestId("active")).toHaveTextContent(draftTabId!)
  })

  it("does not report a preview replacement for a preview tab already closed in the same batch", () => {
    const replacedTabIds: string[] = []
    renderTabs()

    expect(latestContext).not.toBeNull()

    latestContext?.onPreviewTabReplaced((tabId) => {
      replacedTabIds.push(tabId)
    })
    act(() => {
      latestContext?.openTab(1, 1, "codex", false, "First")
    })

    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")

    act(() => {
      latestContext?.closeTab("conv-1-codex-1")
      latestContext?.openTab(1, 2, "codex", false, "Second")
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-2")
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
    expect(replacedTabIds).toEqual([])
  })
})

function tabItem(
  folderId: number,
  conversationId: number,
  isActive = false
): OpenedTab {
  return {
    id: conversationId,
    folder_id: folderId,
    conversation_id: conversationId,
    agent_type: "codex",
    position: 0,
    is_active: isActive,
    is_pinned: true,
  }
}

describe("TabProvider cross-client sync", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    foldersMock = defaultFoldersMock
    listOpenedTabsMock.mockResolvedValue({ items: [], version: 0 })
    saveOpenedTabsMock.mockResolvedValue({
      accepted: true,
      version: 1,
      tabs: [],
    })
    tabsChangedHandler = null
    subscribeMock.mockImplementation(
      (event: string, handler: (change: TabsChanged) => void) => {
        if (event === TABS_CHANGED_EVENT) tabsChangedHandler = handler
        return Promise.resolve(() => {})
      }
    )
    onTransportReconnectMock.mockReturnValue(() => {})
  })

  async function renderHydrated() {
    renderTabs()
    // Flush mount effects: the hydrate promise + the async subscribe() IIFE
    // that captures the handler.
    await act(async () => {})
  }

  it("applies a remote snapshot, adding a conversation tab", async () => {
    await renderHydrated()
    expect(tabsChangedHandler).not.toBeNull()

    act(() => {
      tabsChangedHandler?.({
        version: 1,
        origin: "other-device",
        tabs: [tabItem(1, 1)],
      })
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
  })

  it("does not save when applying a remote snapshot (no echo back)", async () => {
    await renderHydrated()
    saveOpenedTabsMock.mockClear()

    act(() => {
      tabsChangedHandler?.({
        version: 1,
        origin: "other-device",
        tabs: [tabItem(1, 1)],
      })
    })

    // The applying-remote guard makes the save effect a no-op: no timer armed,
    // so the save is never issued.
    expect(saveOpenedTabsMock).not.toHaveBeenCalled()
  })

  it("re-picks a neighbor when a remote snapshot removes the focused tab", async () => {
    await renderHydrated()

    act(() => {
      tabsChangedHandler?.({
        version: 1,
        origin: "x",
        tabs: [tabItem(1, 1), tabItem(1, 2)],
      })
    })
    act(() => {
      latestContext?.switchTab("conv-1-codex-2")
    })
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")

    act(() => {
      tabsChangedHandler?.({ version: 2, origin: "x", tabs: [tabItem(1, 1)] })
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-2"
    )
    // The remote snapshot carried no active marker (sender was on a draft), so
    // focus re-picks the surviving neighbor.
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")
  })

  it("preserves the device-local draft across a remote apply", async () => {
    await renderHydrated()

    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })
    const draftId = latestContext?.activeTabId
    expect(draftId).toMatch(/^new-/)

    act(() => {
      tabsChangedHandler?.({ version: 1, origin: "x", tabs: [tabItem(1, 1)] })
    })

    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toContain("conv-1-codex-1")
    expect(tabsText).toContain(draftId ?? "")
  })

  it("synthesizes a replacement draft when a remote snapshot is empty", async () => {
    listOpenedTabsMock.mockResolvedValue({ items: [tabItem(1, 1)], version: 1 })
    await renderHydrated()
    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")

    act(() => {
      tabsChangedHandler?.({ version: 2, origin: "x", tabs: [] })
    })

    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toMatch(/^new-/)
    expect(screen.getByTestId("active")).toHaveTextContent(tabsText)
  })

  it("drops a remote change at or below the current version", async () => {
    listOpenedTabsMock.mockResolvedValue({ items: [], version: 5 })
    await renderHydrated()

    act(() => {
      tabsChangedHandler?.({ version: 3, origin: "x", tabs: [tabItem(1, 1)] })
    })
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-1"
    )

    act(() => {
      tabsChangedHandler?.({ version: 5, origin: "x", tabs: [tabItem(1, 2)] })
    })
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-2"
    )
  })

  it("buffers a remote change that beats hydration and applies it after", async () => {
    let resolveList: (snap: {
      items: OpenedTab[]
      version: number
    }) => void = () => {}
    listOpenedTabsMock.mockReturnValue(
      new Promise((res) => {
        resolveList = res
      })
    )
    renderTabs()
    await act(async () => {
      await Promise.resolve()
    })
    expect(tabsChangedHandler).not.toBeNull()

    // Arrives before hydration completes → buffered, not yet applied.
    act(() => {
      tabsChangedHandler?.({ version: 1, origin: "x", tabs: [tabItem(1, 1)] })
    })
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-1"
    )

    // Hydrate at version 0 → the buffered v1 change is applied.
    await act(async () => {
      resolveList({ items: [], version: 0 })
      await Promise.resolve()
    })
    await waitFor(() => {
      expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
    })
  })

  it("mirrors the focused tab from a remote snapshot", async () => {
    await renderHydrated()

    act(() => {
      tabsChangedHandler?.({
        version: 1,
        origin: "x",
        tabs: [tabItem(1, 1), tabItem(1, 2, true)],
      })
    })

    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-2")
    // Focus is mirrored: the remote's active tab (c2) becomes ours.
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
  })

  it("does not steal focus from an in-progress local draft", async () => {
    await renderHydrated()

    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })
    const draftId = latestContext?.activeTabId
    expect(draftId).toMatch(/^new-/)

    // Remote focuses a conversation tab — but we're typing in a draft, so the
    // draft stays focused (its unsent input must not be yanked away).
    act(() => {
      tabsChangedHandler?.({
        version: 1,
        origin: "x",
        tabs: [tabItem(1, 1, true)],
      })
    })

    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toContain("conv-1-codex-1")
    expect(tabsText).toContain(draftId ?? "")
    expect(screen.getByTestId("active")).toHaveTextContent(draftId ?? "")
  })

  it("saves the focused tab so focus syncs across clients", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true), tabItem(1, 2)],
      version: 1,
    })
    await renderHydrated()
    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-1")
    saveOpenedTabsMock.mockClear()

    act(() => {
      latestContext?.switchTab("conv-1-codex-2")
    })

    // The debounced save fires with the new focus reflected in `is_active`.
    await waitFor(() => expect(saveOpenedTabsMock).toHaveBeenCalled(), {
      timeout: 2000,
    })
    const calls = saveOpenedTabsMock.mock.calls
    const items = calls[calls.length - 1][0] as OpenedTab[]
    const active = items.find((it) => it.is_active)
    expect(active?.conversation_id).toBe(2)
  })

  it("restores the focused tab from is_active on hydrate", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1), tabItem(1, 2, true)],
      version: 3,
    })
    await renderHydrated()

    expect(screen.getByTestId("active")).toHaveTextContent("conv-1-codex-2")
  })

  it("cancels a pending local save when a remote snapshot supersedes it", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true), tabItem(1, 2)],
      version: 1,
    })
    await renderHydrated()
    saveOpenedTabsMock.mockClear()

    // Arm a debounced save by switching focus...
    act(() => {
      latestContext?.switchTab("conv-1-codex-2")
    })
    // ...then a newer remote snapshot lands before the 500ms timer fires.
    act(() => {
      tabsChangedHandler?.({ version: 2, origin: "x", tabs: [tabItem(1, 1)] })
    })

    // The superseded save must never fire (its timer is cleared on apply), so a
    // now-stale payload can't save with the bumped version and clobber truth.
    await new Promise((r) => setTimeout(r, 600))
    expect(saveOpenedTabsMock).not.toHaveBeenCalled()
  })

  it("cancels an armed save when the set reverts to the last-saved state", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true)],
      version: 1,
    })
    await renderHydrated()
    saveOpenedTabsMock.mockClear()

    // Open c2 (arms a debounced save for [c1,c2]) then close it before 500ms —
    // the set reverts to the already-saved [c1], so the armed save (which would
    // persist & broadcast the closed c2) must be cancelled, not just skipped.
    act(() => {
      latestContext?.openTab(1, 2, "codex", true, "Second")
    })
    act(() => {
      latestContext?.closeTab("conv-1-codex-2")
    })

    await new Promise((r) => setTimeout(r, 600))
    expect(saveOpenedTabsMock).not.toHaveBeenCalled()
  })

  it("re-saves to reconcile when the set reverts while a save is in flight", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true)],
      version: 1,
    })
    // Control save resolution so we can revert mid-flight.
    let resolveSave: (r: SaveTabsOutcome) => void = () => {}
    saveOpenedTabsMock.mockImplementation(
      () =>
        new Promise<SaveTabsOutcome>((res) => {
          resolveSave = res
        })
    )
    await renderHydrated()

    // Open c2 → arm a save; let the 500ms debounce fire (now in flight).
    act(() => {
      latestContext?.openTab(1, 2, "codex", true, "Second")
    })
    await waitFor(() => expect(saveOpenedTabsMock).toHaveBeenCalledTimes(1), {
      timeout: 2000,
    })
    expect(
      (saveOpenedTabsMock.mock.calls[0][0] as OpenedTab[]).map(
        (i) => i.conversation_id
      )
    ).toEqual([1, 2])

    // Revert (close c2) BEFORE the in-flight save resolves.
    act(() => {
      latestContext?.closeTab("conv-1-codex-2")
    })
    // The in-flight save resolves accepted — it persisted the obsolete [c1,c2].
    await act(async () => {
      resolveSave({ accepted: true, version: 2, tabs: [] })
      await Promise.resolve()
    })

    // Divergence must self-heal: a second save persists the reverted [c1].
    await waitFor(() => expect(saveOpenedTabsMock).toHaveBeenCalledTimes(2), {
      timeout: 2000,
    })
    expect(
      (saveOpenedTabsMock.mock.calls[1][0] as OpenedTab[]).map(
        (i) => i.conversation_id
      )
    ).toEqual([1])
  })

  it("does not regress the version when an accepted save resolves after a newer remote", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true)],
      version: 1,
    })
    let resolveSave: (r: SaveTabsOutcome) => void = () => {}
    saveOpenedTabsMock.mockImplementation(
      () =>
        new Promise<SaveTabsOutcome>((res) => {
          resolveSave = res
        })
    )
    await renderHydrated()

    // Arm + fire a save (in flight, based on v1).
    act(() => {
      latestContext?.openTab(1, 2, "codex", true, "Second")
    })
    await waitFor(() => expect(saveOpenedTabsMock).toHaveBeenCalledTimes(1), {
      timeout: 2000,
    })

    // A newer remote snapshot (v5) is applied while the save is in flight.
    act(() => {
      tabsChangedHandler?.({
        version: 5,
        origin: "x",
        tabs: [tabItem(1, 1), tabItem(1, 3)],
      })
    })
    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-3")

    // The stale save resolves accepted at the older v2.
    await act(async () => {
      resolveSave({ accepted: true, version: 2, tabs: [] })
      await Promise.resolve()
    })

    // The version must not have regressed to 2 — a remote at v3 is still dropped.
    act(() => {
      tabsChangedHandler?.({ version: 3, origin: "x", tabs: [tabItem(1, 9)] })
    })
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-9"
    )
  })

  it("ignores a rejected save's stale snapshot when a newer remote already applied", async () => {
    listOpenedTabsMock.mockResolvedValue({
      items: [tabItem(1, 1, true)],
      version: 1,
    })
    let resolveSave: (r: SaveTabsOutcome) => void = () => {}
    saveOpenedTabsMock.mockImplementation(
      () =>
        new Promise<SaveTabsOutcome>((res) => {
          resolveSave = res
        })
    )
    await renderHydrated()

    // Arm + fire a save (in flight, based on v1).
    act(() => {
      latestContext?.openTab(1, 2, "codex", true, "Second")
    })
    await waitFor(() => expect(saveOpenedTabsMock).toHaveBeenCalledTimes(1), {
      timeout: 2000,
    })

    // A newer remote snapshot (v5) is applied while the save is in flight.
    act(() => {
      tabsChangedHandler?.({
        version: 5,
        origin: "x",
        tabs: [tabItem(1, 1), tabItem(1, 3)],
      })
    })
    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-3")

    // The save is REJECTED carrying the server's older v2 snapshot.
    await act(async () => {
      resolveSave({
        accepted: false,
        version: 2,
        tabs: [tabItem(1, 1), tabItem(1, 2)],
      })
      await Promise.resolve()
    })

    // The stale v2 reconciliation must NOT clobber the applied v5 state nor
    // regress the version: c3 stays, c2 never appears, and a later remote at v3
    // is still dropped (version stayed at 5).
    expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-3")
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-2"
    )
    act(() => {
      tabsChangedHandler?.({ version: 3, origin: "x", tabs: [tabItem(1, 9)] })
    })
    expect(screen.getByTestId("tabs").textContent).not.toContain(
      "conv-1-codex-9"
    )
  })

  it("preserves a bound draft's local id and runtime session across a remote apply", async () => {
    await renderHydrated()

    act(() => {
      latestContext?.openNewConversationTab(1, "/repo")
    })
    const draftId = latestContext?.activeTabId
    expect(draftId).toMatch(/^new-/)

    // The draft binds to a real conversation but keeps its `new-*` id + runtime.
    act(() => {
      latestContext?.bindConversationTab(draftId!, 1, "codex", "First", -7)
    })

    // A remote snapshot now includes that conversation.
    act(() => {
      tabsChangedHandler?.({ version: 1, origin: "x", tabs: [tabItem(1, 1)] })
    })

    // The tab keeps its original id (no remount under conv-1-codex-1) and its
    // live runtime session id survives — the in-progress conversation isn't lost.
    const tabsText = screen.getByTestId("tabs").textContent ?? ""
    expect(tabsText).toContain(draftId ?? "")
    expect(tabsText).not.toContain("conv-1-codex-1")
    const bound = latestContext?.tabs.find((tb) => tb.id === draftId)
    expect(bound?.runtimeConversationId).toBe(-7)
  })

  it("reconciles a change missed before the subscription went live", async () => {
    // Hydrate sees v1; a change committed before the server-side receiver was
    // live bumped the server to v2 — the post-subscribe refetch must catch it.
    listOpenedTabsMock
      .mockResolvedValueOnce({ items: [], version: 1 })
      .mockResolvedValueOnce({ items: [tabItem(1, 1)], version: 2 })
    await renderHydrated()

    await waitFor(() => {
      expect(screen.getByTestId("tabs")).toHaveTextContent("conv-1-codex-1")
    })
  })
})
