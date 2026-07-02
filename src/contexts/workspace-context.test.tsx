import { act, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  WorkspaceProvider,
  useWorkspaceActions,
  useWorkspaceContext,
  useWorkspaceExternalConflict,
  useWorkspaceFileTabs,
  useWorkspaceView,
} from "@/contexts/workspace-context"
import * as api from "@/lib/api"

vi.mock("next-intl", () => {
  // Return a STABLE function instance across renders, mirroring next-intl's
  // real behavior. An unstable `t` would churn every action callback that
  // lists it as a dependency, silently breaking the actions-context
  // stability the render-isolation suite asserts.
  const t = (key: string, values?: Record<string, string>) =>
    values ? `${key}:${JSON.stringify(values)}` : key
  return { useTranslations: () => t }
})

// Dynamic folder store shared by the active-folder and app-workspace mocks.
// Tests mutate it (switch active folder, remove folders, toggle hydration)
// inside act() and consumers re-render via useSyncExternalStore.
const foldersMock = vi.hoisted(() => {
  type FolderLike = { id: number; path: string; name: string; color: string }
  const defaultFolders = (): FolderLike[] => [
    { id: 1, path: "/repo", name: "repo", color: "inherit" },
    { id: 2, path: "/repo2", name: "repo2", color: "inherit" },
  ]
  let allFolders = defaultFolders()
  let foldersHydrated = true
  let activeFolderId: number | null = 1
  let snapshot: {
    allFolders: FolderLike[]
    foldersHydrated: boolean
    activeFolderId: number | null
  } = { allFolders, foldersHydrated, activeFolderId }
  const listeners = new Set<() => void>()
  const commit = () => {
    snapshot = { allFolders, foldersHydrated, activeFolderId }
    for (const listener of [...listeners]) listener()
  }
  return {
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    getSnapshot: () => snapshot,
    setAllFolders(next: FolderLike[]) {
      allFolders = next
      commit()
    },
    setHydrated(value: boolean) {
      foldersHydrated = value
      commit()
    },
    setActiveFolderId(id: number | null) {
      activeFolderId = id
      commit()
    },
    reset() {
      allFolders = defaultFolders()
      foldersHydrated = true
      activeFolderId = 1
      commit()
    },
  }
})

vi.mock("@/contexts/active-folder-context", async () => {
  const { useSyncExternalStore } = await import("react")
  return {
    useActiveFolder: () => {
      const snap = useSyncExternalStore(
        foldersMock.subscribe,
        foldersMock.getSnapshot,
        foldersMock.getSnapshot
      )
      const activeFolder =
        snap.allFolders.find((f) => f.id === snap.activeFolderId) ?? null
      return { activeFolder, activeFolderId: snap.activeFolderId }
    },
  }
})

vi.mock("@/contexts/app-workspace-context", async () => {
  const { useSyncExternalStore } = await import("react")
  return {
    useAppWorkspace: () => {
      const snap = useSyncExternalStore(
        foldersMock.subscribe,
        foldersMock.getSnapshot,
        foldersMock.getSnapshot
      )
      return {
        allFolders: snap.allFolders,
        foldersHydrated: snap.foldersHydrated,
        getFolder: (id: number) => snap.allFolders.find((f) => f.id === id),
      }
    },
  }
})

beforeEach(() => {
  foldersMock.reset()
})

vi.mock("@/lib/api", () => ({
  readFileForEdit: vi.fn(),
  readFileBase64: vi.fn(),
  readFilePreview: vi.fn(),
  gitIsTracked: vi.fn(),
  gitShowFile: vi.fn(),
  gitDiff: vi.fn(),
  gitDiffWithBranch: vi.fn(),
  gitShowDiff: vi.fn(),
  saveFileContent: vi.fn(),
  saveFileCopy: vi.fn(),
}))

// Controllable workspace-state store: WorkspaceProvider subscribes to its
// envelopes to auto-open office previews (hook path), and the tab watcher
// acquires per-root imperative stores (getWorkspaceStateStore path).
// `emitEnvelope` pushes an event on the office/hook stream; `emitRoot`
// pushes on a specific root's imperative store as if that folder's file
// watcher fired. Acquire/release totals are tracked per root so tests can
// assert subscription stability (no churn on keystrokes).
const workspaceStoreMock = vi.hoisted(() => {
  type EnvelopeListener = (env: {
    seq: number
    kind: string
    changed_paths: string[]
  }) => void
  const listeners = new Set<EnvelopeListener>()
  interface FakeRootStore {
    acquired: number
    acquireCalls: number
    listeners: Set<EnvelopeListener>
    store: {
      acquire: () => void
      release: () => void
      subscribeEnvelopes: (listener: EnvelopeListener) => () => void
    }
  }
  const storeByRoot = new Map<string, FakeRootStore>()
  const getRootEntry = (rootPath: string): FakeRootStore => {
    let entry = storeByRoot.get(rootPath)
    if (!entry) {
      const rootListeners = new Set<EnvelopeListener>()
      const created: FakeRootStore = {
        acquired: 0,
        acquireCalls: 0,
        listeners: rootListeners,
        store: {
          acquire: () => {
            created.acquired += 1
            created.acquireCalls += 1
          },
          release: () => {
            created.acquired -= 1
          },
          subscribeEnvelopes: (listener: EnvelopeListener) => {
            rootListeners.add(listener)
            return () => {
              rootListeners.delete(listener)
            }
          },
        },
      }
      entry = created
      storeByRoot.set(rootPath, entry)
    }
    return entry
  }
  return {
    subscribeEnvelopes: (listener: EnvelopeListener) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    emitEnvelope: (changed_paths: string[]) => {
      for (const listener of [...listeners]) {
        listener({ seq: 1, kind: "fs_change", changed_paths })
      }
    },
    getStore: (rootPath: string) => getRootEntry(rootPath).store,
    emitRoot: (
      rootPath: string,
      changed_paths: string[],
      kind = "fs_change"
    ) => {
      const entry = storeByRoot.get(rootPath)
      if (!entry) return
      for (const listener of [...entry.listeners]) {
        listener({ seq: 1, kind, changed_paths })
      }
    },
    acquiredCount: (rootPath: string) =>
      storeByRoot.get(rootPath)?.acquired ?? 0,
    acquireCalls: (rootPath: string) =>
      storeByRoot.get(rootPath)?.acquireCalls ?? 0,
    reset: () => {
      listeners.clear()
      storeByRoot.clear()
    },
  }
})

vi.mock("@/hooks/use-workspace-state-store", () => ({
  useWorkspaceStateStore: () => ({
    rootPath: "/repo",
    seq: 0,
    version: 1,
    health: "healthy" as const,
    tree: [],
    git: [],
    error: null,
    degraded: false,
    isGitRepo: true,
    requestResync: async () => {},
    restart: async () => {},
    subscribeEnvelopes: workspaceStoreMock.subscribeEnvelopes,
  }),
  getWorkspaceStateStore: (rootPath: string) =>
    workspaceStoreMock.getStore(rootPath),
}))

beforeEach(() => {
  workspaceStoreMock.reset()
})

const mockedApi = api as unknown as {
  readFileForEdit: ReturnType<typeof vi.fn>
  gitIsTracked: ReturnType<typeof vi.fn>
  gitShowFile: ReturnType<typeof vi.fn>
  saveFileContent: ReturnType<typeof vi.fn>
}

