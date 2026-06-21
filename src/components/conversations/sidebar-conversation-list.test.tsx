import {
  createRef,
  type ReactNode,
  type Ref,
  useEffect,
  useImperativeHandle,
  useState,
} from "react"
import { act, fireEvent, render } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  SidebarConversationList,
  type SidebarConversationListHandle,
} from "./sidebar-conversation-list"
import type { DbConversationSummary, FolderDetail } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

// ── Probes ────────────────────────────────────────────────────────────────
// AgentIcon renders once per card body → counts card re-renders. The Folder /
// FolderOpen lucide icon renders once per FolderHeader body → counts folder
// re-renders. Both increment only when the owning memoized component does NOT
// bail out, so they measure exactly the production memo path.
const probes = vi.hoisted(() => ({ card: 0, folder: 0 }))

// Mutable backing store the mocked context hooks read from. Function refs are
// stable across renders (as the real providers' useCallback values are); only
// `conversations` and `tabs` churn — `tabs` is rebuilt fresh every render to
// mirror tab-context re-deriving it on each `conversations` change.
const store = vi.hoisted(() => ({
  conversations: [] as unknown[],
  folders: [] as unknown[],
  allFolders: [] as unknown[],
  activeTabId: null as string | null,
  tabSpec: [] as Array<{
    id: string
    conversationId: number | null
    agentType: string
    folderId: number
    title: string
    isPinned: boolean
  }>,
}))

const stableWorkspaceFns = vi.hoisted(() => ({
  refreshConversations: () => {},
  updateConversationLocal: () => {},
  removeFolderFromWorkspace: () => {},
  reorderFolders: vi.fn(() => Promise.resolve()),
  openFolder: () => {},
  refreshFolder: () => {},
}))

const stableTabFns = vi.hoisted(() => ({
  openTab: () => {},
  closeConversationTab: () => {},
  closeTabsByFolder: () => {},
  openNewConversationTab: () => {},
}))

const stableAgents = vi.hoisted(() => ({ sortedTypes: ["claude_code"] }))

// Context functions are stable refs in production (useCallback values); the
// mocks must be too, else the list's folder callbacks (which close over them)
// would churn and mask the memo behaviour under test.
const stableTask = vi.hoisted(() => ({
  addTask: () => {},
  updateTask: () => {},
}))
const stableTerminal = vi.hoisted(() => ({
  createTerminalInDirectory: () => {},
}))

vi.mock("@/components/agent-icon", () => ({
  AgentIcon: () => {
    probes.card++
    return null
  },
}))

// Controllable virtua geometry for the sticky-overlay tests. All rows are 32px
// (h-[2rem]), so offsets are index*32 and findItemIndex is floor(offset/32).
const virtuaCtl = vi.hoisted(() => ({
  scrollOffset: 0,
  onScroll: null as ((offset: number) => void) | null,
  scrollToIndex: vi.fn(),
}))

// Render EVERY row (data.map) rather than only a window, so the render-count
// probes stay meaningful in jsdom (which has no real layout/scroll). This is
// exactly why virtua's windowing itself needs manual QA on a large dataset. The
// mock also forwards a settable VirtualizerHandle (ref-as-prop, React 19) so the
// list's scroll-driven sticky logic can be exercised; with scrollOffset left at
// 0 the overlay stays hidden, so the memo-scope tests below are unaffected.
vi.mock("virtua", () => ({
  Virtualizer: ({
    data,
    children,
    onScroll,
    ref,
  }: {
    data: unknown[]
    children: (row: unknown, index: number) => ReactNode
    onScroll?: (offset: number) => void
    ref?: Ref<unknown>
  }) => {
    virtuaCtl.onScroll = onScroll ?? null
    useImperativeHandle(ref, () => ({
      get scrollOffset() {
        return virtuaCtl.scrollOffset
      },
      get scrollSize() {
        return data.length * 32
      },
      get viewportSize() {
        return 600
      },
      findItemIndex: (offset: number) =>
        Math.max(0, Math.min(data.length - 1, Math.floor(offset / 32))),
      getItemOffset: (index: number) => index * 32,
      getItemSize: () => 32,
      scrollToIndex: virtuaCtl.scrollToIndex,
      scrollTo: () => {},
      scrollBy: () => {},
    }))
    return <>{data.map((row, i) => children(row, i))}</>
  },
}))

