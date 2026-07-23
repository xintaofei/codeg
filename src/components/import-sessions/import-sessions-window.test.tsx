import { describe, it, expect, vi, beforeEach } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  type ReactNode,
} from "react"
import { NextIntlClientProvider } from "next-intl"
import enMessages from "@/i18n/messages/en.json"
import type { ScanResult } from "@/lib/types"
import { ImportSessionsWindow } from "./import-sessions-window"

// Render every row (windowing off) and expose a controllable handle, mirroring
// the sidebar test template — jsdom has no layout, so real virtua would render
// nothing.
const scrollToIndexSpy = vi.hoisted(() => vi.fn())
vi.mock("virtua", () => ({
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  Virtualizer: forwardRef(function MockVirtualizer(props: any, ref) {
    useImperativeHandle(ref, () => ({ scrollToIndex: scrollToIndexSpy }))
    return (
      <>
        {props.data.map((item: unknown, index: number) => (
          <div key={index}>{props.children(item, index)}</div>
        ))}
      </>
    )
  }),
}))

// Fire the viewport bridge synchronously after mount so the Virtualizer gate
// opens (the real ScrollArea only calls it once OverlayScrollbars initializes).
vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({
    children,
    onViewportRef,
  }: {
    children: ReactNode
    onViewportRef?: (el: HTMLElement | null) => void
  }) => {
    useEffect(() => {
      onViewportRef?.(document.createElement("div"))
    }, [onViewportRef])
    return <div data-testid="scroll-area">{children}</div>
  },
}))

const scanMock = vi.hoisted(() => vi.fn())
const importMock = vi.hoisted(() => vi.fn())
vi.mock("@/lib/api", () => ({
  scanImportableSessions: scanMock,
  importSelectedSessions: importMock,
}))

const subscribeMock = vi.hoisted(() =>
  vi.fn(async () => {
    return () => {}
  })
)
vi.mock("@/lib/platform", () => ({
  subscribe: subscribeMock,
}))

const toastMock = vi.hoisted(() => ({
  error: vi.fn(),
  success: vi.fn(),
  info: vi.fn(),
}))
vi.mock("sonner", () => ({ toast: toastMock }))

const NOW = "2026-07-20T08:00:00Z"

function session(
  externalId: string,
  agentType: string,
  title: string,
  status: "new" | "imported" | "deleted"
) {
  return {
    external_id: externalId,
    agent_type: agentType,
    title,
    started_at: NOW,
    ended_at: null,
    message_count: 3,
    model: null,
    git_branch: null,
    status,
  }
}

function scanFixture(): ScanResult {
  return {
    folders: [
      {
        path: "/tmp/alpha",
        name: "alpha",
        exists_in_codeg: false,
        folder_id: null,
        agent_types: ["claude_code", "codex"],
        sessions: [
          session("a1", "claude_code", "Alpha one", "new"),
          session("a2", "codex", "Alpha two", "new"),
          session("a3", "codex", "Alpha old", "imported"),
        ],
      },
      {
        path: "/tmp/beta",
        name: "beta",
        exists_in_codeg: true,
        folder_id: 7,
        agent_types: ["gemini"],
        sessions: [
          session("b1", "gemini", "Beta gone", "deleted"),
          session("b2", "gemini", "Beta new", "new"),
        ],
      },
    ],
    no_folder_count: 1,
    total_sessions: 5,
    importable_count: 3,
  } as ScanResult
}

function renderWindow(focusPath: string | null = null) {
  const onClose = vi.fn()
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ImportSessionsWindow focusPath={focusPath} onClose={onClose} />
    </NextIntlClientProvider>
  )
  return { onClose }
}

function sessionCheckbox(title: string): HTMLElement {
  return screen.getByRole("checkbox", { name: `Select session ${title}` })
}