function WorkspaceProbe() {
  const {
    mode,
    activePane,
    fileTabs,
    activeFileTabId,
    filesMaximized,
    openSessionFileDiff,
    closeFileTab,
    closeAllFileTabs,
    toggleFilesMaximized,
    activateConversationPane,
  } = useWorkspaceContext()

  return (
    <div>
      <output data-testid="mode">{mode}</output>
      <output data-testid="file-tab-count">{fileTabs.length}</output>
      <output data-testid="active-pane">{activePane}</output>
      <output data-testid="files-maximized">{String(filesMaximized)}</output>
      <output data-testid="active-file-tab">{activeFileTabId ?? "none"}</output>
      <button
        type="button"
        onClick={() =>
          openSessionFileDiff("src/app.ts", "diff --git", "Turn 1")
        }
      >
        Open diff
      </button>
      <button
        type="button"
        onClick={() =>
          openSessionFileDiff("src/other.ts", "diff --git", "Turn 1")
        }
      >
        Open diff 2
      </button>
      <button
        type="button"
        onClick={() => activeFileTabId && closeFileTab(activeFileTabId)}
      >
        Close active
      </button>
      <button type="button" onClick={closeAllFileTabs}>
        Close all
      </button>
      <button type="button" onClick={toggleFilesMaximized}>
        Toggle maximize
      </button>
      <button type="button" onClick={activateConversationPane}>
        Activate conversation
      </button>
    </div>
  )
}

function renderWorkspace() {
  return render(
    <WorkspaceProvider>
      <WorkspaceProbe />
    </WorkspaceProvider>
  )
}

describe("WorkspaceProvider mode", () => {
  it("derives conversation mode from an empty file workspace", () => {
    localStorage.setItem("workspace:mode", JSON.stringify({ mode: "files" }))

    renderWorkspace()

    expect(screen.getByTestId("mode")).toHaveTextContent("conversation")
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
  })

  it("derives fusion mode while file tabs are open and returns to conversation when they close", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })

    expect(screen.getByTestId("mode")).toHaveTextContent("fusion")
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("1")

    act(() => {
      screen.getByRole("button", { name: "Close all" }).click()
    })

    expect(screen.getByTestId("mode")).toHaveTextContent("conversation")
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
  })
})

describe("WorkspaceProvider files-maximized", () => {
  it("toggles filesMaximized only while files are open", () => {
    renderWorkspace()

    // No files yet — toggling should not enable maximize (derived value gated
    // on fusion mode).
    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")

    // Open a file, then toggle: maximize flips on, then off.
    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    expect(screen.getByTestId("mode")).toHaveTextContent("fusion")

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("true")

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")
  })

  it("does not mutate active pane on maximize toggle, preserving revert semantics", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    // Opening a file routes activePane to "files".
    expect(screen.getByTestId("active-pane")).toHaveTextContent("files")

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    // Maximize must not silently rewrite the user's last-active pane.
    expect(screen.getByTestId("active-pane")).toHaveTextContent("files")
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("true")

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("active-pane")).toHaveTextContent("files")
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")
  })

  it("resets filesMaximized when all file tabs close, and does not leak into newly reopened files", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("true")

    act(() => {
      screen.getByRole("button", { name: "Close all" }).click()
    })
    expect(screen.getByTestId("mode")).toHaveTextContent("conversation")
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")

    // Reopening a file must start from the normal split, not a stale maximized
    // layout.
    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    expect(screen.getByTestId("mode")).toHaveTextContent("fusion")
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")
  })

  it("resets filesMaximized when the last tab is closed individually", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("true")

    act(() => {
      screen.getByRole("button", { name: "Close active" }).click()
    })
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")
  })

  it("exits filesMaximized when activating the conversation pane while files stay open", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
    })
    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("true")
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("1")

    // Mirrors what happens when the user opens a session from the sidebar:
    // TabContext.openTab -> activateConversationPane(). The overlay must
    // release so the conversation becomes visible, but the file tab itself
    // must remain so the user can return to it later.
    act(() => {
      screen.getByRole("button", { name: "Activate conversation" }).click()
    })
    expect(screen.getByTestId("files-maximized")).toHaveTextContent("false")
    expect(screen.getByTestId("active-pane")).toHaveTextContent("conversation")
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("1")
    expect(screen.getByTestId("mode")).toHaveTextContent("fusion")
  })

  it("does not touch file tab data when toggling maximize", () => {
    renderWorkspace()

    act(() => {
      screen.getByRole("button", { name: "Open diff" }).click()
      screen.getByRole("button", { name: "Open diff 2" }).click()
    })
    const tabCountBefore =
      screen.getByTestId("file-tab-count").textContent ?? ""
    const activeBefore = screen.getByTestId("active-file-tab").textContent ?? ""

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent(
      tabCountBefore
    )
    expect(screen.getByTestId("active-file-tab")).toHaveTextContent(
      activeBefore
    )

    act(() => {
      screen.getByRole("button", { name: "Toggle maximize" }).click()
    })
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent(
      tabCountBefore
    )
    expect(screen.getByTestId("active-file-tab")).toHaveTextContent(
      activeBefore
    )
  })
})

interface CapturedTab {
  content: string
  loading: boolean
  saveState?: string
}

function FilePreviewProbe({
  onCapture,
}: {
  onCapture?: (tab: CapturedTab | null) => void
}) {
  const { openFilePreview, activeFileTab } = useWorkspaceContext()
  const snapshot: CapturedTab | null = activeFileTab
    ? {
        content: activeFileTab.content,
        loading: activeFileTab.loading,
        saveState: activeFileTab.saveState,
      }
    : null
  onCapture?.(snapshot)
  return (
    <div>
      <output data-testid="content">{activeFileTab?.content ?? ""}</output>
      <output data-testid="loading">
        {String(activeFileTab?.loading ?? false)}
      </output>
      <output data-testid="save-state">
        {activeFileTab?.saveState ?? "none"}
      </output>
      <button onClick={() => void openFilePreview("a.ts")}>open</button>
      <button onClick={() => void openFilePreview("a.ts", { reload: true })}>
        reload
      </button>
    </div>
  )
}

