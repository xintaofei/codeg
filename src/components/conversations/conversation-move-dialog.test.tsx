import { type ReactElement } from "react"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"
import type { DbConversationSummary, FolderDetail } from "@/lib/types"

const h = vi.hoisted(() => ({
  moveConversation: vi.fn(),
  openFolder: vi.fn(),
  moveConversationTab: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  moveConversation: h.moveConversation,
  openFolder: h.openFolder,
  listAllConversations: vi.fn(async () => []),
  listAllFolderDetails: vi.fn(async () => []),
  listOpenFolderDetails: vi.fn(async () => []),
  openFolderById: vi.fn(),
  openWorktreeFolder: vi.fn(),
  removeFolderFromWorkspace: vi.fn(),
  reorderFolders: vi.fn(),
  getFolder: vi.fn(),
}))

vi.mock("@/stores/tab-store", () => {
  const state = { moveConversationTab: h.moveConversationTab }
  const useStore = (selector: (value: typeof state) => unknown) =>
    selector(state)
  useStore.getState = () => state
  return { useTabStore: useStore }
})

vi.mock("sonner", () => ({
  toast: {
    success: h.toastSuccess,
    error: h.toastError,
  },
}))

// Keep this suite on the move workflow. The shared controls have their own
// interaction tests; these light adapters expose their inputs deterministically.
vi.mock("@/components/shared/folder-select", () => ({
  FolderSelect: ({
    folders,
    value,
    onChange,
  }: {
    folders: FolderDetail[]
    value: number | null
    onChange: (id: number) => void
  }) => (
    <div>
      <output data-testid="selected-folder">{value ?? "none"}</output>
      {folders.map((folder) => (
        <button
          key={folder.id}
          type="button"
          onClick={() => onChange(folder.id)}
        >
          {folder.name}
        </button>
      ))}
    </div>
  ),
}))

vi.mock("@/components/shared/directory-path-input", () => ({
  DirectoryPathInput: ({
    value,
    onValueChange,
    disabled,
  }: {
    value: string
    onValueChange: (value: string) => void
    disabled?: boolean
  }) => (
    <input
      aria-label="Directory path"
      value={value}
      disabled={disabled}
      onChange={(event) => onValueChange(event.target.value)}
    />
  ),
}))

import {
  ConversationMoveDialog,
  type ConversationMoveTarget,
} from "./conversation-move-dialog"

const folder = (
  id: number,
  name: string,
  path: string,
  kind: FolderDetail["kind"] = "regular"
): FolderDetail => ({
  id,
  name,
  path,
  git_branch: null,
  default_agent_type: null,
  last_opened_at: "2026-08-28T00:00:00Z",
  sort_order: id,
  color: "inherit",
  parent_id: null,
  kind,
  alias: null,
})

const SOURCE = folder(1, "source", "/repo/source")
const TARGET = folder(2, "target", "/repo/target")
const CHAT = folder(3, "chat", "/data/chat/3", "chat")

const moveTarget: ConversationMoveTarget = {
  conversationId: 7,
  folderId: SOURCE.id,
  folderPath: SOURCE.path,
  title: "Migration test",
}

function summary(folderId: number): DbConversationSummary {
  return {
    id: 7,
    folder_id: folderId,
    title: "Migration test",
    title_locked: false,
    agent_type: "codex",
    status: "in_progress",
    kind: "regular",
    model: null,
    git_branch: null,
    external_id: "session-7",
    message_count: 3,
    child_count: 0,
    created_at: "2026-08-28T00:00:00Z",
    updated_at: "2026-08-28T00:00:00Z",
    pinned_at: null,
    parent_id: null,
    parent_tool_use_id: null,
    delegation_call_id: null,
    origin_cwd: SOURCE.path,
  }
}

function withIntl(ui: ReactElement) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  resetAppWorkspaceStore()
  useAppWorkspaceStore.setState({
    folders: [SOURCE, TARGET],
    allFolders: [SOURCE, TARGET, CHAT],
    foldersHydrated: true,
  })
  h.moveConversation.mockResolvedValue(summary(TARGET.id))
  h.openFolder.mockResolvedValue(TARGET)
})

