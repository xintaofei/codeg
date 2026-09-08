import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import type { BridgeGrant } from "@/lib/browser/browser-bridge"

const api = vi.hoisted(() => ({
  bridgeOpen: vi.fn(),
  bridgeClose: vi.fn(() => Promise.resolve()),
  probeBridge: vi.fn(() => Promise.resolve(true)),
  openExternalTab: vi.fn(),
  copyTextToClipboard: vi.fn(() => Promise.resolve(true)),
  isDesktop: vi.fn(() => false),
}))

vi.mock("@/lib/browser/browser-bridge", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/browser/browser-bridge")>()),
  bridgeOpen: api.bridgeOpen,
  bridgeClose: api.bridgeClose,
  probeBridge: api.probeBridge,
}))
vi.mock("@/lib/link-open", () => ({
  openExternalTab: api.openExternalTab,
}))
vi.mock("@/lib/utils", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/utils")>()),
  copyTextToClipboard: api.copyTextToClipboard,
}))
vi.mock("@/lib/transport", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/transport")>()),
  isDesktop: () => api.isDesktop(),
}))
// The native view pulls in the surface host and the tab store; the web
// branch must not need any of it.
vi.mock("./browser-surface-host", () => ({
  BrowserSurfaceHost: () => <div data-testid="native-surface" />,
}))

import enMessages from "@/i18n/messages/en.json"
import { bridgeEntryUrl } from "@/lib/browser/browser-bridge"
import { buildFileTabId } from "@/lib/file-tab-id"

import { BRIDGE_FRAME_SANDBOX, BrowserBridgeView } from "./browser-bridge-view"
import { BrowserTabView } from "./browser-tab-view"

const grant: BridgeGrant = {
  targetPort: 3000,
  bridgePort: 3081,
  entryPath: "/__codeg_bridge/enter/cap-one",
  publicHost: null,
  path: "/docs?x=1",
}

function tab(url = "http://localhost:3000/docs?x=1"): BrowserWorkspaceTab {
  return {
    id: buildFileTabId({ kind: "browser", id: "tab-1" }),
    kind: "browser",
    folderId: null,
    title: "localhost:3000",
    description: null,
    path: null,
    language: "browser",
    content: "",
    loading: true,
    readonly: true,
    browser: { initialUrl: url, openerTabId: null },
  }
}

function renderView(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  api.bridgeOpen.mockReset()
  api.bridgeClose.mockClear()
  api.probeBridge.mockReset()
  api.probeBridge.mockResolvedValue(true)
  api.openExternalTab.mockClear()
  api.copyTextToClipboard.mockClear()
  api.isDesktop.mockReturnValue(false)
})

describe("BrowserBridgeView", () => {
  it("mints a grant for the tab and shows the page in a sandboxed frame", async () => {
    api.bridgeOpen.mockResolvedValue(grant)
    renderView(<BrowserBridgeView tab={tab()} />)
    expect(screen.getByRole("status")).toHaveTextContent(
      "Connecting through the server"
    )
    const frame = (await screen.findByTitle(
      "Dev server preview"
    )) as HTMLIFrameElement
    expect(api.bridgeOpen).toHaveBeenCalledWith(
      "http://localhost:3000/docs?x=1",
      "tab-1"
    )
    const expectedSrc = bridgeEntryUrl(grant, window.location)
    expect(frame.getAttribute("src")).toBe(expectedSrc)
    expect(expectedSrc).toContain(":3081/__codeg_bridge/enter/cap-one?to=")
    expect(frame.getAttribute("sandbox")).toBe(BRIDGE_FRAME_SANDBOX)
    expect(frame.getAttribute("sandbox")).not.toContain("allow-top-navigation")
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer")
    expect(api.probeBridge).toHaveBeenCalledWith(
      expectedSrc.slice(0, expectedSrc.indexOf("/__codeg_bridge"))
    )
    // The address shown is the one the user opened, not the bridge's.
    expect(screen.getByTitle("http://localhost:3000/docs?x=1")).toBeTruthy()
  })

  it("opens the same entry in a new tab and copies the address", async () => {
    api.bridgeOpen.mockResolvedValue(grant)
    renderView(<BrowserBridgeView tab={tab()} />)
    await screen.findByTitle("Dev server preview")
    fireEvent.click(screen.getByRole("button", { name: "Open in a new tab" }))
    expect(api.openExternalTab).toHaveBeenCalledWith(
      bridgeEntryUrl(grant, window.location)
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy address" }))
    await waitFor(() =>
      expect(api.copyTextToClipboard).toHaveBeenCalledWith(
        "http://localhost:3000/docs?x=1"
      )
    )
  })

  it("reload mints a fresh grant and reloads the frame with it", async () => {
    api.bridgeOpen.mockResolvedValueOnce(grant).mockResolvedValueOnce({
      ...grant,
      entryPath: "/__codeg_bridge/enter/cap-two",
    })
    renderView(<BrowserBridgeView tab={tab()} />)
    await screen.findByTitle("Dev server preview")
    fireEvent.click(screen.getByRole("button", { name: "Reload" }))
    await waitFor(() =>
      expect(
        screen.getByTitle("Dev server preview").getAttribute("src")
      ).toContain("cap-two")
    )
    expect(api.bridgeOpen).toHaveBeenCalledTimes(2)
  })

  it("explains an unreachable bridge port instead of a blank frame", async () => {
    api.bridgeOpen.mockResolvedValue(grant)
    api.probeBridge.mockResolvedValue(false)
    renderView(<BrowserBridgeView tab={tab()} />)
    await screen.findByText("The bridge port can't be reached")
    expect(screen.getByRole("status")).toHaveTextContent(":3081")
    expect(screen.getByRole("status")).toHaveTextContent("CODEG_BRIDGE_PORTS")
    expect(screen.queryByTitle("Dev server preview")).toBeNull()
    // A top-level tab may still get there (a VPN, a different route), so
    // the new-tab action stays.
    fireEvent.click(
      screen.getAllByRole("button", { name: "Open in a new tab" })[0]
    )
    expect(api.openExternalTab).toHaveBeenCalledWith(
      bridgeEntryUrl(grant, window.location)
    )
  })

  it("shows the server's refusal", async () => {
    api.bridgeOpen.mockRejectedValue(
      new Error("192.168.1.9 is not the server's loopback")
    )
    renderView(<BrowserBridgeView tab={tab("http://192.168.1.9:3000/")} />)
    await screen.findByText("This page can't be shown here")
    expect(screen.getByRole("status")).toHaveTextContent(
      "192.168.1.9 is not the server's loopback"
    )
    expect(
      screen.getByRole("button", { name: "Open in a new tab" })
    ).toBeDisabled()
  })

  it("releases the grant when the view goes away", async () => {
    api.bridgeOpen.mockResolvedValue(grant)
    const view = renderView(<BrowserBridgeView tab={tab()} />)
    await screen.findByTitle("Dev server preview")
    expect(api.bridgeClose).not.toHaveBeenCalled()
    await act(async () => {
      view.unmount()
    })
    expect(api.bridgeClose).toHaveBeenCalledWith("tab-1")
  })
})

describe("BrowserTabView in a browser", () => {
  it("renders the bridge view, never the native surface", async () => {
    api.bridgeOpen.mockResolvedValue(grant)
    renderView(<BrowserTabView tab={tab()} />)
    await screen.findByTitle("Dev server preview")
    expect(screen.queryByTestId("native-surface")).toBeNull()
  })
})