describe("openFilePreview cache semantics", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  it("activates an already-loaded tab without refetching", async () => {
    mockedApi.readFileForEdit.mockResolvedValue({
      path: "a.ts",
      content: "hello",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    render(
      <WorkspaceProvider>
        <FilePreviewProbe />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open").click()
    })
    expect(screen.getByTestId("content")).toHaveTextContent("hello")
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)

    // Second click on the same file — pure cache hit.
    await act(async () => {
      screen.getByText("open").click()
    })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)
    expect(screen.getByTestId("loading")).toHaveTextContent("false")
    expect(screen.getByTestId("content")).toHaveTextContent("hello")
  })

  it("forces refetch when reload: true and preserves content during fetch", async () => {
    let resolveSecond:
      | ((v: {
          path: string
          content: string
          etag: string
          mtime_ms: number
          readonly: boolean
          line_ending: "lf"
        }) => void)
      | null = null
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v1",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockImplementationOnce(() => new Promise((res) => (resolveSecond = res)))

    let captured = null as CapturedTab | null
    render(
      <WorkspaceProvider>
        <FilePreviewProbe onCapture={(t) => (captured = t)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open").click()
    })
    expect(captured).toMatchObject({ content: "v1", loading: false })

    await act(async () => {
      screen.getByText("reload").click()
    })
    // Mid-fetch: content preserved, loading true.
    expect(captured).toMatchObject({ content: "v1", loading: true })

    await act(async () => {
      resolveSecond!({
        path: "a.ts",
        content: "v2",
        etag: "e2",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })
    })
    expect(captured).toMatchObject({ content: "v2", loading: false })
  })

  it("deduplicates concurrent opens of the same path", async () => {
    let resolveFirst:
      | ((v: {
          path: string
          content: string
          etag: string
          mtime_ms: number
          readonly: boolean
          line_ending: "lf"
        }) => void)
      | null = null
    mockedApi.readFileForEdit.mockImplementationOnce(
      () => new Promise((res) => (resolveFirst = res))
    )

    render(
      <WorkspaceProvider>
        <FilePreviewProbe />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open").click()
      screen.getByText("open").click()
      screen.getByText("open").click()
    })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)

    await act(async () => {
      resolveFirst!({
        path: "a.ts",
        content: "x",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
    })
  })

  it("retries after an error and clears the error state", async () => {
    mockedApi.readFileForEdit
      .mockRejectedValueOnce(new Error("boom"))
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "ok",
        etag: "e",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })

    let captured = null as CapturedTab | null
    render(
      <WorkspaceProvider>
        <FilePreviewProbe onCapture={(t) => (captured = t)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open").click()
    })
    expect(captured?.saveState).toBe("error")

    await act(async () => {
      screen.getByText("open").click()
    })
    expect(captured).toMatchObject({
      content: "ok",
      loading: false,
      saveState: "idle",
    })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(2)
  })

  it("does not resurrect a closed tab when reload: true arrives late", async () => {
    mockedApi.readFileForEdit.mockResolvedValue({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    function Probe() {
      const { openFilePreview, fileTabs, activeFileTabId, closeAllFileTabs } =
        useWorkspaceContext()
      return (
        <div>
          <output data-testid="tab-count">{fileTabs.length}</output>
          <output data-testid="active-id">{activeFileTabId ?? "none"}</output>
          <button onClick={() => void openFilePreview("a.ts")}>open</button>
          <button
            onClick={() => void openFilePreview("a.ts", { reload: true })}
          >
            reload
          </button>
          <button onClick={closeAllFileTabs}>close all</button>
        </div>
      )
    }

    render(
      <WorkspaceProvider>
        <Probe />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open").click()
    })
    expect(screen.getByTestId("tab-count")).toHaveTextContent("1")

    await act(async () => {
      screen.getByText("close all").click()
    })
    expect(screen.getByTestId("tab-count")).toHaveTextContent("0")

    // Simulate a watcher-driven reload that lands after the user closed
    // the tab. The reload should be a no-op — never create a phantom tab.
    await act(async () => {
      screen.getByText("reload").click()
    })
    expect(screen.getByTestId("tab-count")).toHaveTextContent("0")
    expect(screen.getByTestId("active-id")).toHaveTextContent("none")
    // The closed-and-reload sequence triggered only the initial open.
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)
  })

  it("clears in-flight tracking on close so reopen is not falsely deduped", async () => {
    let resolveFirst:
      | ((v: {
          path: string
          content: string
          etag: string
          mtime_ms: number
          readonly: boolean
          line_ending: "lf"
        }) => void)
      | null = null
    mockedApi.readFileForEdit
      .mockImplementationOnce(() => new Promise((res) => (resolveFirst = res)))
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v2",
        etag: "e2",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })

    let captured = null as CapturedTab | null
    function Probe({
      onCapture,
    }: {
      onCapture: (tab: CapturedTab | null) => void
    }) {
      const { openFilePreview, activeFileTab, closeAllFileTabs } =
        useWorkspaceContext()
      onCapture(
        activeFileTab
          ? {
              content: activeFileTab.content,
              loading: activeFileTab.loading,
              saveState: activeFileTab.saveState,
            }
          : null
      )
      return (
        <div>
          <button onClick={() => void openFilePreview("a.ts")}>open</button>
          <button onClick={closeAllFileTabs}>close all</button>
        </div>
      )
    }

    render(
      <WorkspaceProvider>
        <Probe onCapture={(t) => (captured = t)} />
      </WorkspaceProvider>
    )

    // Start a load and immediately close (load is still pending).
    await act(async () => {
      screen.getByText("open").click()
    })
    await act(async () => {
      screen.getByText("close all").click()
    })

    // Reopen — the stale in-flight marker should have been cleared, so
    // the second fetch must run and populate content (not get deduped).
    await act(async () => {
      screen.getByText("open").click()
    })

    // Drain the original (now-orphaned) fetch — it should no-op since
    // its target tab id was removed during close.
    await act(async () => {
      resolveFirst!({
        path: "a.ts",
        content: "v1",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
    })

    expect(captured).toMatchObject({ content: "v2", loading: false })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(2)
  })
})

interface BackgroundProbeSnapshot {
  activeId: string | null
  tabs: Array<{
    id: string
    path: string | null
    content: string
    isDirty: boolean
    stale: boolean
  }>
}

function BackgroundReloadProbe({
  onCapture,
}: {
  onCapture: (snapshot: BackgroundProbeSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    activeFileTabId,
    reloadOpenFileBackground,
    markTabsStale,
    updateActiveFileContent,
    switchFileTab,
  } = useWorkspaceContext()
  onCapture({
    activeId: activeFileTabId,
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      path: tab.path,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button onClick={() => void openFilePreview("b.ts")}>open-b</button>
      <button onClick={() => void reloadOpenFileBackground(1, "a.ts")}>
        bg-reload-a
      </button>
      <button onClick={() => markTabsStale(1, "a.ts")}>stale-a</button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => switchFileTab("file:1:a.ts")}>switch-a</button>
    </div>
  )
}

describe("background reload + stale semantics", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  it("reloadOpenFileBackground refreshes content without changing activeFileTabId", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "b.ts",
        content: "b-v1",
        etag: "eb1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v2",
        etag: "ea2",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })

    let snap: BackgroundProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <BackgroundReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    expect(snap.activeId).toBe("file:1:b.ts")

    await act(async () => {
      screen.getByText("bg-reload-a").click()
    })

    // active tab stays on B; tab A content refreshed in place.
    expect(snap.activeId).toBe("file:1:b.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("a-v2")
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(3)
  })

  it("markTabsStale flips stale=true on the matching non-active tab", async () => {
    mockedApi.readFileForEdit.mockResolvedValue({
      path: "a.ts",
      content: "a-v1",
      etag: "ea1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: BackgroundProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <BackgroundReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    // Open A, then B — A becomes a background tab. (Marking the ACTIVE
    // tab stale is immediately auto-resolved by the provider watcher's
    // stale-on-activation pass, so the flag would not stay observable.)
    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.stale).toBe(false)

    await act(async () => {
      screen.getByText("stale-a").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.stale).toBe(true)
  })

  it("activates a stale clean tab and refetches as if reload:true was passed", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "b.ts",
        content: "b-v1",
        etag: "eb1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v2",
        etag: "ea2",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })

    let snap: BackgroundProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <BackgroundReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    await act(async () => {
      screen.getByText("stale-a").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.stale).toBe(true)

    // Plain activation (no reload option) must still refetch because stale.
    await act(async () => {
      screen.getByText("open-a").click()
    })

    expect(snap.activeId).toBe("file:1:a.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("a-v2")
    expect(tabA?.stale).toBe(false)
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(3)
  })

  it("activates a stale dirty tab without overwriting local edits", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "b.ts",
        content: "b-v1",
        etag: "eb1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      // Conflict-detection read fired by stale-on-activation for the
      // dirty tab: disk is UNCHANGED (same etag), so no conflict and no
      // content overwrite.
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })

    let snap: BackgroundProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <BackgroundReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    await act(async () => {
      screen.getByText("stale-a").click()
    })

    const callsBefore = mockedApi.readFileForEdit.mock.calls.length

    // Activate the dirty stale tab. The watcher runs ONE conflict-check
    // read (never a content refetch) — unsaved edits must survive.
    await act(async () => {
      screen.getByText("switch-a").click()
    })

    expect(snap.activeId).toBe("file:1:a.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
    expect(tabA?.stale).toBe(true)
    expect(mockedApi.readFileForEdit.mock.calls.length).toBe(callsBefore + 1)
  })

  it("reloadOpenFileBackground is a no-op when the path is not open", async () => {
    let snap: BackgroundProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <BackgroundReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("bg-reload-a").click()
    })

    expect(snap.tabs).toHaveLength(0)
    expect(mockedApi.readFileForEdit).not.toHaveBeenCalled()
  })
})

