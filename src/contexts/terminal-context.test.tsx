import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { TerminalProvider, useTerminalContext } from "./terminal-context"

const h = vi.hoisted(() => ({
  terminalKill: vi.fn(async () => {}),
  remoteId: null as number | null,
  windowLabel: "main",
}))

vi.mock("@/lib/api", () => ({
  getSystemTerminalSettings: vi.fn(async () => ({ default_shell: null })),
  terminalKill: h.terminalKill,
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ subscribe: async () => () => {} }),
  getActiveRemoteConnectionId: () => h.remoteId,
}))
vi.mock("@/lib/browser/window-label", () => ({
  getCurrentWindowLabel: () => h.windowLabel,
}))
vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: { id: 7, path: "/tmp/codeg-749" },
    activeFolderId: 7,
  }),
}))
vi.mock("@/hooks/use-shortcut-settings", () => ({
  useShortcutSettings: () => ({
    shortcuts: {
      new_terminal_tab: null,
      close_current_terminal_tab: null,
    },
  }),
}))
vi.mock("@/lib/keyboard-shortcuts", () => ({
  matchShortcutEvent: () => false,
}))

function Probe() {
  const terminal = useTerminalContext()
  return (
    <div>
      <button
        onClick={() => {
          void terminal.createTerminalWithCommand("Long build", "sleep 120")
        }}
      >
        Run
      </button>
      <button
        onClick={() =>
          terminal.activeTabId && terminal.closeTerminal(terminal.activeTabId)
        }
      >
        Close
      </button>
      <span data-testid="tabs">{terminal.tabs.length}</span>
      <span data-testid="active">{terminal.activeTabId ?? ""}</span>
      <span data-testid="command">
        {terminal.tabs[0]?.initialCommand ?? ""}
      </span>
    </div>
  )
}

describe("TerminalProvider reload recovery", () => {
  beforeEach(() => {
    h.terminalKill.mockClear()
    h.remoteId = null
    h.windowLabel = "main"
    sessionStorage.clear()
    window.name = ""
  })

  it("keeps a running command owned by this page and restores its tab after reload", () => {
    const first = render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    fireEvent.click(screen.getByRole("button", { name: "Run" }))
    expect(screen.getByTestId("tabs")).toHaveTextContent("1")
    expect(screen.getByTestId("command")).toHaveTextContent("sleep 120")
    const originalId = screen.getByTestId("active").textContent

    expect(sessionStorage.getItem("codeg:terminal-session:v1")).not.toContain(
      "sleep 120"
    )
    first.unmount()
    expect(h.terminalKill).not.toHaveBeenCalled()

    render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    expect(screen.getByTestId("tabs")).toHaveTextContent("1")
    expect(screen.getByTestId("active").textContent).toBe(originalId)
    expect(screen.getByTestId("command")).toBeEmptyDOMElement()
  })

  it("explicit close records a kill even while spawn is still pending", () => {
    render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    fireEvent.click(screen.getByRole("button", { name: "Run" }))
    const id = screen.getByTestId("active").textContent
    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    expect(h.terminalKill).toHaveBeenCalledWith(id)
    expect(screen.getByTestId("tabs")).toHaveTextContent("0")
    expect(sessionStorage.getItem("codeg:terminal-session:v1")).not.toContain(
      id
    )
  })

  it("rejects a forged terminal ID and preserves an unrelated window name", () => {
    window.name = "host-window"
    sessionStorage.setItem(
      "codeg:terminal-session:v1",
      JSON.stringify({
        version: 1,
        scope: JSON.stringify(["main", null]),
        pageId: "host-window",
        isOpen: true,
        activeTabId: "other-user-terminal",
        tabs: [
          {
            id: "other-user-terminal",
            folderId: 7,
            title: "bad",
            workingDir: "/tmp",
          },
        ],
      })
    )
    render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    expect(window.name).toBe("host-window")
    expect(screen.getByTestId("tabs")).toHaveTextContent("0")
  })

  it("does not auto-attach a copied opener session in a new tab", () => {
    const first = render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    fireEvent.click(screen.getByRole("button", { name: "Run" }))
    const copiedStorage = sessionStorage.getItem("codeg:terminal-session:v1")
    expect(copiedStorage).toBeTruthy()
    first.unmount()

    // New browsing context: the opener's sessionStorage is cloned, but the
    // browser gives this tab its own window.name.
    window.name = ""
    sessionStorage.setItem("codeg:terminal-session:v1", copiedStorage!)
    render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    expect(screen.getByTestId("tabs")).toHaveTextContent("0")
    expect(h.terminalKill).not.toHaveBeenCalled()
  })

  it("does not restore another remote workspace window's terminals", () => {
    h.windowLabel = "remote-workspace-3"
    h.remoteId = 3
    const first = render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    fireEvent.click(screen.getByRole("button", { name: "Run" }))
    expect(screen.getByTestId("tabs")).toHaveTextContent("1")
    first.unmount()

    h.windowLabel = "remote-workspace-4"
    h.remoteId = 4
    render(
      <TerminalProvider>
        <Probe />
      </TerminalProvider>
    )
    expect(screen.getByTestId("tabs")).toHaveTextContent("0")
  })
})
