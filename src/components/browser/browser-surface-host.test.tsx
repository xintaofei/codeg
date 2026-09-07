import { act, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import type { BrowserTabState } from "@/lib/browser/types"

const api = vi.hoisted(() => ({
  browserOpenTab: vi.fn(),
  browserSetBounds: vi.fn(() => Promise.resolve()),
  browserSetVisible: vi.fn(() => Promise.resolve()),
}))
vi.mock("@/lib/browser/browser-api", () => api)
vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceView: () => ({
    mode: "conversation",
    activePane: "files",
    filesMaximized: false,
  }),
}))
vi.mock("@/contexts/workbench-route-context", () => ({
  useOptionalWorkbenchRoute: () => null,
}))
vi.mock("@/components/ui/overlay-host-hidden", () => ({
  useOverlayHostHidden: () => false,
}))

import { BrowserSurfaceHost } from "./browser-surface-host"
import { resetBrowserTabStoreForTests } from "@/lib/browser/browser-tab-store"
import {
  acquireNativeSurfaceOcclusion,
  resetNativeSurfaceOcclusionForTests,
} from "@/lib/browser/native-surface-occlusion"

function tab(id = "abc"): BrowserWorkspaceTab {
  return {
    id: `browser:${id}`,
    kind: "browser",
    folderId: 1,
    title: "example.com",
    description: null,
    path: null,
    language: "browser",
    content: "",
    loading: true,
    readonly: true,
    browser: { initialUrl: "https://example.com/", openerTabId: null },
  }
}

function state(id = "abc"): BrowserTabState {
  return {
    tabId: id,
    ownerWindow: "main",
    surface: "child",
    channel: "degraded",
    url: "",
    requestedUrl: "https://example.com/",
    title: "",
    favicon: null,
    loading: true,
    canGoBack: false,
    canGoForward: false,
    origin: null,
    zoom: 1,
    error: null,
    remoteHost: null,
    openerTabId: null,
  }
}

async function flush() {
  await act(async () => {
    await Promise.resolve()
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

describe("BrowserSurfaceHost", () => {
  beforeEach(() => {
    api.browserOpenTab.mockReset()
    api.browserSetBounds.mockClear()
    api.browserSetVisible.mockClear()
    resetBrowserTabStoreForTests()
    resetNativeSurfaceOcclusionForTests()
    // jsdom has no layout: give the host a rect.
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 100,
      y: 50,
      left: 100,
      top: 50,
      width: 800,
      height: 600,
      right: 900,
      bottom: 650,
      toJSON: () => ({}),
    })
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("creates the surface once at its rect, hides it under an overlay lease, and hides on unmount", async () => {
    api.browserOpenTab.mockImplementation(() => Promise.resolve(state("host1")))
    const { unmount } = render(<BrowserSurfaceHost tab={tab("host1")} />)
    await flush()
    expect(api.browserOpenTab).toHaveBeenCalledTimes(1)
    expect(api.browserOpenTab.mock.calls[0][0]).toMatchObject({
      tabId: "host1",
      url: "https://example.com/",
      bounds: { x: 100, y: 50, width: 800, height: 600 },
    })

    let release: () => void = () => {}
    await act(async () => {
      release = acquireNativeSurfaceOcclusion("dialog")
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    // Hidden with focus handoff.
    expect(api.browserSetVisible).toHaveBeenLastCalledWith("host1", false, true)

    await act(async () => {
      release()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(api.browserSetVisible).toHaveBeenLastCalledWith("host1", true, false)

    unmount()
    expect(api.browserSetVisible).toHaveBeenLastCalledWith(
      "host1",
      false,
      false
    )
  })

  it("drives an owned window from tab visibility, not from its invisible placeholder", async () => {
    // The tab view hides the placeholder (`invisible`) when the page lives in
    // its own window; the window must still be shown, and never sized.
    Object.defineProperty(HTMLElement.prototype, "checkVisibility", {
      configurable: true,
      value: () => false,
    })
    try {
      api.browserOpenTab.mockImplementation(() =>
        Promise.resolve({ ...state("host-window"), surface: "window" })
      )
      const { unmount } = render(
        <BrowserSurfaceHost tab={tab("host-window")} />
      )
      await flush()
      expect(api.browserSetBounds).not.toHaveBeenCalled()
      expect(api.browserSetVisible).toHaveBeenLastCalledWith(
        "host-window",
        true,
        false
      )
      unmount()
      expect(api.browserSetVisible).toHaveBeenLastCalledWith(
        "host-window",
        false,
        false
      )
    } finally {
      delete (HTMLElement.prototype as { checkVisibility?: unknown })
        .checkVisibility
    }
  })

  it("does not create a second surface for a tab that already has one", async () => {
    api.browserOpenTab.mockImplementation(() => Promise.resolve(state("host2")))
    const first = render(<BrowserSurfaceHost tab={tab("host2")} />)
    await flush()
    first.unmount()
    render(<BrowserSurfaceHost tab={tab("host2")} />)
    await flush()
    expect(api.browserOpenTab).toHaveBeenCalledTimes(1)
    // Re-mount re-applies bounds and shows the existing surface.
    expect(api.browserSetBounds).toHaveBeenCalledWith("host2", {
      x: 100,
      y: 50,
      width: 800,
      height: 600,
    })
    expect(api.browserSetVisible).toHaveBeenLastCalledWith("host2", true, false)
  })

  it("stays hidden while the view is force-hidden (error page)", async () => {
    api.browserOpenTab.mockImplementation(() => Promise.resolve(state("host3")))
    render(<BrowserSurfaceHost tab={tab("host3")} hidden />)
    await flush()
    expect(api.browserSetVisible).toHaveBeenLastCalledWith("host3", false, true)
  })
})