// FolderHeader renders exactly one of FolderClosed/FolderOpen in its body →
// folder re-render probe. Every other icon stays real. (The Folders section
// header's Open Folder / Clone Repository actions use FolderOpenDot / FolderGit2,
// which are NOT mocked here, so they never inflate this probe.)
vi.mock("lucide-react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("lucide-react")>()
  return {
    ...actual,
    FolderClosed: () => {
      probes.folder++
      return null
    },
    FolderOpen: () => {
      probes.folder++
      return null
    },
  }
})

// The list mounts the Virtualizer only once OverlayScrollbars surfaces its
// viewport; the mock fires that bridge synchronously after mount.
vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({
    children,
    onViewportRef,
  }: {
    children?: ReactNode
    onViewportRef?: (el: HTMLElement | null) => void
  }) => {
    useEffect(() => {
      onViewportRef?.(document.createElement("div"))
    }, [onViewportRef])
    return <>{children}</>
  },
}))

vi.mock("next-themes", () => ({
  useTheme: () => ({ resolvedTheme: "light" }),
}))

vi.mock("@/hooks/use-appearance", () => ({
  useThemeColor: () => ({ themeColor: "blue" }),
  useZoomLevel: () => {},
}))

vi.mock("@/hooks/use-sorted-available-agents", () => ({
  useSortedAvailableAgents: () => ({
    sortedTypes: stableAgents.sortedTypes,
    fresh: true,
    refresh: () => {},
  }),
}))

vi.mock("@/contexts/terminal-context", () => ({
  useTerminalContext: () => stableTerminal,
}))

vi.mock("@/contexts/task-context", () => ({
  useTaskContext: () => stableTask,
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: null }),
}))

vi.mock("@/contexts/app-workspace-context", () => ({
  useAppWorkspace: () => ({
    folders: store.folders,
    allFolders: store.allFolders,
    conversations: store.conversations,
    conversationsLoading: false,
    conversationsError: null,
    ...stableWorkspaceFns,
  }),
}))

vi.mock("@/contexts/tab-context", () => ({
  useTabContext: () => ({
    ...stableTabFns,
    activeTabId: store.activeTabId,
    // Fresh array + fresh objects every render → worst-case churn, exactly what
    // the list's reuseSelected/reuseSet must absorb to keep folders memoized.
    tabs: store.tabSpec.map((t) => ({ ...t })),
  }),
}))
vi.mock("@/contexts/workbench-route-context", () => {
  // Stable singleton — the real provider memoizes these (useCallback([])), so a
  // fresh object per render would break the list's callback-identity memoization
  // probes.
  const value = {
    routeId: "conversations",
    isConversations: true,
    setRoute: () => {},
    openConversations: () => {},
  }
  return { useWorkbenchRoute: () => value }
})

// These only mount when their state opens (never in these tests); stub to keep
// the import graph light.
vi.mock("./conversation-manage-dialog", () => ({
  ConversationManageDialog: () => null,
}))
vi.mock("@/components/layout/clone-dialog", () => ({ CloneDialog: () => null }))
vi.mock("@/components/shared/directory-browser-dialog", () => ({
  DirectoryBrowserDialog: () => null,
}))

const MINUTE = 60_000
const FIXED = 1_700_000_000_000

function conv(
  id: number,
  folderId: number,
  overrides: Partial<DbConversationSummary> = {}
): DbConversationSummary {
  const createdAt = new Date(FIXED - 5 * MINUTE).toISOString()
  return {
    id,
    folder_id: folderId,
    title: `conv-${id}`,
    title_locked: false,
    agent_type: "claude_code",
    status: "pending",
    kind: "regular",
    model: null,
    git_branch: null,
    external_id: null,
    message_count: 0,
    created_at: createdAt,
    updated_at: createdAt,
    pinned_at: null,
    ...overrides,
  }
}

function folder(
  id: number,
  name: string,
  parentId: number | null = null
): FolderDetail {
  return {
    id,
    name,
    path: `/p/${id}`,
    color: "blue",
    default_agent_type: null,
    parent_id: parentId,
  } as unknown as FolderDetail
}

