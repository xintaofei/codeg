import { render, screen, cleanup } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { ConversationFolderBranchPicker } from "./conversation-context-bar"
import type { FolderDetail } from "@/lib/types"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"

// ---------------------------------------------------------------------------
// Mocks. The picker reads three contexts/hooks and one api call; everything
// else (cmdk tree building, branch-tree expansion) is pure and runs for real.
// ---------------------------------------------------------------------------

const switchToBranch = vi.fn().mockResolvedValue(undefined)
const gitCheckout = vi.fn().mockResolvedValue(undefined)
const gitListAllBranches = vi.fn()
const openNewConversationTab = vi.fn()

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

vi.mock("@/hooks/use-switch-to-branch", () => ({
  useSwitchToBranch: () => switchToBranch,
}))

vi.mock("@/lib/api", () => ({
  gitListAllBranches: (path: string) => gitListAllBranches(path),
  // Present only so the "never bare-checkout" assertion has a spy to read; the
  // component must not reach for it any more.
  gitCheckout: (path: string, branch: string) => gitCheckout(path, branch),
}))

// Tab state, mutated per test before render. Workspace state (folders /
// branches) is seeded into the real zustand store in beforeEach.
let tabs: Array<{
  id: string
  folderId: number
  conversationId: number | null
  isChat?: boolean
}> = []
let activeTabId: string | null = null

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (
    selector: (s: {
      tabs: typeof tabs
      activeTabId: typeof activeTabId
    }) => unknown
  ) => selector({ tabs, activeTabId }),
  useTabActions: () => ({
    openNewConversationTab,
    openChatModeTab: vi.fn(),
  }),
}))

function mkFolder(p: Partial<FolderDetail> & { id: number }): FolderDetail {
  return {
    name: `folder-${p.id}`,
    path: `/repo/folder-${p.id}`,
    git_branch: null,
    default_agent_type: null,
    last_opened_at: "2026-01-01T00:00:00Z",
    sort_order: p.id,
    color: "blue",
    parent_id: null,
    kind: "regular",
    alias: null,
    ...p,
  }
}

const repo = mkFolder({
  id: 1,
  name: "repo",
  path: "/repo",
  git_branch: "main",
})

beforeEach(() => {
  switchToBranch.mockClear()
  gitCheckout.mockClear()
  openNewConversationTab.mockClear()
  gitListAllBranches.mockReset()
  gitListAllBranches.mockResolvedValue({
    local: ["main", "feat-x"],
    remote: [],
    worktree_branches: ["feat-x"],
  })
  resetAppWorkspaceStore()
  useAppWorkspaceStore.setState({
    folders: [repo],
    allFolders: [repo],
    branches: new Map([[1, "main"]]),
  })
})

afterEach(() => cleanup())

async function openBranchPickerAndSelect(branchName: string) {
  const user = userEvent.setup()
  // Two triggers render (folder + branch); the branch one is labelled by the
  // current branch.
  await user.click(screen.getByRole("button", { name: /main/ }))
  const item = await screen.findByText(branchName)
  await user.click(item)
}

describe("ConversationFolderBranchPicker — branch checkout", () => {
  it("routes an EXISTING conversation through switchToBranch (not a bare checkout)", async () => {
    tabs = [{ id: "tab-1", folderId: 1, conversationId: 42 }]
    activeTabId = "tab-1"

    render(<ConversationFolderBranchPicker tabId="tab-1" />)
    await openBranchPickerAndSelect("feat-x")

    expect(switchToBranch).toHaveBeenCalledTimes(1)
    expect(switchToBranch).toHaveBeenCalledWith({
      activeFolder: repo,
      branchName: "feat-x",
      currentBranch: "main",
      isRemote: false,
    })
    // The whole point of the fix: existing conversations must never run a bare
    // in-place `git checkout` (which fails for a worktree branch or hijacks a
    // worktree onto a free one).
    expect(gitCheckout).not.toHaveBeenCalled()
  })

  it("routes a DRAFT conversation through switchToBranch too (unchanged)", async () => {
    tabs = [{ id: "tab-draft", folderId: 1, conversationId: null }]
    activeTabId = "tab-draft"

    render(<ConversationFolderBranchPicker tabId="tab-draft" />)
    await openBranchPickerAndSelect("feat-x")

    expect(switchToBranch).toHaveBeenCalledWith({
      activeFolder: repo,
      branchName: "feat-x",
      currentBranch: "main",
      isRemote: false,
    })
    expect(gitCheckout).not.toHaveBeenCalled()
  })

  it("does not fire a checkout when the picked branch is the current one", async () => {
    tabs = [{ id: "tab-1", folderId: 1, conversationId: 42 }]
    activeTabId = "tab-1"

    render(<ConversationFolderBranchPicker tabId="tab-1" />)
    const user = userEvent.setup()
    await user.click(screen.getByRole("button", { name: /main/ }))
    // Click the already-current branch inside the open popover.
    const items = await screen.findAllByText("main")
    await user.click(items[items.length - 1])

    expect(switchToBranch).not.toHaveBeenCalled()
    expect(gitCheckout).not.toHaveBeenCalled()
  })
})
