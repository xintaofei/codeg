import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { RightEdgeRail } from "./right-edge-rail"
import enMessages from "@/i18n/messages/en.json"

const state = vi.hoisted(() => ({
  activeFolder: null as unknown,
  isChatMode: false,
  isConversations: true,
  auxPanelOpen: false,
  activeTab: "session_details",
  terminalOpen: false,
  toggleAuxPanel: vi.fn(),
  openTab: vi.fn(),
  setActiveTab: vi.fn(),
  toggleTerminal: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  openSettingsWindow: vi.fn(() => Promise.resolve()),
}))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: state.activeFolder }),
}))
vi.mock("@/contexts/aux-panel-context", () => ({
  useAuxPanelContext: () => ({
    isOpen: state.auxPanelOpen,
    activeTab: state.activeTab,
    toggle: state.toggleAuxPanel,
    openTab: state.openTab,
    setActiveTab: state.setActiveTab,
  }),
}))
vi.mock("@/contexts/terminal-context", () => ({
  useTerminalContext: () => ({
    isOpen: state.terminalOpen,
    toggle: state.toggleTerminal,
  }),
}))
vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({ isConversations: state.isConversations }),
}))
vi.mock("@/components/workbench/workbench-content", () => ({
  WorkbenchRouteChromeActions: () => (
    <button type="button" aria-label="route-actions" />
  ),
}))
vi.mock("@/hooks/use-is-active-chat-mode", () => ({
  useIsActiveChatMode: () => state.isChatMode,
}))
vi.mock("@/hooks/use-is-mac", () => ({ useIsMac: () => false }))
vi.mock("@/hooks/use-shortcut-settings", () => ({
  useShortcutSettings: () => ({
    shortcuts: { toggle_terminal: "mod+j", open_settings: "mod+," },
  }),
}))

function renderRail() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <RightEdgeRail />
    </NextIntlClientProvider>
  )
}

describe("RightEdgeRail", () => {
  beforeEach(() => {
    state.activeFolder = { id: 1, name: "repo" }
    state.isChatMode = false
    state.isConversations = true
    state.auxPanelOpen = false
    state.activeTab = "session_details"
    state.terminalOpen = false
    state.toggleAuxPanel.mockClear()
    state.openTab.mockClear()
    state.setActiveTab.mockClear()
    state.toggleTerminal.mockClear()
  })

  it("opens the aux panel on the clicked tab when closed", () => {
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Changes" }))
    expect(state.openTab).toHaveBeenCalledWith("changes")
    expect(state.toggleAuxPanel).not.toHaveBeenCalled()
  })

  it("switches tabs while the panel is open on a different tab", () => {
    state.auxPanelOpen = true
    state.activeTab = "file_tree"
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Commits" }))
    expect(state.setActiveTab).toHaveBeenCalledWith("git_log")
    expect(state.toggleAuxPanel).not.toHaveBeenCalled()
  })

  it("closes the panel when the already-active tab icon is pressed", () => {
    state.auxPanelOpen = true
    state.activeTab = "file_tree"
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: "Files" }))
    expect(state.toggleAuxPanel).toHaveBeenCalled()
    expect(state.setActiveTab).not.toHaveBeenCalled()
  })

  it("hides the folder-scoped tabs without a folder", () => {
    state.activeFolder = null
    renderRail()
    expect(screen.queryByRole("button", { name: "Files" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Changes" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Commits" })).toBeNull()
    // Session details + terminal + settings remain.
    expect(screen.getAllByRole("button")).toHaveLength(3)
  })

  it("hides the folder-scoped tabs in chat mode", () => {
    state.isChatMode = true
    renderRail()
    expect(screen.queryByRole("button", { name: "Files" })).toBeNull()
  })

  it("toggles the terminal", () => {
    renderRail()
    fireEvent.click(screen.getByRole("button", { name: /Toggle Terminal/ }))
    expect(state.toggleTerminal).toHaveBeenCalled()
  })

  it("shows route chrome actions instead of panel toggles off the conversations route", () => {
    state.isConversations = false
    renderRail()
    expect(screen.getByRole("button", { name: "route-actions" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Files" })).toBeNull()
    expect(screen.queryByRole("button", { name: /Toggle Terminal/ })).toBeNull()
    // Settings stays on every route.
    expect(screen.getByRole("button", { name: /Open Settings/ })).toBeTruthy()
  })
})