interface ApplyExternalProbeSnapshot {
  activeId: string | null
  tabs: Array<{
    id: string
    content: string
    etag: string | null | undefined
    isDirty: boolean
    stale: boolean
    loading: boolean
  }>
}

function ApplyExternalReloadProbe({
  onCapture,
}: {
  onCapture: (snapshot: ApplyExternalProbeSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    activeFileTabId,
    applyExternalReload,
    updateActiveFileContent,
    markTabsStale,
  } = useWorkspaceContext()
  onCapture({
    activeId: activeFileTabId,
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      content: tab.content,
      etag: tab.etag,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
      loading: tab.loading,
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button onClick={() => void openFilePreview("b.ts")}>open-b</button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => markTabsStale(1, "a.ts")}>stale-a</button>
      <button
        onClick={() =>
          void applyExternalReload(1, "a.ts", {
            path: "a.ts",
            content: "ext-content",
            etag: "ext-etag",
            mtime_ms: 99,
            readonly: false,
            line_ending: "lf",
          })
        }
      >
        apply-a
      </button>
      <button
        onClick={() =>
          void applyExternalReload(1, "missing.ts", {
            path: "missing.ts",
            content: "x",
            etag: "x",
            mtime_ms: 1,
            readonly: false,
            line_ending: "lf",
          })
        }
      >
        apply-missing
      </button>
    </div>
  )
}

describe("applyExternalReload prefetched-write semantics", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  it("writes prefetched content into the matching tab without a second readFileForEdit", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "a-v1",
      etag: "ea1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: ApplyExternalProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyExternalReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("ext-content")
    expect(tabA?.etag).toBe("ext-etag")
    expect(tabA?.loading).toBe(false)
    // The whole point: the prefetched payload is the source of truth, no
    // additional file read is issued.
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)
  })

  it("does not change activeFileTabId when reloading a non-active tab", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "b.ts",
        content: "b-v1",
        etag: "eb1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })

    let snap: ApplyExternalProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyExternalReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    expect(snap.activeId).toBe("file:1:b.ts")

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    expect(snap.activeId).toBe("file:1:b.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("ext-content")
  })

  it("refuses to overwrite a dirty tab", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "a-v1",
      etag: "ea1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: ApplyExternalProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyExternalReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
  })

  it("clears stale=true on a successful apply", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "a-v1",
        etag: "ea1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "b.ts",
        content: "b-v1",
        etag: "eb1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })

    let snap: ApplyExternalProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyExternalReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    // A must be a BACKGROUND tab when marked stale — an active clean
    // stale tab is auto-reloaded by the watcher before apply could run.
    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("open-b").click()
    })
    await act(async () => {
      screen.getByText("stale-a").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.stale).toBe(true)

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.stale).toBe(false)
    expect(tabA?.content).toBe("ext-content")
  })

  it("is a no-op when the path has no open tab", async () => {
    let snap: ApplyExternalProbeSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyExternalReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("apply-missing").click()
    })

    expect(snap.tabs).toHaveLength(0)
    expect(mockedApi.readFileForEdit).not.toHaveBeenCalled()
  })
})

interface RejectFileTabSnapshot {
  activeId: string | null
  tabs: Array<{
    id: string
    content: string
    isDirty: boolean
    saveState?: string
    saveError?: string | null
    loading: boolean
  }>
}

function RejectFileTabProbe({
  onCapture,
}: {
  onCapture: (snapshot: RejectFileTabSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    activeFileTabId,
    rejectFileTab,
    updateActiveFileContent,
  } = useWorkspaceContext()
  onCapture({
    activeId: activeFileTabId,
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      saveState: tab.saveState,
      saveError: tab.saveError ?? null,
      loading: tab.loading,
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => rejectFileTab(1, "a.ts", "ENOENT: file removed")}>
        reject-a
      </button>
      <button onClick={() => rejectFileTab(1, "missing.ts", "irrelevant")}>
        reject-missing
      </button>
    </div>
  )
}

describe("rejectFileTab missing-on-disk semantics", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  it("replaces a clean tab's content with the supplied error and marks it errored", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "fresh",
      etag: "ea1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: RejectFileTabSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <RejectFileTabProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    expect(snap.tabs[0]?.content).toBe("fresh")

    await act(async () => {
      screen.getByText("reject-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.saveState).toBe("error")
    expect(tabA?.saveError).toBe("ENOENT: file removed")
    expect(tabA?.loading).toBe(false)
    // Original content should no longer be presented as the file's body —
    // an error message stands in its place so the user is never silently
    // shown a buffer that no longer matches disk.
    expect(tabA?.content).not.toBe("fresh")
    expect(tabA?.content.length).toBeGreaterThan(0)
  })

  it("refuses to touch a dirty tab so unsaved edits survive an external delete", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "ea1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: RejectFileTabSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <RejectFileTabProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    expect(snap.tabs[0]?.isDirty).toBe(true)
    expect(snap.tabs[0]?.content).toBe("dirty-local")

    await act(async () => {
      screen.getByText("reject-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
    // saveError stays null — only the watcher's markTabsStale path is
    // responsible for surfacing the read failure to the user when the
    // buffer is dirty.
    expect(tabA?.saveError).toBeNull()
  })

  it("is a no-op when the path has no open tab", async () => {
    let snap: RejectFileTabSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <RejectFileTabProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("reject-missing").click()
    })

    expect(snap.tabs).toHaveLength(0)
  })
})

interface GitHangProbeSnapshot {
  tabs: Array<{ id: string; content: string }>
}

function ApplyThenReloadProbe({
  onCapture,
}: {
  onCapture: (snapshot: GitHangProbeSnapshot) => void
}) {
  const { openFilePreview, fileTabs, applyExternalReload } =
    useWorkspaceContext()
  onCapture({
    tabs: fileTabs.map((tab) => ({ id: tab.id, content: tab.content })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button
        onClick={() =>
          void applyExternalReload(1, "a.ts", {
            path: "a.ts",
            content: "ext-content",
            etag: "ext-etag",
            mtime_ms: 99,
            readonly: false,
            line_ending: "lf",
          })
        }
      >
        apply-a
      </button>
      <button onClick={() => void openFilePreview("a.ts", { reload: true })}>
        reload-a
      </button>
    </div>
  )
}

describe("applyExternalReload does not block subsequent reloads on slow git base", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
  })

  it("releases the in-flight marker before awaiting git base so a foreground reload still fires", async () => {
    // First open: git not tracked → no gitShowFile.
    mockedApi.gitIsTracked.mockResolvedValueOnce(false)
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v1",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v3",
        etag: "e3",
        mtime_ms: 3,
        readonly: false,
        line_ending: "lf",
      })

    let snap: GitHangProbeSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyThenReloadProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(1)

    // Now simulate a stuck git: subsequent gitIsTracked/gitShowFile hang
    // forever. applyExternalReload's git base refresh must not block
    // user-initiated reload via the inFlightLoadsRef dedup path.
    mockedApi.gitIsTracked.mockImplementation(() => new Promise(() => {}))
    mockedApi.gitShowFile.mockImplementation(() => new Promise(() => {}))

    await act(async () => {
      screen.getByText("apply-a").click()
    })
    // Content was written from the prefetched payload despite hung git.
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.content).toBe(
      "ext-content"
    )

    // Foreground reload — must fire a second readFileForEdit, not be
    // deduplicated by a lingering in-flight marker from applyExternalReload.
    await act(async () => {
      screen.getByText("reload-a").click()
    })

    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(2)
  })
})