// Re-render only the list, leaving the intl provider mounted once — mirrors
// production, where NextIntlClientProvider sits high in the tree and stays
// stable (so `useTranslations` returns a stable `t`) while the list re-renders
// on each conversations change.
const harness: { rerender: () => void } = { rerender: () => {} }
function Harness() {
  const [, setTick] = useState(0)
  useEffect(() => {
    harness.rerender = () => setTick((n) => n + 1)
  }, [])
  return <SidebarConversationList showCompleted sortMode="created" />
}

function tree() {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <Harness />
    </NextIntlClientProvider>
  )
}

// Reset the virtua geometry before every test (runs before each describe's own
// beforeEach) so a scrolled overlay test never bleeds into the memo-scope or
// drag suites, which all assume scrollOffset 0 → overlay hidden.
beforeEach(() => {
  virtuaCtl.scrollOffset = 0
  virtuaCtl.onScroll = null
  virtuaCtl.scrollToIndex.mockClear()
})

describe("SidebarConversationList — single status event re-render scope", () => {
  beforeEach(() => {
    vi.useFakeTimers({ now: FIXED })
    probes.card = 0
    probes.folder = 0
    store.folders = [folder(1, "Folder 1"), folder(2, "Folder 2")]
    store.allFolders = store.folders
    store.conversations = [
      conv(11, 1),
      conv(12, 1),
      conv(21, 2),
      conv(22, 2),
      conv(23, 2),
    ]
    // One open tab in folder 1 → exercises the selectedConversation object and
    // openTabKeys Set reuse paths (these churn refs every render via the mock).
    store.activeTabId = "tab-11"
    store.tabSpec = [
      {
        id: "tab-11",
        conversationId: 11,
        agentType: "claude_code",
        folderId: 1,
        title: "conv-11",
        isPinned: false,
      },
    ]
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("re-renders exactly one card and no folder headers when a single summary changes", () => {
    render(tree())

    // Sanity: initial mount rendered all 5 cards and both folders.
    expect(probes.card).toBe(5)
    expect(probes.folder).toBe(2)

    // Mirror updateConversationLocal: replace exactly one summary (folder 2,
    // conv 22) with a new object; every other summary keeps its identity.
    const prev = store.conversations as DbConversationSummary[]
    const next = prev.slice()
    const idx = next.findIndex((c) => c.id === 22)
    next[idx] = { ...next[idx], status: "completed" }
    store.conversations = next

    probes.card = 0
    probes.folder = 0
    act(() => harness.rerender())

    // Card-level gate: only the changed card re-renders (R1 + R1b + shared now).
    expect(probes.card).toBe(1)
    // Folder headers are fully decoupled from their conversation rows in the
    // flat model — a status event leaves every header's props (count, expanded,
    // stable callbacks) unchanged, so no header re-renders at all.
    expect(probes.folder).toBe(0)
  })

  it("re-renders nothing when conversations are unchanged despite tab churn", () => {
    render(tree())

    probes.card = 0
    probes.folder = 0
    // Same conversations reference; tabs still churns (fresh array each render).
    act(() => harness.rerender())

    expect(probes.card).toBe(0)
    expect(probes.folder).toBe(0)
  })
})

describe("SidebarConversationList — Pinned section (migration semantics)", () => {
  beforeEach(() => {
    probes.card = 0
    probes.folder = 0
    store.folders = [folder(1, "Folder 1"), folder(2, "Folder 2")]
    store.allFolders = store.folders
    store.activeTabId = null
    store.tabSpec = []
    store.conversations = [
      conv(11, 1),
      conv(12, 1, { pinned_at: new Date(FIXED).toISOString() }), // pinned
      conv(21, 2),
    ]
  })

  it("moves a pinned conversation into the Pinned section above Folders, without duplicating it", () => {
    render(tree())
    const text = document.body.textContent ?? ""
    // The Pinned section header exists only because something is pinned, and it
    // sits above the Folders section.
    expect(text).toContain("Pinned")
    expect(text).toContain("Folders")
    const iPinned = text.indexOf("Pinned")
    const iFolders = text.indexOf("Folders")
    const iConv12 = text.indexOf("conv-12") // the pinned conversation
    const iConv11 = text.indexOf("conv-11") // unpinned → stays in its folder
    // conv-12 renders under the Pinned header and above the Folders section…
    expect(iPinned).toBeLessThan(iConv12)
    expect(iConv12).toBeLessThan(iFolders)
    // …while the unpinned conv-11 lives down in the folders section.
    expect(iFolders).toBeLessThan(iConv11)
    // Migration, not duplication: 3 conversations → exactly 3 rendered cards.
    expect(probes.card).toBe(3)
  })

  it("omits the Pinned section entirely when nothing is pinned", () => {
    store.conversations = [conv(11, 1), conv(21, 2)]
    render(tree())
    const text = document.body.textContent ?? ""
    expect(text).not.toContain("Pinned")
    expect(text).toContain("Folders")
  })
})

// jsdom has no PointerEvent and no layout, so the gesture is driven with plain
// bubbling events plus a mocked getBoundingClientRect. This exercises the
// component wiring (threshold → surface gating → commit/abort) that the pure
// index-math unit tests can't reach; real virtua scrolling/autoscroll still
// needs manual QA.
function firePointer(
  target: EventTarget,
  type: string,
  props: {
    clientX?: number
    clientY?: number
    pointerId?: number
    button?: number
  } = {}
) {
  const ev = new Event(type, { bubbles: true, cancelable: true })
  Object.assign(ev, {
    pointerId: 1,
    button: 0,
    clientX: 0,
    clientY: 0,
    ...props,
  })
  target.dispatchEvent(ev)
}

describe("SidebarConversationList — folder drag gesture", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.useFakeTimers({ now: FIXED })
    stableWorkspaceFns.reorderFolders.mockClear()
    store.folders = [folder(1, "F1"), folder(2, "F2"), folder(3, "F3")]
    store.allFolders = store.folders
    store.conversations = [conv(11, 1), conv(21, 2), conv(31, 3)]
    store.activeTabId = null
    store.tabSpec = []
    // Fixed geometry: viewport / drag surface anchored at top=0 and tall enough
    // that the test pointer Ys stay clear of the autoscroll edges.
    rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        top: 0,
        bottom: 600,
        left: 0,
        right: 200,
        width: 200,
        height: 600,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect)
  })

  afterEach(() => {
    // A committed drag leaves a one-shot capture-phase "click" suppressor on
    // window whose rAF-based removal does not fire under fake timers. Drain it
    // with a throwaway window click (target=window never reaches the React root)
    // so it cannot swallow a later test's click.
    window.dispatchEvent(new MouseEvent("click", { bubbles: true }))
    rectSpy.mockRestore()
    vi.useRealTimers()
  })

  function grip(folderId: number): HTMLElement {
    const button = document.querySelector(`[data-folder-id="${folderId}"]`)
    const el = button?.parentElement
    if (!el) throw new Error(`grip for folder ${folderId} not found`)
    return el
  }

  // Press folder 1, cross the 6px threshold (mounts the collapsed surface), then
  // move to y=40 → slot floor(40/32)=1 (a MIDDLE slot, distinct from the
  // bottom-clamp value the old bug produced), i.e. order [1,2,3] → [2,1,3].
  function dragFolderOneToSlotOne() {
    act(() => firePointer(grip(1), "pointerdown", { clientY: 100 }))
    // Threshold crossing flips into drag mode. The surface is not mounted yet,
    // so this move must NOT retarget (the regression Codex flagged).
    act(() => firePointer(window, "pointermove", { clientY: 120 }))
    // Surface mounted now → retarget to slot 1.
    act(() => firePointer(window, "pointermove", { clientY: 40 }))
  }

  it("commits the reorder to the targeted slot on pointerup", async () => {
    render(tree())
    dragFolderOneToSlotOne()
    await act(async () => {
      firePointer(window, "pointerup", { clientY: 40 })
    })
    expect(stableWorkspaceFns.reorderFolders).toHaveBeenCalledTimes(1)
    // A middle slot — not the last — so this can only pass with correct
    // surface-relative targeting, not the old bottom-clamp behavior.
    expect(stableWorkspaceFns.reorderFolders).toHaveBeenCalledWith([2, 1, 3])
  })

  it("does not reorder when released right after crossing the threshold (before the surface can retarget)", async () => {
    render(tree())
    act(() => firePointer(grip(1), "pointerdown", { clientY: 100 }))
    // Cross the threshold from a 'scrolled' position, then release immediately.
    // The collapsed surface mounts only after this move, so there is no valid
    // target yet — the old viewport-fallback would have bottom-clamped here.
    act(() => firePointer(window, "pointermove", { clientY: 200 }))
    await act(async () => {
      firePointer(window, "pointerup", { clientY: 200 })
    })
    expect(stableWorkspaceFns.reorderFolders).not.toHaveBeenCalled()
  })

  it("aborts without persisting on pointercancel", () => {
    render(tree())
    dragFolderOneToSlotOne()
    act(() => firePointer(window, "pointercancel", { clientY: 40 }))
    expect(stableWorkspaceFns.reorderFolders).not.toHaveBeenCalled()
  })

  it("aborts without persisting on Escape", () => {
    render(tree())
    dragFolderOneToSlotOne()
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true })
      )
    })
    expect(stableWorkspaceFns.reorderFolders).not.toHaveBeenCalled()
  })

  it("does nothing when the press never crosses the drag threshold", async () => {
    render(tree())
    act(() => firePointer(grip(1), "pointerdown", { clientY: 100 }))
    act(() => firePointer(window, "pointermove", { clientY: 103 })) // 3px < 6px
    await act(async () => {
      firePointer(window, "pointerup", { clientY: 103 })
    })
    expect(stableWorkspaceFns.reorderFolders).not.toHaveBeenCalled()
  })
})