describe("ConversationMoveDialog", () => {
  it("offers only eligible destinations and applies the authoritative move", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const onClose = vi.fn()
    render(
      withIntl(<ConversationMoveDialog target={moveTarget} onClose={onClose} />)
    )

    expect(screen.getByRole("button", { name: "target" })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "source" })).toBeNull()
    expect(screen.queryByRole("button", { name: "chat" })).toBeNull()

    await user.click(screen.getByRole("button", { name: "target" }))
    await user.click(screen.getByRole("button", { name: "Move to folder" }))

    await waitFor(() => {
      expect(h.moveConversation).toHaveBeenCalledWith(7, 2)
    })
    expect(h.moveConversationTab).toHaveBeenCalledWith(7, 2, "/repo/target")
    expect(
      useAppWorkspaceStore
        .getState()
        .conversations.find((item) => item.id === 7)?.folder_id
    ).toBe(2)
    expect(h.toastSuccess).toHaveBeenCalledWith("Conversation moved")
    expect(onClose).toHaveBeenCalledOnce()
  })

  it("registers an arbitrary directory and selects it as the destination", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const opened = folder(4, "new-target", "/srv/new-target")
    h.openFolder.mockResolvedValue(opened)
    h.moveConversation.mockResolvedValue(summary(opened.id))
    render(
      withIntl(<ConversationMoveDialog target={moveTarget} onClose={vi.fn()} />)
    )

    fireEvent.change(screen.getByLabelText("Directory path"), {
      target: { value: opened.path },
    })
    await user.click(screen.getByRole("button", { name: "Use folder" }))

    await waitFor(() => {
      expect(h.openFolder).toHaveBeenCalledWith(opened.path)
      expect(screen.getByTestId("selected-folder")).toHaveTextContent("4")
    })
    await user.click(screen.getByRole("button", { name: "Move to folder" }))
    await waitFor(() => {
      expect(h.moveConversation).toHaveBeenCalledWith(7, 4)
    })
    expect(h.moveConversationTab).toHaveBeenCalledWith(7, 4, opened.path)
  })

  it("refuses an arbitrary path that resolves to the source folder", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    h.openFolder.mockResolvedValue(SOURCE)
    render(
      withIntl(<ConversationMoveDialog target={moveTarget} onClose={vi.fn()} />)
    )

    fireEvent.change(screen.getByLabelText("Directory path"), {
      target: { value: SOURCE.path },
    })
    await user.click(screen.getByRole("button", { name: "Use folder" }))

    await waitFor(() => {
      expect(h.toastError).toHaveBeenCalledWith(
        "Choose a different folder from the current one"
      )
    })
    expect(screen.getByTestId("selected-folder")).toHaveTextContent("none")
    expect(h.moveConversation).not.toHaveBeenCalled()
  })

  it("keeps the dialog open and explains an in-flight-turn rejection", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    const onClose = vi.fn()
    h.moveConversation.mockRejectedValue({
      code: "turn_in_progress",
      message: "busy",
    })
    render(
      withIntl(<ConversationMoveDialog target={moveTarget} onClose={onClose} />)
    )

    await user.click(screen.getByRole("button", { name: "target" }))
    await user.click(screen.getByRole("button", { name: "Move to folder" }))

    await waitFor(() => {
      expect(h.toastError).toHaveBeenCalledWith("Could not move conversation", {
        description:
          "Wait for the current response to finish, then try moving again.",
      })
    })
    expect(onClose).not.toHaveBeenCalled()
    expect(
      screen.getByRole("heading", { name: "Move conversation" })
    ).toBeInTheDocument()
  })

  it("surfaces an arbitrary-directory registration failure without selecting it", async () => {
    const user = userEvent.setup({ pointerEventsCheck: 0 })
    h.openFolder.mockRejectedValue(new Error("permission denied"))
    render(
      withIntl(<ConversationMoveDialog target={moveTarget} onClose={vi.fn()} />)
    )

    fireEvent.change(screen.getByLabelText("Directory path"), {
      target: { value: "/forbidden" },
    })
    await user.click(screen.getByRole("button", { name: "Use folder" }))

    await waitFor(() => {
      expect(h.toastError).toHaveBeenCalledWith("Could not open folder", {
        description: "permission denied",
      })
    })
    expect(screen.getByTestId("selected-folder")).toHaveTextContent("none")
  })
})