interface AtomicGuardSnapshot {
  tabs: Array<{
    id: string
    content: string
    isDirty: boolean
    stale: boolean
    saveState?: string
    saveError?: string | null
    etag: string | null | undefined
    gitBaseContent?: string | undefined
  }>
}

function ApplyEditRaceProbe({
  onCapture,
}: {
  onCapture: (snapshot: AtomicGuardSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    applyExternalReload,
    updateActiveFileContent,
    closeAllFileTabs,
  } = useWorkspaceContext()
  onCapture({
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
      saveState: tab.saveState,
      saveError: tab.saveError ?? null,
      etag: tab.etag,
      gitBaseContent: tab.gitBaseContent,
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button
        onClick={() => {
          // Single React event handler: both setFileTabs calls land in
          // the same batch. updater1 (from updateActiveFileContent)
          // marks the tab dirty; updater2 (from applyExternalReload)
          // would silently clobber that edit unless its updater performs
          // an atomic isDirty re-check.
          updateActiveFileContent("dirty-local")
          void applyExternalReload(1, "a.ts", {
            path: "a.ts",
            content: "ext-content",
            etag: "ext-etag",
            mtime_ms: 99,
            readonly: false,
            line_ending: "lf",
          })
        }}
      >
        edit-and-apply
      </button>
      <button
        onClick={() =>
          void applyExternalReload(1, "a.ts", {
            path: "a.ts",
            content: "ext-content",
            etag: "ext-etag",
            mtime_ms: 99,
            readonly: false,
            line_ending: "lf",
          })
        }
      >
        apply-a
      </button>
      <button onClick={closeAllFileTabs}>close-all</button>
    </div>
  )
}

function RejectEditRaceProbe({
  onCapture,
}: {
  onCapture: (snapshot: AtomicGuardSnapshot) => void
}) {
  const { openFilePreview, fileTabs, rejectFileTab, updateActiveFileContent } =
    useWorkspaceContext()
  onCapture({
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
      saveState: tab.saveState,
      saveError: tab.saveError ?? null,
      etag: tab.etag,
      gitBaseContent: tab.gitBaseContent,
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a</button>
      <button
        onClick={() => {
          // Same race shape as the apply probe above, but the second
          // queued updater is rejectFileTab — must also gate on
          // isDirty INSIDE the updater to avoid clobbering the edit.
          updateActiveFileContent("dirty-local")
          rejectFileTab(1, "a.ts", "boom")
        }}
      >
        edit-and-reject
      </button>
    </div>
  )
}

describe("atomic dirty guard in functional updaters", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  it("applyExternalReload refuses to overwrite a dirty edit enqueued in the same React batch", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: AtomicGuardSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyEditRaceProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })

    await act(async () => {
      screen.getByText("edit-and-apply").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
    // etag stays at the pre-apply value — proof that the apply was
    // refused as a whole, not partially.
    expect(tabA?.etag).toBe("e1")
  })

  it("rejectFileTab refuses to clobber a dirty edit enqueued in the same React batch", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: AtomicGuardSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <RejectEditRaceProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })

    await act(async () => {
      screen.getByText("edit-and-reject").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
    expect(tabA?.saveState).not.toBe("error")
    expect(tabA?.saveError).toBeNull()
  })

  it("applyExternalReload marks the refused tab stale so the conflict prompt fires without waiting for save", async () => {
    // Refusing the apply protects the dirty buffer from data loss, but
    // the user is then editing against disk that has silently diverged.
    // Marking stale=true wires the dirty refusal into the existing
    // aux-panel effect (tab.stale && tab.isDirty → announceConflict),
    // surfacing the divergence immediately instead of at save time.
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: AtomicGuardSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyEditRaceProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })

    await act(async () => {
      screen.getByText("edit-and-apply").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.stale).toBe(true)
  })

  it("rejectFileTab marks the refused dirty tab stale for symmetry with applyExternalReload", async () => {
    // rejectFileTab is normally called from the watcher only after
    // markTabsStale has already flagged the path, so stale=true here is
    // usually idempotent. Setting it inside the updater keeps the API
    // self-consistent: any direct caller that refuses the reject because
    // of a concurrent keystroke still surfaces divergence via the
    // existing stale+dirty conflict path, without relying on callers
    // remembering to call markTabsStale first.
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: AtomicGuardSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <RejectEditRaceProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })

    await act(async () => {
      screen.getByText("edit-and-reject").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.stale).toBe(true)
  })
})

describe("applyExternalReload git base does not stale-write after close+reopen", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
  })

  it("does not write a stale gitBaseContent into a reopened tab whose etag differs", async () => {
    // Initial open: not tracked → skip git base fetch entirely.
    mockedApi.gitIsTracked.mockResolvedValueOnce(false)
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "etag-orig",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })

    let snap: AtomicGuardSnapshot = { tabs: [] }
    render(
      <WorkspaceProvider>
        <ApplyEditRaceProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("open-a").click()
    })

    // Now the apply path: tracked, gitShowFile hangs (we'll resolve it
    // ourselves after close+reopen to simulate a slow git call).
    let resolveStaleGit: ((value: string) => void) | null = null
    mockedApi.gitIsTracked.mockResolvedValueOnce(true)
    mockedApi.gitShowFile.mockImplementationOnce(
      () => new Promise<string>((res) => (resolveStaleGit = res))
    )

    await act(async () => {
      screen.getByText("apply-a").click()
    })
    // Tab now has etag "ext-etag" (from apply-a's hardcoded fetched).
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.etag).toBe("ext-etag")

    // Close the tab; the stale git fetch is still pending.
    await act(async () => {
      screen.getByText("close-all").click()
    })
    expect(snap.tabs).toHaveLength(0)

    // Reopen the same path with a different etag and no git base
    // (so the only writes to gitBaseContent could come from the stale
    // fetch we have not yet resolved).
    mockedApi.gitIsTracked.mockResolvedValueOnce(false)
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v2",
      etag: "etag-different",
      mtime_ms: 2,
      readonly: false,
      line_ending: "lf",
    })

    await act(async () => {
      screen.getByText("open-a").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.etag).toBe(
      "etag-different"
    )
    // Sanity: reopen did not inherit a gitBaseContent (its own git path
    // was skipped via gitIsTracked → false).
    expect(snap.tabs.find((t) => t.id === "file:1:a.ts")?.gitBaseContent).toBe(
      undefined
    )

    // Resolve the dangling git fetch from the FIRST (closed) tab's apply.
    // It targets the same tabId but a different etag — must be rejected.
    await act(async () => {
      resolveStaleGit!("stale-base")
    })

    const tabA = snap.tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.gitBaseContent).not.toBe("stale-base")
    expect(tabA?.gitBaseContent).toBe(undefined)
  })
})

