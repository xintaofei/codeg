import { act, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import {
  WorkspaceProvider,
  useWorkspaceContext,
} from "@/contexts/workspace-context"
import * as api from "@/lib/api"

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string, values?: Record<string, string>) =>
    values ? `${key}:${JSON.stringify(values)}` : key,
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: { id: 1, path: "/repo", name: "repo" },
    activeFolderId: 1,
  }),
}))

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
}))

const mockedApi = api as unknown as {
  readFileForEdit: ReturnType<typeof vi.fn>
  gitIsTracked: ReturnType<typeof vi.fn>
  gitShowFile: ReturnType<typeof vi.fn>
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
      <button onClick={() => void reloadOpenFileBackground("a.ts")}>
        bg-reload-a
      </button>
      <button onClick={() => markTabsStale("a.ts")}>stale-a</button>
      <button onClick={() => updateActiveFileContent("dirty-local")}>
        edit
      </button>
      <button onClick={() => switchFileTab("file:a.ts")}>switch-a</button>
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
    expect(snap.activeId).toBe("file:b.ts")

    await act(async () => {
      screen.getByText("bg-reload-a").click()
    })

    // active tab stays on B; tab A content refreshed in place.
    expect(snap.activeId).toBe("file:b.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
    expect(tabA?.content).toBe("a-v2")
    expect(mockedApi.readFileForEdit).toHaveBeenCalledTimes(3)
  })

  it("markTabsStale flips stale=true on the matching tab", async () => {
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

    await act(async () => {
      screen.getByText("open-a").click()
    })
    expect(snap.tabs[0]?.stale).toBe(false)

    await act(async () => {
      screen.getByText("stale-a").click()
    })
    expect(snap.tabs[0]?.stale).toBe(true)
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
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.stale).toBe(true)

    // Plain activation (no reload option) must still refetch because stale.
    await act(async () => {
      screen.getByText("open-a").click()
    })

    expect(snap.activeId).toBe("file:a.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    // Activate the dirty stale tab. Must NOT refetch (would clobber edits).
    await act(async () => {
      screen.getByText("switch-a").click()
    })

    expect(snap.activeId).toBe("file:a.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
    expect(tabA?.stale).toBe(true)
    expect(mockedApi.readFileForEdit.mock.calls.length).toBe(callsBefore)
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
      <button onClick={() => markTabsStale("a.ts")}>stale-a</button>
      <button
        onClick={() =>
          void applyExternalReload("a.ts", {
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
          void applyExternalReload("missing.ts", {
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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
    expect(snap.activeId).toBe("file:b.ts")

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    expect(snap.activeId).toBe("file:b.ts")
    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
    expect(tabA?.isDirty).toBe(true)
    expect(tabA?.content).toBe("dirty-local")
  })

  it("clears stale=true on a successful apply", async () => {
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
      screen.getByText("stale-a").click()
    })
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.stale).toBe(true)

    await act(async () => {
      screen.getByText("apply-a").click()
    })

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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
      <button onClick={() => rejectFileTab("a.ts", "ENOENT: file removed")}>
        reject-a
      </button>
      <button onClick={() => rejectFileTab("missing.ts", "irrelevant")}>
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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
          void applyExternalReload("a.ts", {
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
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.content).toBe(
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
          void applyExternalReload("a.ts", {
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
          void applyExternalReload("a.ts", {
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
          rejectFileTab("a.ts", "boom")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
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
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.etag).toBe("ext-etag")

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
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.etag).toBe(
      "etag-different"
    )
    // Sanity: reopen did not inherit a gitBaseContent (its own git path
    // was skipped via gitIsTracked → false).
    expect(snap.tabs.find((t) => t.id === "file:a.ts")?.gitBaseContent).toBe(
      undefined
    )

    // Resolve the dangling git fetch from the FIRST (closed) tab's apply.
    // It targets the same tabId but a different etag — must be rejected.
    await act(async () => {
      resolveStaleGit!("stale-base")
    })

    const tabA = snap.tabs.find((t) => t.id === "file:a.ts")
    expect(tabA?.gitBaseContent).not.toBe("stale-base")
    expect(tabA?.gitBaseContent).toBe(undefined)
  })
})
