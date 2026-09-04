import { act, cleanup, render, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { FolderDetail } from "@/lib/types"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"

// Mirrors how the component is really mounted: hydration is flipped by the
// stores, and the tab/route halves come from contexts.
let tabs: {
  tabsHydrated: boolean
  openNewConversationTab: ReturnType<typeof vi.fn>
}
let openConversations: ReturnType<typeof vi.fn>
let openFolder: ReturnType<typeof vi.fn>
let desktop = true
let remoteWindow = true
let search = ""
let eventHandler: ((event: { payload: unknown }) => void) | null = null
let listenCalls = 0

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (selector: (s: typeof tabs) => unknown) => selector(tabs),
  useTabActions: () => tabs,
}))
vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({ openConversations }),
}))
vi.mock("@/lib/platform", () => ({
  isDesktop: () => desktop,
  isRemoteDesktopWindow: () => desktop && remoteWindow,
}))
vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(search),
}))
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (_event: string, cb: (event: { payload: unknown }) => void) => {
    eventHandler = cb
    listenCalls += 1
    return () => {}
  },
}))

const toast = vi.hoisted(() => ({ error: vi.fn() }))
vi.mock("sonner", () => ({ toast }))

import { RemoteWorkspaceOpenFolderListener } from "./remote-workspace-open-folder-listener"

const folder = (path: string) =>
  ({ id: 11, path, name: "projects" }) as FolderDetail

function renderListener() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <RemoteWorkspaceOpenFolderListener />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  eventHandler = null
  listenCalls = 0
  desktop = true
  remoteWindow = true
  search = ""
  openFolder = vi.fn(async (path: string) => folder(path))
  openConversations = vi.fn()
  tabs = { tabsHydrated: true, openNewConversationTab: vi.fn() }
  resetAppWorkspaceStore()
  useAppWorkspaceStore.setState({ foldersHydrated: true, openFolder })
})

afterEach(() => cleanup())

describe("RemoteWorkspaceOpenFolderListener", () => {
  it("opens the folder handed over when the window was spawned for it", async () => {
    // Put the params in the real URL too: the component strips the handoff
    // param from `window.location`, not from the Next router.
    window.history.replaceState(
      {},
      "",
      "/workspace?remoteConnectionId=3&openFolderPath=/srv/projects"
    )
    search = "remoteConnectionId=3&openFolderPath=/srv/projects"
    renderListener()

    await waitFor(() =>
      expect(openFolder).toHaveBeenCalledWith("/srv/projects")
    )
    expect(openConversations).toHaveBeenCalled()
    expect(tabs.openNewConversationTab).toHaveBeenCalledWith(
      11,
      "/srv/projects"
    )
    // The remote identity has to survive; only the handoff param is dropped.
    expect(window.location.search).toBe("?remoteConnectionId=3")
  })

  it("queues a spawn-time request until hydration completes", async () => {
    search = "openFolderPath=/srv/projects"
    useAppWorkspaceStore.setState({ foldersHydrated: false })
    tabs = { ...tabs, tabsHydrated: false }
    const { rerender } = renderListener()
    expect(openFolder).not.toHaveBeenCalled()

    tabs = { ...tabs, tabsHydrated: true }
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <RemoteWorkspaceOpenFolderListener />
      </NextIntlClientProvider>
    )
    act(() => {
      useAppWorkspaceStore.setState({ foldersHydrated: true })
    })

    await waitFor(() =>
      expect(openFolder).toHaveBeenCalledWith("/srv/projects")
    )
  })

  it("opens a folder handed to an already-open window", async () => {
    renderListener()
    await waitFor(() => expect(eventHandler).toBeTruthy())

    eventHandler!({ payload: { path: "/srv/api" } })
    await waitFor(() => expect(openFolder).toHaveBeenCalledWith("/srv/api"))
    expect(tabs.openNewConversationTab).toHaveBeenCalledWith(11, "/srv/api")
  })

  it("ignores an event with no path", async () => {
    renderListener()
    await waitFor(() => expect(eventHandler).toBeTruthy())

    eventHandler!({ payload: {} })
    eventHandler!({ payload: { path: "" } })
    await Promise.resolve()
    expect(openFolder).not.toHaveBeenCalled()
  })

  it("stays idle in a local window, which hands folders out, not in", () => {
    remoteWindow = false
    renderListener()
    expect(listenCalls).toBe(0)
  })

  it("stays idle on the web, where no local window can hand anything over", () => {
    desktop = false
    renderListener()
    expect(listenCalls).toBe(0)
  })

  it("reports a folder the remote host refuses to open", async () => {
    openFolder = vi.fn(async () => {
      throw new Error("not a directory")
    })
    useAppWorkspaceStore.setState({ openFolder })
    search = "openFolderPath=/srv/gone"
    renderListener()

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "Failed to open the folder",
        expect.anything()
      )
    )
  })
})
