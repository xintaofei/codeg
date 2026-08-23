import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const surface = vi.hoisted(() => ({
  attachBrowserSurface: vi.fn(),
  detachBrowserSurface: vi.fn(),
  closeBrowserSurface: vi.fn(),
  runBrowserSurfaceAction: vi.fn(),
}))
const connection = vi.hoisted(() => ({ connectionId: "connection-a" }))
const api = vi.hoisted(() => ({
  getBrowserRuntimeSettings: vi.fn(),
  recoverBrowserRuntime: vi.fn(),
  updateBrowserRuntimeSettings: vi.fn(),
}))

vi.mock("@/lib/browser-surface", () => surface)
vi.mock("@/lib/api", () => api)
vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (selector: (state: { activeTabId: string }) => unknown) =>
    selector({ activeTabId: "tab-a" }),
}))
vi.mock("@/hooks/use-connection", () => ({
  useConnection: () => connection,
}))
vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

import {
  AuxPanelBrowserTab,
  browserFramePoint,
  browserNativeSurfaceBounds,
  browserPointerButton,
} from "./aux-panel-browser-tab"
import type {
  BrowserSurfaceAction,
  BrowserSurfaceEvent,
  BrowserSurfaceSnapshot,
} from "@/lib/browser-surface"

const snapshot: BrowserSurfaceSnapshot = {
  sessionId: "connection-a",
  surfaceKind: "native",
  tabs: [
    { id: "t1", title: "One", url: "https://one.example/" },
    { id: "t2", title: "Two", url: "https://two.example/" },
  ],
  activeTargetId: "t1",
  active: {
    tab: { id: "t1", title: "One", url: "https://one.example/" },
    loading: false,
    canGoBack: true,
    canGoForward: false,
  },
}

const frameSnapshot: BrowserSurfaceSnapshot = {
  ...snapshot,
  surfaceKind: "frame",
}