// Drives the sticky overlay via the controllable virtua handle. The overlay is
// resolved from the layout effect at mount (no scroll event needed): set
// virtuaCtl.scrollOffset before render and assert the duplicated header. Real
// virtua scrolling / handoff smoothness still needs manual QA.
describe("SidebarConversationList — sticky folder header overlay", () => {
  beforeEach(() => {
    localStorage.clear() // folderExpanded persists across tests otherwise
    store.folders = [folder(1, "Folder 1"), folder(2, "Folder 2")]
    store.allFolders = store.folders
    // rows: F1(0) c11(1) c12(2) F2(3) c21(4) c22(5) c23(6)
    store.conversations = [
      conv(11, 1),
      conv(12, 1),
      conv(21, 2),
      conv(22, 2),
      conv(23, 2),
    ]
    store.activeTabId = null
    store.tabSpec = []
  })

  function headerCount(folderId: number): number {
    return document.querySelectorAll(`[data-folder-id="${folderId}"]`).length
  }

  it("hides the overlay at the top of the list", () => {
    virtuaCtl.scrollOffset = 0
    render(tree())
    // Only the real in-list header exists for each folder.
    expect(headerCount(1)).toBe(1)
    expect(headerCount(2)).toBe(1)
  })

  it("shows a sticky overlay for the folder scrolled through", () => {
    virtuaCtl.scrollOffset = 40 // past F1's header (offset 0), inside conv 11
    render(tree())
    // Folder 1 header is duplicated in the DOM (in-list + overlay); folder 2 is
    // not.
    expect(headerCount(1)).toBe(2)
    expect(headerCount(2)).toBe(1)
    // Only one of the two is accessible: the in-list copy is suppressed
    // (inert + aria-hidden) so the overlay is the sole tab stop / announcement.
    const f1 = document.querySelectorAll('[data-folder-id="1"]')
    expect(
      (f1[0] as HTMLElement).closest('[aria-hidden="true"]')
    ).not.toBeNull()
    expect((f1[1] as HTMLElement).closest('[aria-hidden="true"]')).toBeNull()
    // The accessible (overlay) toggle exposes its expanded state to AT.
    expect((f1[1] as HTMLElement).getAttribute("aria-expanded")).toBe("true")
  })

  it("tracks the active folder as the scroll moves into the next folder", () => {
    virtuaCtl.scrollOffset = 130 // inside folder 2 (F2 header at offset 96)
    render(tree())
    expect(headerCount(1)).toBe(1)
    expect(headerCount(2)).toBe(2)
  })

  it("collapses from the overlay and scrolls the folder header to the top", () => {
    // rAF runs synchronously so the deferred scrollToIndex is observable.
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      cb(0)
      return 0
    })
    try {
      virtuaCtl.scrollOffset = 130 // overlay shows folder 2
      render(tree())
      const headers = document.querySelectorAll('[data-folder-id="2"]')
      expect(headers.length).toBe(2)
      // headers[1] is the overlay copy (rendered after ScrollArea in DOM order).
      act(() => {
        fireEvent.click(headers[1] as HTMLElement)
      })
      // The folder collapsed (its conversation rows are gone).
      expect(document.body.textContent).not.toContain("conv-21")
      // The "Folders" section header occupies flat index 0, so folder 2's header
      // is flat index 4 → scrolled to the top, instant.
      expect(virtuaCtl.scrollToIndex).toHaveBeenCalledWith(
        4,
        expect.objectContaining({ align: "start" })
      )
    } finally {
      vi.unstubAllGlobals()
    }
  })

  it("hides the overlay while a folder drag is in progress", () => {
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        top: 0,
        bottom: 600,
        left: 0,
        right: 200,
        width: 200,
        height: 600,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      } as DOMRect)
    try {
      virtuaCtl.scrollOffset = 40 // overlay shows folder 1
      render(tree())
      expect(headerCount(1)).toBe(2) // suppressed in-list + overlay
      // Drag a NON-sticky folder (folder 2) from its in-list header — folder 1's
      // in-list header is inert while its overlay is showing, and the overlay
      // itself has no drag grip.
      const grip = (
        document.querySelector('[data-folder-id="2"]') as HTMLElement
      ).parentElement as HTMLElement
      act(() => firePointer(grip, "pointerdown", { clientY: 100 }))
      act(() => firePointer(window, "pointermove", { clientY: 120 })) // cross 6px
      // Virtualizer unmounted → drag surface shows each folder once, overlay gone.
      expect(headerCount(1)).toBe(1)
      act(() => firePointer(window, "pointercancel", { clientY: 120 }))
    } finally {
      rectSpy.mockRestore()
    }
  })
})

