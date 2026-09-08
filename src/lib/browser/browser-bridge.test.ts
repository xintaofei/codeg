import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const call = vi.fn()
const isDesktop = vi.fn(() => false)

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call }),
  isDesktop: () => isDesktop(),
}))

import {
  bridgeClose,
  bridgeEntryUrl,
  bridgeOpen,
  bridgeOrigin,
  bridgeStatus,
  bridgeStatusSnapshot,
  isBridgeableUrl,
  probeBridge,
  resetBridgeStatusForTests,
  type BridgeGrant,
} from "./browser-bridge"

const grant: BridgeGrant = {
  targetPort: 3000,
  bridgePort: 3081,
  entryPath: "/__codeg_bridge/enter/cap123",
  publicHost: null,
  path: "/docs?x=1",
}

beforeEach(() => {
  call.mockReset()
  isDesktop.mockReturnValue(false)
  resetBridgeStatusForTests()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("bridgeStatus", () => {
  it("asks the server once and keeps a synchronous snapshot", async () => {
    call.mockResolvedValue({ enabled: true, ports: [3081], publicHost: null })
    expect(bridgeStatusSnapshot()).toBeNull()
    const first = bridgeStatus()
    const second = bridgeStatus()
    expect(await first).toEqual({
      enabled: true,
      ports: [3081],
      publicHost: null,
    })
    await second
    expect(call).toHaveBeenCalledTimes(1)
    expect(call).toHaveBeenCalledWith("browser_bridge_status", {})
    expect(bridgeStatusSnapshot()?.enabled).toBe(true)
  })

  it("is off on the desktop without a round trip", async () => {
    isDesktop.mockReturnValue(true)
    expect(await bridgeStatus()).toEqual({
      enabled: false,
      ports: [],
      publicHost: null,
    })
    expect(call).not.toHaveBeenCalled()
    expect(bridgeStatusSnapshot()?.enabled).toBe(false)
  })

  it("treats a failed call as off and asks again next time", async () => {
    call.mockRejectedValueOnce(new Error("down"))
    expect((await bridgeStatus()).enabled).toBe(false)
    call.mockResolvedValue({ enabled: true, ports: [0], publicHost: "h" })
    expect((await bridgeStatus()).enabled).toBe(true)
    expect(call).toHaveBeenCalledTimes(2)
  })
})

describe("grants", () => {
  it("open and close go through the API with the tab id", async () => {
    call.mockResolvedValueOnce(grant)
    expect(await bridgeOpen("http://localhost:3000/docs?x=1", "tab-1")).toBe(
      grant
    )
    expect(call).toHaveBeenLastCalledWith("browser_bridge_open", {
      url: "http://localhost:3000/docs?x=1",
      tabId: "tab-1",
    })
    call.mockResolvedValueOnce({ ok: true })
    await expect(bridgeClose("tab-1")).resolves.toBeUndefined()
    expect(call).toHaveBeenLastCalledWith("browser_bridge_close", {
      tabId: "tab-1",
    })
  })
})

describe("isBridgeableUrl", () => {
  it.each([
    "http://localhost:3000/",
    "http://127.0.0.1:8080/x",
    "http://[::1]:5173/",
    "http://0.0.0.0:3000/",
    "http://app.localhost/",
  ])("%s can be bridged", (url) => {
    expect(isBridgeableUrl(url)).toBe(true)
  })

  it.each([
    "https://localhost:3000/",
    "http://192.168.1.10:3000/",
    "http://example.com/",
    "ws://localhost:3000/",
    "localhost:3000",
    "",
  ])("%s cannot", (url) => {
    expect(isBridgeableUrl(url)).toBe(false)
  })
})

describe("bridge URLs", () => {
  it("use the page's host and scheme with the bridge port", () => {
    const page = { protocol: "http:", hostname: "192.168.1.5" }
    expect(bridgeOrigin(grant, page)).toBe("http://192.168.1.5:3081")
    expect(bridgeEntryUrl(grant, page)).toBe(
      "http://192.168.1.5:3081/__codeg_bridge/enter/cap123?to=%2Fdocs%3Fx%3D1"
    )
  })

  it("prefer the server's public host and bracket IPv6", () => {
    expect(
      bridgeOrigin(
        { ...grant, publicHost: "bridge.example" },
        { protocol: "https:", hostname: "codeg.example" }
      )
    ).toBe("https://bridge.example:3081")
    expect(bridgeOrigin(grant, { protocol: "http:", hostname: "::1" })).toBe(
      "http://[::1]:3081"
    )
    // `location.hostname` keeps the brackets already.
    expect(bridgeOrigin(grant, { protocol: "http:", hostname: "[::1]" })).toBe(
      "http://[::1]:3081"
    )
  })

  it("always redirect to a root-relative path", () => {
    expect(
      bridgeEntryUrl(
        { ...grant, path: "" },
        { protocol: "http:", hostname: "h" }
      )
    ).toBe("http://h:3081/__codeg_bridge/enter/cap123?to=%2F")
  })
})

describe("probeBridge", () => {
  it("is true only for an ok answer from the ping route", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true })
    vi.stubGlobal("fetch", fetchMock)
    expect(await probeBridge("http://h:3081")).toBe(true)
    expect(fetchMock).toHaveBeenCalledWith(
      "http://h:3081/__codeg_bridge/ping",
      expect.objectContaining({
        mode: "cors",
        credentials: "omit",
        cache: "no-store",
      })
    )
    fetchMock.mockResolvedValue({ ok: false })
    expect(await probeBridge("http://h:3081")).toBe(false)
    fetchMock.mockRejectedValue(new TypeError("Failed to fetch"))
    expect(await probeBridge("http://h:3081")).toBe(false)
  })
})
