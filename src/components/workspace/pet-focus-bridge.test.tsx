import { render, waitFor, cleanup } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Mutable hook returns so a test can flip hydration across rerenders. The mocks
// read these module-level vars, so reassigning + rerendering simulates the
// provider state changing.
let workspace: {
  foldersHydrated: boolean
  folders: { id: number }[]
  addFolderToWorkspaceById: ReturnType<typeof vi.fn>
}
let tabs: { tabsHydrated: boolean; openTab: ReturnType<typeof vi.fn> }
let capturedHandler: ((p: unknown) => void) | null = null

vi.mock("@/contexts/app-workspace-context", () => ({
  useAppWorkspace: () => workspace,
}))
vi.mock("@/contexts/tab-context", () => ({
  useTabContext: () => tabs,
}))
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({
    subscribe: async (_event: string, cb: (p: unknown) => void) => {
      capturedHandler = cb
      return () => {}
    },
  }),
}))

import { PetFocusBridge } from "./deep-link-bootstrap"

describe("PetFocusBridge", () => {
  beforeEach(() => {
    capturedHandler = null
    workspace = {
      foldersHydrated: false,
      folders: [{ id: 7 }],
      addFolderToWorkspaceById: vi.fn(),
    }
    tabs = { tabsHydrated: false, openTab: vi.fn() }
  })
  afterEach(() => cleanup())

  it("queues a request that arrives before hydration and replays it", async () => {
    const { rerender } = render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    // Arrives before folders/tabs (and the independently-loading conversations
    // snapshot) are ready — must not be dropped.
    capturedHandler!({ folderId: 7, conversationId: 42, agent: "claude_code" })
    expect(tabs.openTab).not.toHaveBeenCalled()

    // Hydration completes → queued request replays.
    workspace = { ...workspace, foldersHydrated: true }
    tabs = { ...tabs, tabsHydrated: true }
    rerender(<PetFocusBridge />)

    await waitFor(() =>
      expect(tabs.openTab).toHaveBeenCalledWith(7, 42, "claude_code", true)
    )
  })

  it("opens immediately when already hydrated, without re-adding an open folder", async () => {
    workspace = { ...workspace, foldersHydrated: true }
    tabs = { ...tabs, tabsHydrated: true }
    render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    capturedHandler!({ folderId: 7, conversationId: 9, agent: "codex" })
    await waitFor(() =>
      expect(tabs.openTab).toHaveBeenCalledWith(7, 9, "codex", true)
    )
    expect(workspace.addFolderToWorkspaceById).not.toHaveBeenCalled()
  })

  it("ignores malformed payloads", async () => {
    workspace = { ...workspace, foldersHydrated: true }
    tabs = { ...tabs, tabsHydrated: true }
    render(<PetFocusBridge />)
    await waitFor(() => expect(capturedHandler).toBeTruthy())

    capturedHandler!({ folderId: "x", conversationId: 1, agent: "codex" })
    capturedHandler!({ folderId: 7, conversationId: 1 }) // missing agent
    await Promise.resolve()
    expect(tabs.openTab).not.toHaveBeenCalled()
  })
})