describe("SidebarConversationList — scrollToActive across a worktree merge", () => {
  const EXPANDED_KEY = "workspace:sidebar-folder-expanded"

  beforeEach(() => {
    // Root folder 1 + worktree child folder 2 (parent_id = 1), one conversation
    // in each. Select the worktree conversation via the active tab.
    store.folders = [folder(1, "Root"), folder(2, "Worktree", 1)]
    store.allFolders = store.folders
    store.conversations = [conv(11, 1), conv(21, 2)]
    store.activeTabId = "tab-21"
    store.tabSpec = [
      {
        id: "tab-21",
        conversationId: 21,
        agentType: "claude_code",
        folderId: 2,
        title: "conv-21",
        isPinned: false,
      },
    ]
    // Collapse the parent (root) group so the merged worktree row is initially
    // absent from the flat model.
    localStorage.setItem(EXPANDED_KEY, JSON.stringify({ 1: false }))
  })

  afterEach(() => {
    localStorage.removeItem(EXPANDED_KEY)
  })

  it("expands the parent group to reveal and scroll to a merged worktree conversation", () => {
    const ref = createRef<SidebarConversationListHandle>()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <SidebarConversationList showCompleted sortMode="created" ref={ref} />
      </NextIntlClientProvider>
    )

    // Parent collapsed → the worktree row is not in the flat model, so no scroll
    // can resolve yet.
    expect(virtuaCtl.scrollToIndex).not.toHaveBeenCalled()

    act(() => {
      ref.current?.scrollToActive()
    })

    // The fix resolves the *display group* (parent folder 1), expands it, and the
    // deferred scroll then finds the worktree row. Pre-fix this stayed at 0
    // because it checked/expanded the child folder id (2) — never a rendered
    // group — so the row never entered the flat model.
    expect(virtuaCtl.scrollToIndex).toHaveBeenCalled()
  })
})

describe("SidebarConversationList — folder ⋯ opens the same menu as right-click", () => {
  beforeEach(() => {
    probes.card = 0
    probes.folder = 0
    store.folders = [folder(1, "Folder 1")]
    store.allFolders = store.folders
    store.conversations = [conv(11, 1)]
    store.activeTabId = null
    store.tabSpec = []
  })

  it("opens the folder context menu via the ⋯ button — no right-click needed", () => {
    render(tree())
    // Closed: Radix mounts the menu content lazily, so its items aren't present.
    expect(document.body.textContent).not.toContain("Manage conversations")

    // The ⋯ button dispatches a synthetic `contextmenu` event that bubbles to the
    // same <ContextMenuTrigger> the right-click uses — single source of truth.
    const moreBtn = document.querySelector('[aria-label="More options"]')
    expect(moreBtn).not.toBeNull()
    act(() => {
      fireEvent.click(moreBtn as HTMLElement)
    })

    // The identical menu is now open — assert a label unique to the folder menu.
    expect(document.body.textContent).toContain("Manage conversations")
  })
})
