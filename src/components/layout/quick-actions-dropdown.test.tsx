import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { FolderDetail } from "@/lib/types"

// The menu is a thin router over existing entry points, so every one of them is
// stubbed: the test's job is to prove each row reaches the right door (and that
// the desktop-only rows disappear off the desktop), not to re-test the doors.
const mocks = vi.hoisted(() => {
  const connections = [
    { id: 11, name: "prod-box", base_url: "https://prod.example" },
    { id: 12, name: "lab-box", base_url: "https://lab.example" },
  ]
  return {
    connections,
    openImportSessionsWindow: vi.fn(),
    openProjectBootWindow: vi.fn(() => Promise.resolve()),
    openPetWindow: vi.fn(() => Promise.resolve()),
    openRemoteWorkspace: vi.fn(() => Promise.resolve()),
    listRemoteWorkspaceConnections: vi.fn(() => Promise.resolve(connections)),
    setSearchOpen: vi.fn(),
    setRoute: vi.fn(),
  }
})

let desktop = true
vi.mock("@/lib/platform", () => ({ isDesktop: () => desktop }))

vi.mock("@/lib/api", () => ({
  openImportSessionsWindow: mocks.openImportSessionsWindow,
  openProjectBootWindow: mocks.openProjectBootWindow,
}))

vi.mock("@/lib/pet/api", () => ({ openPetWindow: mocks.openPetWindow }))

vi.mock("@/lib/remote-workspace", () => ({
  listRemoteWorkspaceConnections: mocks.listRemoteWorkspaceConnections,
  openRemoteWorkspace: mocks.openRemoteWorkspace,
}))

let activeFolder: FolderDetail | null = null
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder,
    activeFolderId: activeFolder?.id ?? null,
  }),
}))

vi.mock("@/contexts/search-dialog-context", () => ({
  useSearchDialog: () => ({ open: false, setOpen: mocks.setSearchOpen }),
}))

vi.mock("@/contexts/automations-view-context", () => ({
  useAutomationsView: () => ({ unseenFailures: 2 }),
}))

vi.mock("@/contexts/tasks-view-context", () => ({
  useTasksView: () => ({ attentionCount: 0 }),
}))

vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({
    routeId: "conversations",
    setRoute: mocks.setRoute,
  }),
}))

// Dialogs render nothing until opened and drag in large trees; the menu only
// owns their open state, which the assertions below read off these stubs.
vi.mock("./clone-dialog", () => ({
  CloneDialog: ({ open }: { open: boolean }) =>
    open ? <div>CLONE-DIALOG</div> : null,
}))
vi.mock("./workspace-folder-dialog", () => ({
  WorkspaceFolderDialog: ({ open }: { open: boolean }) =>
    open ? <div>FOLDER-DIALOG</div> : null,
}))
vi.mock("./remote-workspace-manage-dialog", () => ({
  RemoteWorkspaceManageDialog: ({ open }: { open: boolean }) =>
    open ? <div>REMOTE-MANAGE-DIALOG</div> : null,
}))
vi.mock("@/components/conversations/conversation-manage-dialog", () => ({
  ConversationManageDialog: ({ folderId }: { folderId: number }) => (
    <div>MANAGE-DIALOG-{folderId}</div>
  ),
}))

import { QuickActionsDropdown } from "./quick-actions-dropdown"
import enMessages from "@/i18n/messages/en.json"

const FOLDER = { id: 7, name: "repo", path: "/tmp/repo" } as FolderDetail

/** Mount once, then open the (single) trigger. */
async function mountAndOpen() {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <QuickActionsDropdown />
    </NextIntlClientProvider>
  )
  await reopen()
}

/** Re-open the already-mounted menu — each item click closes it. */
async function reopen() {
  await userEvent.click(screen.getByRole("button", { name: "Quick actions" }))
}

async function clickItem(name: string | RegExp) {
  await userEvent.click(await screen.findByRole("menuitem", { name }))
}

// "Automations" carries a failure-count badge inside the row (2, per the mock),
// so its accessible name is the label plus that number — matched by prefix.
const AUTOMATIONS_ROW = /^Automations/