describe("WorkspaceProvider office auto-preview", () => {
  beforeEach(() => {
    workspaceStoreMock.reset()
    // Preference defaults ON; drop any "false" a prior test left behind.
    localStorage.removeItem("workspace:office-auto-preview")
  })

  it("auto-opens an office file's preview when the watcher reports it, with no aux panel involved", async () => {
    renderWorkspace()
    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
    expect(screen.getByTestId("active-pane")).toHaveTextContent("conversation")

    // The provider — not the (closed-by-default) file-tree aux panel — owns
    // this subscription, so a watcher envelope surfaces the preview directly.
    await act(async () => {
      workspaceStoreMock.emitEnvelope(["report.pptx"])
    })

    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("1")
    expect(screen.getByTestId("active-file-tab")).toHaveTextContent(
      "file:1:report.pptx"
    )
    expect(screen.getByTestId("active-pane")).toHaveTextContent("files")
    expect(screen.getByTestId("mode")).toHaveTextContent("fusion")
  })

  it("ignores non-office changed paths", async () => {
    renderWorkspace()

    await act(async () => {
      workspaceStoreMock.emitEnvelope(["notes.txt", "src/app.ts"])
    })

    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
    expect(screen.getByTestId("active-pane")).toHaveTextContent("conversation")
  })

  it("opens each office file once, even when later envelopes re-report it", async () => {
    renderWorkspace()

    await act(async () => {
      workspaceStoreMock.emitEnvelope(["deck.pptx"])
    })
    await act(async () => {
      workspaceStoreMock.emitEnvelope(["deck.pptx"])
    })

    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("1")
  })

  it("does not auto-open when the preference is disabled", async () => {
    localStorage.setItem("workspace:office-auto-preview", "false")
    renderWorkspace()

    await act(async () => {
      workspaceStoreMock.emitEnvelope(["report.pptx"])
    })

    expect(screen.getByTestId("file-tab-count")).toHaveTextContent("0")
    expect(screen.getByTestId("active-pane")).toHaveTextContent("conversation")
  })
})

describe("context slice render isolation", () => {
  // The whole point of the three-way context split: fileTabs churn
  // (keystrokes, watcher reloads) must not re-render components that only
  // subscribe to actions (conversation path) or view (layout chrome).
  // Render counting goes through an `onRender` callback (same pattern as
  // the `onCapture` probes above) — direct module-scope mutation inside a
  // component body trips react-hooks/immutability.
  const renderCounts = { actions: 0, view: 0, fileTabs: 0 }

  function ActionsProbe({ onRender }: { onRender: () => void }) {
    onRender()
    const { openSessionFileDiff, updateActiveFileContent } =
      useWorkspaceActions()
    return (
      <div>
        <button
          onClick={() => openSessionFileDiff("src/a.ts", "diff --git a", "T1")}
        >
          slice-open-a
        </button>
        <button
          onClick={() => openSessionFileDiff("src/b.ts", "diff --git b", "T1")}
        >
          slice-open-b
        </button>
        <button onClick={() => updateActiveFileContent("typed")}>
          slice-edit
        </button>
      </div>
    )
  }

  function ViewProbe({ onRender }: { onRender: () => void }) {
    onRender()
    const { mode } = useWorkspaceView()
    return <output data-testid="slice-mode">{mode}</output>
  }

  function FileTabsProbe({ onRender }: { onRender: () => void }) {
    onRender()
    const { fileTabs } = useWorkspaceFileTabs()
    return <output data-testid="slice-tab-count">{fileTabs.length}</output>
  }

  const countActions = () => {
    renderCounts.actions += 1
  }
  const countView = () => {
    renderCounts.view += 1
  }
  const countFileTabs = () => {
    renderCounts.fileTabs += 1
  }

  beforeEach(() => {
    renderCounts.actions = 0
    renderCounts.view = 0
    renderCounts.fileTabs = 0
  })

  it("keeps actions/view consumers un-rendered when fileTabs change within fusion mode", async () => {
    render(
      <WorkspaceProvider>
        <ActionsProbe onRender={countActions} />
        <ViewProbe onRender={countView} />
        <FileTabsProbe onRender={countFileTabs} />
      </WorkspaceProvider>
    )

    // First open flips mode conversation→fusion, so the view consumer is
    // expected to render once more here.
    await act(async () => {
      screen.getByText("slice-open-a").click()
    })
    expect(screen.getByTestId("slice-mode")).toHaveTextContent("fusion")
    expect(screen.getByTestId("slice-tab-count")).toHaveTextContent("1")

    const actionsBefore = renderCounts.actions
    const viewBefore = renderCounts.view
    const fileTabsBefore = renderCounts.fileTabs

    // Second open mutates fileTabs but neither mode (stays fusion) nor
    // activePane (stays files) — only the fileTabs consumer may render.
    await act(async () => {
      screen.getByText("slice-open-b").click()
    })
    expect(screen.getByTestId("slice-tab-count")).toHaveTextContent("2")
    expect(renderCounts.actions).toBe(actionsBefore)
    expect(renderCounts.view).toBe(viewBefore)
    expect(renderCounts.fileTabs).toBeGreaterThan(fileTabsBefore)
  })

  it("keeps actions/view consumers un-rendered on per-keystroke content updates", async () => {
    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })
    mockedApi.gitIsTracked.mockResolvedValue(false)

    function OpenFileProbe() {
      const { openFilePreview } = useWorkspaceActions()
      return (
        <button onClick={() => void openFilePreview("a.ts")}>
          slice-open-file
        </button>
      )
    }

    render(
      <WorkspaceProvider>
        <ActionsProbe onRender={countActions} />
        <ViewProbe onRender={countView} />
        <FileTabsProbe onRender={countFileTabs} />
        <OpenFileProbe />
      </WorkspaceProvider>
    )

    await act(async () => {
      screen.getByText("slice-open-file").click()
    })
    expect(screen.getByTestId("slice-tab-count")).toHaveTextContent("1")

    const actionsBefore = renderCounts.actions
    const viewBefore = renderCounts.view

    // Simulates the editor's onChange — the highest-frequency fileTabs write.
    await act(async () => {
      screen.getByText("slice-edit").click()
    })

    expect(renderCounts.actions).toBe(actionsBefore)
    expect(renderCounts.view).toBe(viewBefore)
  })
})

interface DecoupleSnapshot {
  activeId: string | null
  tabs: Array<{
    id: string
    folderId: number
    content: string
    isDirty: boolean
    stale: boolean
  }>
}

