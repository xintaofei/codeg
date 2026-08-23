import { beforeEach, describe, expect, it, vi } from "vitest"

/**
 * `openUrl` is the app's only sanctioned way to leave for the outside world
 * (see `@/components/ui/browser-link`, which routes every external link
 * through it), so both of its branches are load-bearing:
 *
 * - A Tauri webview opens nothing for `window.open` / `target="_blank"` — no
 *   new-window handler is registered — so ANY Tauri window, remote workspace
 *   included, has to reach the opener plugin.
 * - The web branch must pass `noreferrer`, or the page we open keeps a
 *   `window.opener` handle back into the app.
 */

const mocks = vi.hoisted(() => ({
  isDesktop: vi.fn(() => false),
  getActiveRemoteConnectionId: vi.fn<() => string | null>(() => null),
  tauriOpenUrl: vi.fn(async () => {}),
  tauriOpenPath: vi.fn(async () => {}),
  tauriReveal: vi.fn(async () => {}),
}))

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({}),
  isDesktop: mocks.isDesktop,
  getActiveRemoteConnectionId: mocks.getActiveRemoteConnectionId,
}))

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: mocks.tauriOpenUrl,
  openPath: mocks.tauriOpenPath,
  revealItemInDir: mocks.tauriReveal,
}))

import { openPath, openUrl } from "@/lib/platform"

const URL = "https://example.com/issues/1"

beforeEach(() => {
  mocks.isDesktop.mockReset()
  mocks.isDesktop.mockReturnValue(false)
  mocks.getActiveRemoteConnectionId.mockReset()
  mocks.getActiveRemoteConnectionId.mockReturnValue(null)
  mocks.tauriOpenUrl.mockClear()
  mocks.tauriOpenPath.mockClear()
})

describe("openUrl", () => {
  it("uses the opener plugin on a local desktop window", async () => {
    mocks.isDesktop.mockReturnValue(true)
    await openUrl(URL)
    expect(mocks.tauriOpenUrl).toHaveBeenCalledWith(URL)
  })

  it("uses the opener plugin in a remote-workspace window too", async () => {
    // A remote-desktop window is still a Tauri webview: falling through to
    // `window.open` here would leave every external link dead.
    mocks.isDesktop.mockReturnValue(true)
    mocks.getActiveRemoteConnectionId.mockReturnValue("conn-7")
    const open = vi.spyOn(window, "open").mockReturnValue(null)
    await openUrl(URL)
    expect(mocks.tauriOpenUrl).toHaveBeenCalledWith(URL)
    expect(open).not.toHaveBeenCalled()
    open.mockRestore()
  })

  it("opens a web-mode tab with noreferrer, so the page gets no opener", async () => {
    mocks.isDesktop.mockReturnValue(false)
    const open = vi.spyOn(window, "open").mockReturnValue(null)
    await openUrl(URL)
    expect(open).toHaveBeenCalledWith(URL, "_blank", "noreferrer")
    expect(mocks.tauriOpenUrl).not.toHaveBeenCalled()
    open.mockRestore()
  })
})

describe("openPath", () => {
  it("keeps its remote guard — a path belongs to a specific host", async () => {
    // The contrast that explains why `openUrl` drops the guard: revealing a
    // remote workspace's path on the LOCAL filesystem would open the wrong
    // machine's file, or nothing at all.
    mocks.isDesktop.mockReturnValue(true)
    mocks.getActiveRemoteConnectionId.mockReturnValue("conn-7")
    await openPath("/repo/README.md")
    expect(mocks.tauriOpenPath).not.toHaveBeenCalled()

    mocks.getActiveRemoteConnectionId.mockReturnValue(null)
    await openPath("/repo/README.md")
    expect(mocks.tauriOpenPath).toHaveBeenCalledWith("/repo/README.md")
  })
})