beforeEach(() => {
  desktop = true
  activeFolder = null
  vi.clearAllMocks()
})

describe("QuickActionsDropdown", () => {
  it("groups all ten actions under their headings on desktop", async () => {
    await mountAndOpen()

    for (const group of [
      "Workspace",
      "Sessions",
      "Automation & to-dos",
      "More",
    ]) {
      expect(await screen.findByText(group)).toBeVisible()
    }
    for (const label of [
      "Open Folder",
      "Clone Repository",
      "Project Boot",
      "Open remote workspace",
      "Manage conversations",
      "Import local sessions",
      "Search",
      AUTOMATIONS_ROW,
      "To-dos",
      "Show pet",
    ]) {
      expect(await screen.findByRole("menuitem", { name: label })).toBeVisible()
    }
  })

  it("drops the desktop-only rows in web mode", async () => {
    desktop = false
    await mountAndOpen()

    // The remaining eight still render, so this is a targeted removal rather
    // than the menu failing to open.
    expect(
      await screen.findByRole("menuitem", { name: "Search" })
    ).toBeVisible()
    expect(
      screen.queryByRole("menuitem", { name: "Open remote workspace" })
    ).toBeNull()
    expect(screen.queryByRole("menuitem", { name: "Show pet" })).toBeNull()
    expect(screen.queryByText("More")).toBeNull()
  })

  it("routes each action to its own entry point", async () => {
    activeFolder = FOLDER
    await mountAndOpen()

    await clickItem("Search")
    expect(mocks.setSearchOpen).toHaveBeenCalledWith(true)

    await reopen()
    await clickItem(AUTOMATIONS_ROW)
    expect(mocks.setRoute).toHaveBeenCalledWith("automations")

    await reopen()
    await clickItem("Import local sessions")
    // Anchored on the active folder, like the folder context-menu entry.
    expect(mocks.openImportSessionsWindow).toHaveBeenCalledWith({
      focusPath: "/tmp/repo",
    })

    await reopen()
    await clickItem("Project Boot")
    expect(mocks.openProjectBootWindow).toHaveBeenCalled()

    await reopen()
    await clickItem("To-dos")
    expect(mocks.setRoute).toHaveBeenCalledWith("tasks")

    await reopen()
    await clickItem("Show pet")
    expect(mocks.openPetWindow).toHaveBeenCalled()
  })

  it("loads the remote connections only when its submenu opens", async () => {
    await mountAndOpen()
    // Opening the root menu must not fetch — the list is submenu-scoped.
    expect(mocks.listRemoteWorkspaceConnections).not.toHaveBeenCalled()

    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Open remote workspace" })
    )
    expect(await screen.findByText("prod-box")).toBeVisible()
    expect(screen.getByText("https://lab.example")).toBeVisible()
    expect(mocks.listRemoteWorkspaceConnections).toHaveBeenCalledTimes(1)
  })

  it("opens the picked remote workspace and its manage dialog", async () => {
    await mountAndOpen()
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Open remote workspace" })
    )
    await userEvent.click(await screen.findByText("lab-box"))
    expect(mocks.openRemoteWorkspace).toHaveBeenCalledWith(12)

    await reopen()
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Open remote workspace" })
    )
    await clickItem("Manage remote workspace")
    expect(await screen.findByText("REMOTE-MANAGE-DIALOG")).toBeVisible()
  })

  it("opens the conversation manager scoped to the active folder", async () => {
    activeFolder = FOLDER
    await mountAndOpen()

    await clickItem("Manage conversations")
    expect(await screen.findByText("MANAGE-DIALOG-7")).toBeVisible()
  })

  it("disables conversation management with no active folder", async () => {
    await mountAndOpen()

    expect(
      await screen.findByRole("menuitem", { name: "Manage conversations" })
    ).toHaveAttribute("data-disabled")
  })

  it("opens the folder and clone dialogs from their rows", async () => {
    await mountAndOpen()
    await clickItem("Open Folder")
    expect(await screen.findByText("FOLDER-DIALOG")).toBeVisible()

    await reopen()
    await clickItem("Clone Repository")
    expect(await screen.findByText("CLONE-DIALOG")).toBeVisible()
  })
})
