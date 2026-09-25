import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { GitLogTab } from "./aux-panel-git-log-tab"
import enMessages from "@/i18n/messages/en.json"
import type { GitBranchList } from "@/lib/types"

type PendingBranchList = {
  path: string
  resolve: (list: GitBranchList) => void
  reject: (error: Error) => void
}

const state = vi.hoisted(() => ({
  folder: { id: 1, path: "/worktrees/a" },
  gitLog: vi.fn(async () => ({ entries: [] })),
  deferBranches: false,
  pendingBranches: [] as PendingBranchList[],
  listeners: new Map<string, (payload: { folder_id: number }) => void>(),
}))

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  gitLog: state.gitLog,
  getGitBranch: async (path: string) =>
    path === "/worktrees/a" ? "mainA" : "mainB",
  gitListAllBranches: (path: string) =>
    state.deferBranches
      ? new Promise<GitBranchList>((resolve, reject) => {
          state.pendingBranches.push({ path, resolve, reject })
        })
      : Promise.resolve({
          local: [path === "/worktrees/a" ? "mainA" : "mainB"],
          remote: [],
          worktree_branches: [],
          main_worktree_branch: null,
        }),
  gitCurrentUser: async () => null,
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: state.folder }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({
    openCommitDiff: vi.fn(),
    openFilePreview: vi.fn(),
  }),
}))

vi.mock("@/hooks/use-workspace-state-store", () => ({
  useWorkspaceStateStore: () => ({ isGitRepo: true }),
}))

vi.mock("@/hooks/use-git-quick-actions", () => ({
  useGitQuickActions: () => ({
    running: false,
    pull: vi.fn(),
    fetchAll: vi.fn(),
    openPushWindow: vi.fn(),
    dialogs: null,
  }),
}))

vi.mock("@/stores/app-workspace-store", () => ({
  useAppWorkspaceStore: (
    selector: (state: { gitHeads: Map<number, unknown> }) => unknown
  ) => selector({ gitHeads: new Map() }),
}))

vi.mock("@/lib/platform", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/platform")>()),
  subscribe: async (
    eventName: string,
    listener: (payload: { folder_id: number }) => void
  ) => {
    state.listeners.set(eventName, listener)
    return () => {
      if (state.listeners.get(eventName) === listener) {
        state.listeners.delete(eventName)
      }
    }
  },
}))

vi.mock("@/components/layout/remote-manage-dialog", () => ({
  RemoteManageDialog: () => null,
}))

function tabTree() {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <GitLogTab />
    </NextIntlClientProvider>
  )
}

function renderTab() {
  return render(tabTree())
}

describe("Commits tab branch query", () => {
  beforeEach(() => {
    window.localStorage.clear()
    state.folder = { id: 1, path: "/worktrees/a" }
    state.gitLog.mockClear()
    state.deferBranches = false
    state.pendingBranches = []
    state.listeners.clear()
  })
  afterEach(() => cleanup())

  it("queries only the current worktree HEAD on first open", async () => {
    renderTab()
    await waitFor(() => expect(state.gitLog).toHaveBeenCalled())
    expect(state.gitLog.mock.calls[0]).toEqual([
      "/worktrees/a",
      100,
      "HEAD",
      undefined,
      0,
      undefined,
      false,
      false,
    ])
  })

  it("ignores an older branch refresh that omits the saved branch", async () => {
    const key = "codeg:gitlog:selection:/worktrees/a"
    window.localStorage.setItem(
      key,
      JSON.stringify({ branch: "feature/new", author: null })
    )
    state.deferBranches = true
    renderTab()
    await waitFor(() => expect(state.pendingBranches).toHaveLength(1))
    await waitFor(() =>
      expect(state.listeners.has("folder://git-branch-changed")).toBe(true)
    )

    act(() => {
      state.listeners.get("folder://git-branch-changed")?.({ folder_id: 1 })
    })
    await waitFor(() => expect(state.pendingBranches).toHaveLength(2))
    await act(async () => {
      state.pendingBranches[1].resolve({
        local: ["mainA", "feature/new"],
        remote: [],
        worktree_branches: [],
        main_worktree_branch: null,
      })
    })
    await act(async () => {
      state.pendingBranches[0].resolve({
        local: ["mainA"],
        remote: [],
        worktree_branches: [],
        main_worktree_branch: null,
      })
    })

    expect(JSON.parse(window.localStorage.getItem(key) ?? "{}").branch).toBe(
      "feature/new"
    )
  })

  it("discards an older branch response when the latest refresh fails", async () => {
    const key = "codeg:gitlog:selection:/worktrees/a"
    window.localStorage.setItem(
      key,
      JSON.stringify({ branch: "feature/new", author: null })
    )
    state.deferBranches = true
    renderTab()
    await waitFor(() => expect(state.pendingBranches).toHaveLength(1))
    await waitFor(() =>
      expect(state.listeners.has("folder://git-branch-changed")).toBe(true)
    )
    act(() => {
      state.listeners.get("folder://git-branch-changed")?.({ folder_id: 1 })
    })
    await waitFor(() => expect(state.pendingBranches).toHaveLength(2))

    await act(async () => {
      state.pendingBranches[1].reject(new Error("branch lookup failed"))
    })
    await act(async () => {
      state.pendingBranches[0].resolve({
        local: ["mainA"],
        remote: [],
        worktree_branches: [],
        main_worktree_branch: null,
      })
    })
    expect(JSON.parse(window.localStorage.getItem(key) ?? "{}").branch).toBe(
      "feature/new"
    )
  })

  it("clears old branch metadata while a new worktree loads", async () => {
    const view = renderTab()
    await screen.findByText("mainA")
    state.deferBranches = true
    await waitFor(() =>
      expect(state.listeners.has("folder://git-branch-changed")).toBe(true)
    )
    act(() => {
      state.listeners.get("folder://git-branch-changed")?.({ folder_id: 1 })
    })
    await waitFor(() => expect(state.pendingBranches).toHaveLength(1))

    state.folder = { id: 2, path: "/worktrees/b" }
    view.rerender(tabTree())
    await waitFor(() => expect(state.pendingBranches).toHaveLength(2))
    expect(screen.queryByText("mainA")).toBeNull()

    await act(async () => {
      state.pendingBranches[1].resolve({
        local: ["mainB"],
        remote: [],
        worktree_branches: [],
        main_worktree_branch: null,
      })
    })
    await screen.findByText("mainB")
    await act(async () => {
      state.pendingBranches[0].resolve({
        local: ["mainA"],
        remote: [],
        worktree_branches: [],
        main_worktree_branch: null,
      })
    })
    expect(screen.queryByText("mainA")).toBeNull()
    expect(screen.getByText("mainB")).toBeInTheDocument()
  })

  it("keeps a deliberate All branches selection after remount", async () => {
    const first = renderTab()
    await screen.findByRole("button", { name: "Clear branch filter" })
    fireEvent.click(screen.getByRole("button", { name: "Clear branch filter" }))
    await waitFor(() =>
      expect(state.gitLog).toHaveBeenCalledWith(
        "/worktrees/a",
        100,
        undefined,
        undefined,
        0,
        undefined,
        true,
        false
      )
    )

    first.unmount()
    state.gitLog.mockClear()
    renderTab()
    await waitFor(() =>
      expect(state.gitLog).toHaveBeenCalledWith(
        "/worktrees/a",
        100,
        undefined,
        undefined,
        0,
        undefined,
        true,
        false
      )
    )
  })
})