function DecoupleProbe({
  onCapture,
}: {
  onCapture: (snapshot: DecoupleSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    activeFileTabId,
    updateActiveFileContent,
    saveActiveFile,
    switchFileTab,
  } = useWorkspaceContext()
  onCapture({
    activeId: activeFileTabId,
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      folderId: tab.folderId,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a-f1</button>
      <button onClick={() => void openFilePreview("a.ts", { folderId: 2 })}>
        open-a-f2
      </button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => void saveActiveFile()}>save</button>
      <button onClick={() => switchFileTab("file:2:a.ts")}>switch-a-f2</button>
    </div>
  )
}

describe("file tabs decoupled from the active folder", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.saveFileContent.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
    // Distinguishable content per folder root so routing is observable.
    mockedApi.readFileForEdit.mockImplementation((root: string) =>
      Promise.resolve({
        path: "a.ts",
        content: root === "/repo2" ? "from-repo2" : "from-repo1",
        etag: root === "/repo2" ? "e2" : "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
    )
  })

  function renderDecouple() {
    let snap: DecoupleSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <DecoupleProbe onCapture={(s) => (snap = s)} />
      </WorkspaceProvider>
    )
    return () => snap
  }

  it("keeps open tabs (content and identity) across an active-folder switch", async () => {
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    expect(snap().tabs).toHaveLength(1)
    expect(snap().tabs[0]).toMatchObject({
      id: "file:1:a.ts",
      content: "from-repo1",
    })

    await act(async () => {
      foldersMock.setActiveFolderId(2)
    })

    expect(snap().tabs).toHaveLength(1)
    expect(snap().tabs[0]).toMatchObject({
      id: "file:1:a.ts",
      folderId: 1,
      content: "from-repo1",
    })
    expect(snap().activeId).toBe("file:1:a.ts")
  })

  it("opens the same relative path from two folders as two independent tabs", async () => {
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("open-a-f2").click()
    })

    expect(snap().tabs.map((t) => t.id)).toEqual(["file:1:a.ts", "file:2:a.ts"])
    expect(snap().tabs[0].content).toBe("from-repo1")
    expect(snap().tabs[1].content).toBe("from-repo2")
    expect(mockedApi.readFileForEdit).toHaveBeenCalledWith("/repo", "a.ts")
    expect(mockedApi.readFileForEdit).toHaveBeenCalledWith("/repo2", "a.ts")
  })

  it("saves a folder-2 tab through folder 2's root while folder 1 is active", async () => {
    mockedApi.saveFileContent.mockResolvedValue({
      path: "a.ts",
      etag: "e2-saved",
      mtime_ms: 2,
      readonly: false,
      line_ending: "lf",
    })
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    expect(snap().activeId).toBe("file:2:a.ts")

    await act(async () => {
      screen.getByText("edit").click()
    })
    expect(snap().tabs[0].isDirty).toBe(true)

    // Active WORKSPACE folder stays 1 — the save must still route to the
    // tab's own folder root.
    await act(async () => {
      screen.getByText("save").click()
    })

    expect(mockedApi.saveFileContent).toHaveBeenCalledTimes(1)
    expect(mockedApi.saveFileContent.mock.calls[0][0]).toBe("/repo2")
    expect(mockedApi.saveFileContent.mock.calls[0][1]).toBe("a.ts")
    expect(mockedApi.saveFileContent.mock.calls[0][2]).toBe("dirty-local")
  })

  it("closes only the removed folder's clean tabs and keeps the rest", async () => {
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    expect(snap().tabs).toHaveLength(2)

    await act(async () => {
      foldersMock.setAllFolders([
        { id: 1, path: "/repo", name: "repo", color: "inherit" },
      ])
    })

    expect(snap().tabs).toHaveLength(1)
    expect(snap().tabs[0].id).toBe("file:1:a.ts")
    // Active tab was the removed folder's — a survivor takes over.
    expect(snap().activeId).toBe("file:1:a.ts")
  })

  it("does not wipe tabs during a transient un-hydrated empty folder list", async () => {
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    expect(snap().tabs).toHaveLength(1)

    // Simulates cold start / refresh: the folder list empties while the
    // hydration flag drops. The settled guard must keep every tab.
    await act(async () => {
      foldersMock.setHydrated(false)
      foldersMock.setAllFolders([])
    })
    expect(snap().tabs).toHaveLength(1)

    await act(async () => {
      foldersMock.setAllFolders([
        { id: 1, path: "/repo", name: "repo", color: "inherit" },
        { id: 2, path: "/repo2", name: "repo2", color: "inherit" },
      ])
      foldersMock.setHydrated(true)
    })
    expect(snap().tabs).toHaveLength(1)
    expect(snap().tabs[0].id).toBe("file:1:a.ts")
  })

  it("keeps a dirty tab of a removed folder as a stale orphan instead of dropping it", async () => {
    const snap = renderDecouple()

    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    expect(snap().tabs[0]).toMatchObject({
      id: "file:2:a.ts",
      isDirty: true,
    })

    await act(async () => {
      foldersMock.setAllFolders([
        { id: 1, path: "/repo", name: "repo", color: "inherit" },
      ])
    })

    // Unsaved edits survive; the orphan is flagged stale so the user sees
    // the divergence, and saving it fails visibly (folder root is gone).
    expect(snap().tabs).toHaveLength(1)
    expect(snap().tabs[0]).toMatchObject({
      id: "file:2:a.ts",
      isDirty: true,
      stale: true,
      content: "dirty-local",
    })
  })
})

function ConflictProbe() {
  const { externalConflict, dismissExternalConflict } =
    useWorkspaceExternalConflict()
  return (
    <div>
      <output data-testid="conflict-head">
        {externalConflict
          ? `${externalConflict.folderId}:${externalConflict.path}`
          : "none"}
      </output>
      <button onClick={dismissExternalConflict}>dismiss-conflict</button>
    </div>
  )
}

function WatcherProbe({
  onCapture,
}: {
  onCapture: (snapshot: DecoupleSnapshot) => void
}) {
  const {
    openFilePreview,
    fileTabs,
    activeFileTabId,
    updateActiveFileContent,
    saveActiveFile,
    markTabsStale,
  } = useWorkspaceContext()
  onCapture({
    activeId: activeFileTabId,
    tabs: fileTabs.map((tab) => ({
      id: tab.id,
      folderId: tab.folderId,
      content: tab.content,
      isDirty: Boolean(tab.isDirty),
      stale: Boolean(tab.stale),
    })),
  })
  return (
    <div>
      <button onClick={() => void openFilePreview("a.ts")}>open-a-f1</button>
      <button onClick={() => void openFilePreview("a.ts", { folderId: 2 })}>
        open-a-f2
      </button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => void saveActiveFile()}>save</button>
      <button onClick={() => markTabsStale(1, "a.ts")}>stale-a-f1</button>
    </div>
  )
}