function folderCheckbox(name: string): HTMLElement {
  return screen.getByRole("checkbox", {
    name: `Select all importable sessions in ${name}`,
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  scanMock.mockResolvedValue(scanFixture())
  importMock.mockResolvedValue({
    imported: 0,
    updated: 0,
    skipped: 0,
    not_found: 0,
    failed: 0,
    created_folders: 0,
    folders: [],
    errors: [],
  })
})

describe("ImportSessionsWindow", () => {
  it("shows the scanning phase, then the folder-grouped list", async () => {
    renderWindow()
    expect(screen.getByText("Scanning local agent sessions…")).toBeVisible()

    expect(await screen.findByText("alpha")).toBeVisible()
    expect(screen.getByText("beta")).toBeVisible()
    // All five sessions render as rows under their folders.
    expect(screen.getByText("Alpha one")).toBeVisible()
    expect(screen.getByText("Beta gone")).toBeVisible()
    // The not-in-codeg folder carries the "New" badge; the existing one not.
    const alphaHeader = screen
      .getByText("alpha")
      .closest("[data-folder-path]") as HTMLElement
    expect(alphaHeader).not.toBeNull()
    expect(alphaHeader.textContent).toContain("New")
    // Summary footer counts come from the scan payload.
    expect(
      screen.getByText("5 sessions · 3 importable · 2 folders")
    ).toBeVisible()
    expect(screen.getByText("1 without a project folder skipped")).toBeVisible()
  })

  it("disables imported/deleted rows and badges them", async () => {
    renderWindow()
    await screen.findByText("alpha")

    expect(sessionCheckbox("Alpha old")).toBeDisabled()
    expect(sessionCheckbox("Beta gone")).toBeDisabled()
    expect(sessionCheckbox("Alpha one")).toBeEnabled()
    expect(screen.getByText("Imported")).toBeVisible()
    expect(screen.getByText("Deleted")).toBeVisible()
  })

  it("folder checkbox tri-states over its importable sessions only", async () => {
    renderWindow()
    await screen.findByText("alpha")

    const alpha = folderCheckbox("alpha")
    // Select-all-in-folder: only the two NEW sessions get checked.
    fireEvent.click(alpha)
    expect(sessionCheckbox("Alpha one")).toBeChecked()
    expect(sessionCheckbox("Alpha two")).toBeChecked()
    expect(sessionCheckbox("Alpha old")).not.toBeChecked()
    expect(alpha).toHaveAttribute("data-state", "checked")

    // Unchecking one child flips the folder to indeterminate.
    fireEvent.click(sessionCheckbox("Alpha two"))
    expect(alpha).toHaveAttribute("data-state", "indeterminate")

    // Toggling the folder while partially selected selects the remainder.
    fireEvent.click(alpha)
    expect(sessionCheckbox("Alpha two")).toBeChecked()

    // Toggling while fully selected clears the folder.
    fireEvent.click(alpha)
    expect(sessionCheckbox("Alpha one")).not.toBeChecked()
    expect(sessionCheckbox("Alpha two")).not.toBeChecked()
  })

  it("select all / clear operate on every visible importable session", async () => {
    renderWindow()
    await screen.findByText("alpha")

    fireEvent.click(screen.getByRole("button", { name: "Select all" }))
    expect(screen.getByText("3 selected")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Clear" }))
    expect(screen.getByText("0 selected")).toBeVisible()
  })

  it("collapse-all hides every session row, then expand-all restores them", async () => {
    renderWindow()
    await screen.findByText("alpha")
    // Sessions start expanded under their folders.
    expect(screen.getByText("Alpha one")).toBeVisible()
    expect(screen.getByText("Beta new")).toBeVisible()

    // The toggle starts in "collapse all" mode; collapsing drops the session
    // rows while the folder headers stay.
    fireEvent.click(screen.getByRole("button", { name: "Collapse all" }))
    expect(screen.queryByText("Alpha one")).toBeNull()
    expect(screen.queryByText("Beta new")).toBeNull()
    expect(screen.getByText("alpha")).toBeVisible()
    expect(screen.getByText("beta")).toBeVisible()

    // Its icon/label flips to "expand all"; expanding brings the rows back.
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }))
    expect(screen.getByText("Alpha one")).toBeVisible()
    expect(screen.getByText("Beta new")).toBeVisible()
  })

  it("importable-only filter hides imported and deleted rows", async () => {
    renderWindow()
    await screen.findByText("alpha")

    fireEvent.click(screen.getByRole("switch"))
    expect(screen.queryByText("Alpha old")).toBeNull()
    expect(screen.queryByText("Beta gone")).toBeNull()
    expect(screen.getByText("Alpha one")).toBeVisible()
    expect(screen.getByText("Beta new")).toBeVisible()
  })

  it("imports exactly the selected keys and shows the summary", async () => {
    importMock.mockResolvedValue({
      imported: 2,
      updated: 0,
      skipped: 0,
      not_found: 0,
      failed: 0,
      created_folders: 1,
      folders: [
        {
          path: "/tmp/alpha",
          folder_id: 3,
          created: true,
          imported: 2,
          updated: 0,
          skipped: 0,
        },
      ],
      errors: [],
    })
    renderWindow()
    await screen.findByText("alpha")

    fireEvent.click(sessionCheckbox("Alpha one"))
    fireEvent.click(sessionCheckbox("Alpha two"))
    fireEvent.click(screen.getByRole("button", { name: "Import selected" }))

    await screen.findByText("Import finished")
    expect(importMock).toHaveBeenCalledTimes(1)
    expect(importMock).toHaveBeenCalledWith([
      { agentType: "claude_code", externalId: "a1" },
      { agentType: "codex", externalId: "a2" },
    ])
    // Summary tiles show the tallies.
    expect(screen.getByText("Imported")).toBeVisible()
    expect(screen.getByText("Folders created")).toBeVisible()
  })

  it("surfaces an import failure as a toast and returns to the list", async () => {
    importMock.mockRejectedValue(new Error("An import is already in progress"))
    renderWindow()
    await screen.findByText("alpha")

    fireEvent.click(sessionCheckbox("Alpha one"))
    fireEvent.click(screen.getByRole("button", { name: "Import selected" }))

    await waitFor(() => expect(toastMock.error).toHaveBeenCalledTimes(1))
    // Back on the list, selection intact.
    expect(screen.getByText("Alpha one")).toBeVisible()
    expect(screen.getByText("1 selected")).toBeVisible()
  })

  it("preselects the focusPath folder's importable sessions after the scan", async () => {
    renderWindow("/tmp/beta/")
    await screen.findByText("beta")

    expect(sessionCheckbox("Beta new")).toBeChecked()
    expect(sessionCheckbox("Alpha one")).not.toBeChecked()
    expect(screen.getByText("1 selected")).toBeVisible()
  })

  it("recovers from a scan failure via retry", async () => {
    scanMock.mockRejectedValueOnce(new Error("boom"))
    renderWindow()

    await screen.findByText("Scan failed")
    expect(screen.getByText("boom")).toBeVisible()

    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(await screen.findByText("alpha")).toBeVisible()
    expect(scanMock).toHaveBeenCalledTimes(2)
  })
})