describe("AuxPanelBrowserTab", () => {
  let emit: ((event: BrowserSurfaceEvent) => void) | null

  const attachSnapshot = (next: BrowserSurfaceSnapshot) => {
    surface.attachBrowserSurface.mockImplementation(
      async (
        _connectionId: string,
        listener: (event: BrowserSurfaceEvent) => void
      ) => {
        emit = listener
        return next
      }
    )
    surface.runBrowserSurfaceAction.mockResolvedValue(next)
  }

  beforeEach(() => {
    vi.clearAllMocks()
    emit = null
    connection.connectionId = "connection-a"
    attachSnapshot(snapshot)
    surface.detachBrowserSurface.mockResolvedValue(undefined)
    surface.closeBrowserSurface.mockResolvedValue(undefined)
    api.getBrowserRuntimeSettings.mockResolvedValue({
      enabled: true,
      autoStart: true,
      browserPath: null,
      backend: "embedded",
    })
    api.updateBrowserRuntimeSettings.mockResolvedValue({
      enabled: true,
      autoStart: true,
      browserPath: null,
      backend: "external",
    })
    api.recoverBrowserRuntime.mockResolvedValue(undefined)
  })

  it("attaches once to the real connection and renders every live target", async () => {
    const { rerender } = render(
      <AuxPanelBrowserTab visible onExplicitClose={() => undefined} />
    )

    expect(await screen.findByText("One")).toBeInTheDocument()
    expect(screen.getByText("Two")).toBeInTheDocument()
    expect(surface.attachBrowserSurface).toHaveBeenCalledTimes(1)
    expect(surface.attachBrowserSurface).toHaveBeenCalledWith(
      "connection-a",
      expect.any(Function)
    )

    rerender(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    expect(surface.attachBrowserSurface).toHaveBeenCalledTimes(1)
  })

  it("focuses inner targets and navigates through the same surface", async () => {
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("Two")
    fireEvent.click(screen.getByRole("button", { name: "Two" }))
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        { action: "focus", targetId: "t2" }
      )
    })

    const address = screen.getByRole("textbox", { name: "addressBar" })
    fireEvent.change(address, { target: { value: "example.com" } })
    fireEvent.keyDown(address, { key: "Enter" })
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        { action: "navigate", url: "https://example.com" }
      )
    })
  })

  it("detaches on hide without releasing targets", async () => {
    const { rerender } = render(
      <AuxPanelBrowserTab visible onExplicitClose={() => undefined} />
    )
    await screen.findByText("One")
    rerender(
      <AuxPanelBrowserTab visible={false} onExplicitClose={() => undefined} />
    )

    await waitFor(() => {
      expect(surface.detachBrowserSurface).toHaveBeenCalledWith("connection-a")
    })
    expect(surface.closeBrowserSurface).not.toHaveBeenCalled()
  })

  it("moves the native surface attachment when the Agent session changes", async () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect")
    rect.mockReturnValue({
      width: 240,
      height: 180,
      top: 80,
      right: 400,
      bottom: 260,
      left: 160,
      x: 160,
      y: 80,
      toJSON: () => ({}),
    })
    surface.attachBrowserSurface.mockImplementation(
      async (
        connectionId: string,
        listener: (event: BrowserSurfaceEvent) => void
      ) => {
        emit = listener
        return { ...snapshot, sessionId: connectionId }
      }
    )
    const { rerender } = render(
      <AuxPanelBrowserTab visible onExplicitClose={() => undefined} />
    )
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        expect.objectContaining({ action: "surface", visible: true })
      )
    })

    connection.connectionId = "connection-b"
    rerender(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await waitFor(() => {
      expect(surface.detachBrowserSurface).toHaveBeenCalledWith("connection-a")
      expect(surface.attachBrowserSurface).toHaveBeenCalledWith(
        "connection-b",
        expect.any(Function)
      )
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-b",
        expect.objectContaining({ action: "surface", visible: true })
      )
    })
    rect.mockRestore()
  })

  it("explicit close releases the whole Browser session", async () => {
    const onExplicitClose = vi.fn()
    render(<AuxPanelBrowserTab visible onExplicitClose={onExplicitClose} />)
    await screen.findByText("One")
    fireEvent.click(screen.getByRole("button", { name: "closeBrowser" }))

    await waitFor(() => {
      expect(surface.closeBrowserSurface).toHaveBeenCalledWith("connection-a")
      expect(onExplicitClose).toHaveBeenCalledWith("connection-a")
    })
  })

  it("offers an explicit switch to External when native startup fails", async () => {
    surface.attachBrowserSurface.mockRejectedValue(
      new Error("NATIVE_CONTROLLER_FAILED")
    )
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "NATIVE_CONTROLLER_FAILED"
    )
    fireEvent.click(screen.getByRole("button", { name: "switchToExternal" }))

    await waitFor(() => {
      expect(api.updateBrowserRuntimeSettings).toHaveBeenCalledWith({
        enabled: true,
        autoStart: true,
        browserPath: null,
        backend: "external",
      })
      expect(api.recoverBrowserRuntime).toHaveBeenCalledTimes(1)
      expect(surface.attachBrowserSurface).toHaveBeenCalledTimes(2)
    })
  })

  it("accepts authoritative full snapshots from the stream", async () => {
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("Two")
    act(() => {
      emit?.({
        type: "snapshot",
        snapshot: {
          ...snapshot,
          tabs: [snapshot.tabs[1]!],
          activeTargetId: "t2",
          active: {
            ...snapshot.active!,
            tab: snapshot.tabs[1]!,
          },
        },
      })
    })

    expect(await screen.findByText("Two")).toBeInTheDocument()
    expect(screen.queryByText("One")).not.toBeInTheDocument()
  })

  it("places and shows the native surface in logical window coordinates", async () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect")
    rect.mockReturnValue({
      width: 320,
      height: 620,
      top: 80,
      right: 400,
      bottom: 700,
      left: 80,
      x: 80,
      y: 80,
      toJSON: () => ({}),
    })

    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)

    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        {
          action: "surface",
          bounds: { x: 80, y: 80, width: 320, height: 620 },
          visible: true,
        }
      )
    })
    rect.mockRestore()
  })

  it("hides the native child while a host menu is open and restores it", async () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect")
    rect.mockReturnValue({
      width: 320,
      height: 200,
      top: 80,
      right: 400,
      bottom: 280,
      left: 80,
      x: 80,
      y: 80,
      toJSON: () => ({}),
    })
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        expect.objectContaining({ action: "surface", visible: true })
      )
    })

    const menu = document.createElement("div")
    menu.setAttribute("role", "menu")
    menu.setAttribute("data-state", "open")
    act(() => document.body.append(menu))
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        expect.objectContaining({ action: "surface", visible: false })
      )
    })

    act(() => menu.remove())
    await waitFor(() => {
      const visibility = surface.runBrowserSurfaceAction.mock.calls
        .map(([, action]) => action)
        .filter((action) => action.action === "surface")
        .map((action) => action.visible)
      expect(visibility).toEqual([true, false, true])
    })
    rect.mockRestore()
  })

  it("does not render frames or reconstruct input for a native surface", async () => {
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("One")
    act(() => {
      emit?.({
        type: "frame",
        frame: {
          targetId: "t1",
          data: "AA==",
          mimeType: "image/jpeg",
          deviceWidth: 320,
          deviceHeight: 200,
          pageScaleFactor: 1,
        },
      })
    })

    const slot = screen.getByTestId("browser-surface-slot")
    expect(slot).toHaveAttribute("data-surface-kind", "native")
    expect(slot.querySelector("img")).toBeNull()
    fireEvent.pointerDown(slot, { button: 0, clientX: 40, clientY: 60 })
    fireEvent.keyDown(slot, { key: "a", code: "KeyA" })
    expect(
      surface.runBrowserSurfaceAction.mock.calls.some(
        ([, action]) => action.action === "input"
      )
    ).toBe(false)
  })

  it("keeps frame viewport resizing for the External backend", async () => {
    attachSnapshot(frameSnapshot)
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect")
    rect.mockReturnValue({
      width: 320,
      height: 620,
      top: 0,
      right: 320,
      bottom: 620,
      left: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    })

    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        { action: "resize", width: 320, height: 620 }
      )
    })
    rect.mockRestore()
  })

  it("renders frames without non-uniform stretching", async () => {
    attachSnapshot(frameSnapshot)
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("One")
    act(() => {
      emit?.({
        type: "frame",
        frame: {
          targetId: "t1",
          data: "AA==",
          mimeType: "image/jpeg",
          deviceWidth: 1_600,
          deviceHeight: 900,
          pageScaleFactor: 1,
        },
      })
    })

    expect(screen.getByRole("application").querySelector("img")).toHaveClass(
      "object-contain"
    )
  })

  it("forwards pointer movement through the fitted frame", async () => {
    attachSnapshot(frameSnapshot)
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("One")
    act(() => {
      emit?.({
        type: "frame",
        frame: {
          targetId: "t1",
          data: "AA==",
          mimeType: "image/jpeg",
          deviceWidth: 320,
          deviceHeight: 200,
          pageScaleFactor: 1,
        },
      })
    })

    const page = screen.getByRole("application")
    vi.spyOn(page, "getBoundingClientRect").mockReturnValue({
      width: 320,
      height: 200,
      top: 0,
      right: 320,
      bottom: 200,
      left: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    })
    fireEvent(
      page,
      new MouseEvent("pointermove", {
        bubbles: true,
        clientX: 40,
        clientY: 60,
      })
    )

    await waitFor(() => {
      expect(surface.runBrowserSurfaceAction).toHaveBeenCalledWith(
        "connection-a",
        {
          action: "input",
          input: {
            kind: "mouse",
            event: "moved",
            x: 40,
            y: 60,
            button: "none",
            modifiers: 0,
          },
        }
      )
    })
  })

  it.each([
    [0, "none"],
    [1, "left"],
    [2, "right"],
    [4, "middle"],
  ] as const)("maps pointer buttons=%d to %s", (buttons, button) => {
    expect(browserPointerButton(buttons)).toBe(button)
  })

  it("refreshes the pointer position before pressing", async () => {
    attachSnapshot(frameSnapshot)
    render(<AuxPanelBrowserTab visible onExplicitClose={() => undefined} />)
    await screen.findByText("One")
    act(() => {
      emit?.({
        type: "frame",
        frame: {
          targetId: "t1",
          data: "AA==",
          mimeType: "image/jpeg",
          deviceWidth: 320,
          deviceHeight: 200,
          pageScaleFactor: 1,
        },
      })
    })

    const page = screen.getByRole("application")
    vi.spyOn(page, "getBoundingClientRect").mockReturnValue({
      width: 320,
      height: 200,
      top: 0,
      right: 320,
      bottom: 200,
      left: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    })
    let resolveMoved: ((value: BrowserSurfaceSnapshot) => void) | undefined
    surface.runBrowserSurfaceAction.mockImplementation(
      async (_connectionId: string, action: BrowserSurfaceAction) => {
        if (
          action.action !== "input" ||
          action.input.kind !== "mouse" ||
          action.input.event !== "moved"
        ) {
          return frameSnapshot
        }
        return await new Promise<BrowserSurfaceSnapshot>((resolve) => {
          resolveMoved = resolve
        })
      }
    )
    fireEvent(
      page,
      new MouseEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: 140,
        clientY: 160,
      })
    )

    await waitFor(() => {
      const inputActions = surface.runBrowserSurfaceAction.mock.calls
        .map(([, action]) => action)
        .filter((action) => action.action === "input")
      expect(inputActions).toEqual([
        {
          action: "input",
          input: {
            kind: "mouse",
            event: "moved",
            x: 140,
            y: 160,
            button: "none",
            modifiers: 0,
          },
        },
      ])
    })

    act(() => resolveMoved?.(frameSnapshot))

    await waitFor(() => {
      const inputActions = surface.runBrowserSurfaceAction.mock.calls
        .map(([, action]) => action)
        .filter((action) => action.action === "input")
      expect(inputActions).toEqual([
        {
          action: "input",
          input: {
            kind: "mouse",
            event: "moved",
            x: 140,
            y: 160,
            button: "none",
            modifiers: 0,
          },
        },
        {
          action: "input",
          input: {
            kind: "mouse",
            event: "pressed",
            x: 140,
            y: 160,
            button: "left",
            modifiers: 0,
          },
        },
      ])
    })
  })

  it.each([
    [320, 620],
    [320, 200],
    [620, 320],
  ])(
    "maps input directly when the page viewport matches a %d x %d surface",
    (width, height) => {
      expect(browserFramePoint(width, height, width, height, 0, 0)).toEqual({
        x: 0,
        y: 0,
      })
      expect(
        browserFramePoint(width, height, width, height, width / 2, height / 2)
      ).toEqual({ x: width / 2, y: height / 2 })
      expect(
        browserFramePoint(width, height, width, height, width, height)
      ).toEqual({ x: width, y: height })
    }
  )

  it("maps the fitted frame and rejects temporary letterbox coordinates", () => {
    expect(browserFramePoint(320, 620, 1_600, 900, 160, 310)).toEqual({
      x: 800,
      y: 450,
    })
    expect(browserFramePoint(320, 620, 1_600, 900, 160, 100)).toBeNull()
    expect(browserFramePoint(320, 200, 1_600, 900, -1, 100)).toBeNull()
    expect(browserFramePoint(320, 200, 1_600, 900, 321, 100)).toBeNull()
    expect(browserFramePoint(320, 200, 1_600, 900, 160, 201)).toBeNull()
  })

  it("clips native logical bounds without applying device pixel scaling", () => {
    expect(
      browserNativeSurfaceBounds(
        { left: -20, top: 80, right: 400, bottom: 700 },
        1_024,
        640
      )
    ).toEqual({ x: 0, y: 80, width: 400, height: 560 })
  })
})
