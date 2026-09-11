import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import { useCommandTerminalLinkStore } from "@/stores/command-terminal-link-store"

const mocks = vi.hoisted(() => ({
  activeFolder: { id: 1, path: "/repo", name: "repo" },
  createTerminalWithCommand: vi.fn(async () => "term-1"),
  terminalKill: vi.fn(async () => {}),
  listFolderCommands: vi.fn(),
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: mocks.activeFolder }),
}))

vi.mock("@/contexts/terminal-context", () => ({
  useTerminalContext: () => ({
    createTerminalWithCommand: mocks.createTerminalWithCommand,
    exitedTerminals: new Set<string>(),
    tabs: [],
  }),
}))

vi.mock("@/lib/api", () => ({
  listFolderCommands: mocks.listFolderCommands,
  bootstrapFolderCommandsFromPackageJson: vi.fn(async () => []),
  terminalKill: mocks.terminalKill,
}))

vi.mock("./command-manage-dialog", () => ({
  CommandManageDialog: () => null,
}))

import { CommandDropdown } from "./command-dropdown"

const commands = [
  {
    id: 1,
    folder_id: 1,
    name: "dev",
    command: "npm run dev",
    sort_order: 0,
    created_at: "",
    updated_at: "",
  },
  {
    id: 2,
    folder_id: 1,
    name: "build",
    command: "npm run build",
    sort_order: 1,
    created_at: "",
    updated_at: "",
  },
]

function renderDropdown() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <CommandDropdown />
    </NextIntlClientProvider>
  )
}

/**
 * jsdom has no PointerEvent constructor, so fireEvent.pointerDown degrades to
 * a propertyless Event whose `button` is undefined — which Radix's
 * `event.button === 0` gate rejects. Build MouseEvents directly instead, the
 * same way use-long-press-action.test.tsx does.
 */
function firePointer(
  target: Element,
  type: "pointerdown" | "pointermove" | "pointerup",
  init: { clientX?: number; clientY?: number } = {}
) {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: init.clientX,
    clientY: init.clientY,
    button: 0,
  })
  act(() => {
    target.dispatchEvent(event)
  })
  return event
}

/**
 * Radix dropdowns open on the trigger's pointerdown (not click), and jsdom
 * synthesizes no pointer stream from fireEvent.click — so open the menu the
 * way a real press does.
 */
async function openMenu() {
  renderDropdown()
  await act(async () => {}) // flush the initial commands load
  firePointer(screen.getByRole("button", { name: "dev" }), "pointerdown")
  await act(async () => {})
  return screen.getByRole("menuitem", { name: /build/ })
}

describe("CommandDropdown menu items", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    localStorage.clear()
    useCommandTerminalLinkStore.setState({ links: {} })
    mocks.listFolderCommands.mockResolvedValue(commands)
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it("a short press keeps selecting the command without running it", async () => {
    await openMenu()
    const item = screen.getByRole("menuitem", { name: /build/ })
    firePointer(item, "pointerdown")
    firePointer(item, "pointerup")
    fireEvent.click(item)
    await act(async () => {}) // flush the menu close

    expect(mocks.createTerminalWithCommand).not.toHaveBeenCalled()
    expect(localStorage.getItem("lastCmd:1")).toBe("2")
    // Menu closed and the pill now shows the freshly selected command.
    expect(screen.queryByRole("menuitem", { name: /build/ })).toBeNull()
    expect(screen.getByRole("button", { name: "build" })).toBeInTheDocument()
  })

  it("a long press runs the command directly and closes the menu", async () => {
    await openMenu()
    const item = screen.getByRole("menuitem", { name: /build/ })
    firePointer(item, "pointerdown")
    act(() => {
      vi.advanceTimersByTime(500)
    })

    expect(mocks.createTerminalWithCommand).toHaveBeenCalledWith(
      "build",
      "npm run build"
    )
    await act(async () => {}) // flush the menu close
    expect(screen.queryByRole("menuitem", { name: /build/ })).toBeNull()
    // Running also selects, so the pill follows the launched command.
    expect(localStorage.getItem("lastCmd:1")).toBe("2")
  })
})