describe("provider tab watcher (per-folder streams, lazy staleness)", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.saveFileContent.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
    mockedApi.readFileForEdit.mockImplementation((root: string) =>
      Promise.resolve({
        path: "a.ts",
        content: root === "/repo2" ? "from-repo2" : "from-repo1",
        etag: root === "/repo2" ? "e2" : "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
    )
  })

  function renderWatcher() {
    let snap: DecoupleSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <WatcherProbe onCapture={(s) => (snap = s)} />
        <ConflictProbe />
      </WorkspaceProvider>
    )
    return () => snap
  }

  it("acquires one refcount per folder with open tabs and releases on close, without keystroke churn", async () => {
    renderWatcher()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    expect(workspaceStoreMock.acquiredCount("/repo")).toBe(1)
    const callsAfterOpen = workspaceStoreMock.acquireCalls("/repo")

    // Keystrokes mutate fileTabs every render — the watch signature is
    // unchanged, so the subscription must NOT be rebuilt (blocker #13).
    await act(async () => {
      screen.getByText("edit").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    expect(workspaceStoreMock.acquireCalls("/repo")).toBe(callsAfterOpen)

    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    expect(workspaceStoreMock.acquiredCount("/repo")).toBe(1)
    expect(workspaceStoreMock.acquiredCount("/repo2")).toBe(1)
  })

  it("marks a background folder's tab stale on its envelope without any disk read", async () => {
    const snap = renderWatcher()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    expect(snap().activeId).toBe("file:2:a.ts")
    const readsBefore = mockedApi.readFileForEdit.mock.calls.length

    await act(async () => {
      workspaceStoreMock.emitRoot("/repo", ["a.ts"])
    })

    const tabF1 = snap().tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabF1?.stale).toBe(true)
    expect(tabF1?.content).toBe("from-repo1")
    // The lazy pillar: zero reads for background tabs.
    expect(mockedApi.readFileForEdit.mock.calls.length).toBe(readsBefore)
  })

  it("eagerly reconciles the ACTIVE tab on its folder's envelope (clean → in-place reload)", async () => {
    const snap = renderWatcher()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    expect(snap().tabs[0]?.content).toBe("from-repo1")

    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "external-v2",
      etag: "e-ext",
      mtime_ms: 2,
      readonly: false,
      line_ending: "lf",
    })

    await act(async () => {
      workspaceStoreMock.emitRoot("/repo", ["a.ts"])
    })

    const tabA = snap().tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("external-v2")
    expect(tabA?.stale).toBe(false)
    expect(snap().activeId).toBe("file:1:a.ts")
  })

  it("queues a conflict for the ACTIVE dirty tab instead of clobbering the buffer", async () => {
    const snap = renderWatcher()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })

    mockedApi.readFileForEdit.mockResolvedValueOnce({
      path: "a.ts",
      content: "disk-v2",
      etag: "e-ext",
      mtime_ms: 2,
      readonly: false,
      line_ending: "lf",
    })

    await act(async () => {
      workspaceStoreMock.emitRoot("/repo", ["a.ts"])
    })

    expect(screen.getByTestId("conflict-head")).toHaveTextContent("1:a.ts")
    const tabA = snap().tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("dirty-local")
    expect(tabA?.isDirty).toBe(true)
  })

  it("treats a resync_hint as a full sweep for that folder only", async () => {
    const snap = renderWatcher()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("open-a-f2").click()
    })
    expect(snap().activeId).toBe("file:2:a.ts")

    await act(async () => {
      workspaceStoreMock.emitRoot("/repo", [], "resync_hint")
    })

    expect(snap().tabs.find((t) => t.id === "file:1:a.ts")?.stale).toBe(true)
    // The other folder's tab is untouched — sweeps are per-stream.
    expect(snap().tabs.find((t) => t.id === "file:2:a.ts")?.stale).toBe(false)
  })
})

describe("stale-aware save guard (all write paths funnel through saveFileTab)", () => {
  beforeEach(() => {
    mockedApi.readFileForEdit.mockReset()
    mockedApi.gitIsTracked.mockReset()
    mockedApi.gitShowFile.mockReset()
    mockedApi.saveFileContent.mockReset()
    mockedApi.gitIsTracked.mockResolvedValue(false)
  })

  function renderGuard() {
    let snap: DecoupleSnapshot = { activeId: null, tabs: [] }
    render(
      <WorkspaceProvider>
        <WatcherProbe onCapture={(s) => (snap = s)} />
        <ConflictProbe />
      </WorkspaceProvider>
    )
    return () => snap
  }

  it("refuses to save a stale dirty tab whose disk diverged, surfacing the conflict", async () => {
    // Open (etag e1) — later reads keep reporting a DIVERGED disk (e-div).
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v1",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValue({
        path: "a.ts",
        content: "disk-v2",
        etag: "e-div",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })
    const snap = renderGuard()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    await act(async () => {
      screen.getByText("stale-a-f1").click()
    })

    await act(async () => {
      screen.getByText("save").click()
    })

    // Blind write refused: no saveFileContent, buffer intact, conflict up.
    expect(mockedApi.saveFileContent).not.toHaveBeenCalled()
    const tabA = snap().tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.content).toBe("dirty-local")
    expect(tabA?.isDirty).toBe(true)
    expect(screen.getByTestId("conflict-head")).toHaveTextContent("1:a.ts")
  })

  it("proceeds with the save when the stale flag proves spurious (etag equal)", async () => {
    // Disk never changed: every read reports etag e1.
    mockedApi.readFileForEdit.mockResolvedValue({
      path: "a.ts",
      content: "v1",
      etag: "e1",
      mtime_ms: 1,
      readonly: false,
      line_ending: "lf",
    })
    mockedApi.saveFileContent.mockResolvedValue({
      path: "a.ts",
      etag: "e-saved",
      mtime_ms: 3,
      readonly: false,
      line_ending: "lf",
    })
    const snap = renderGuard()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    await act(async () => {
      screen.getByText("stale-a-f1").click()
    })

    await act(async () => {
      screen.getByText("save").click()
    })

    expect(mockedApi.saveFileContent).toHaveBeenCalledTimes(1)
    const tabA = snap().tabs.find((t) => t.id === "file:1:a.ts")
    expect(tabA?.isDirty).toBe(false)
    expect(tabA?.stale).toBe(false)
    expect(screen.getByTestId("conflict-head")).toHaveTextContent("none")
  })

  it("re-surfaces the conflict when the user saves again after dismissing the dialog", async () => {
    mockedApi.readFileForEdit
      .mockResolvedValueOnce({
        path: "a.ts",
        content: "v1",
        etag: "e1",
        mtime_ms: 1,
        readonly: false,
        line_ending: "lf",
      })
      .mockResolvedValue({
        path: "a.ts",
        content: "disk-v2",
        etag: "e-div",
        mtime_ms: 2,
        readonly: false,
        line_ending: "lf",
      })
    renderGuard()

    await act(async () => {
      screen.getByText("open-a-f1").click()
    })
    await act(async () => {
      screen.getByText("edit").click()
    })
    await act(async () => {
      screen.getByText("stale-a-f1").click()
    })
    await act(async () => {
      screen.getByText("save").click()
    })
    expect(screen.getByTestId("conflict-head")).toHaveTextContent("1:a.ts")

    // User closes the dialog without resolving.
    await act(async () => {
      screen.getByText("dismiss-conflict").click()
    })
    expect(screen.getByTestId("conflict-head")).toHaveTextContent("none")

    // Saving again is still refused (disk unchanged, still diverged) —
    // and the dialog MUST come back: a silent no-op would strand the tab.
    await act(async () => {
      screen.getByText("save").click()
    })
    expect(mockedApi.saveFileContent).not.toHaveBeenCalled()
    expect(screen.getByTestId("conflict-head")).toHaveTextContent("1:a.ts")
  })
})
