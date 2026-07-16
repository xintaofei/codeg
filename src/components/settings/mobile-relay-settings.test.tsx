import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { MobileRelaySettingsCard } from "./mobile-relay-settings"

const getSettings = vi.fn()
const saveSettings = vi.fn()
const createPairing = vi.fn()
const getPairingStatus = vi.fn()
const confirmPairing = vi.fn()
const rejectPairing = vi.fn()
const revokeDevice = vi.fn()

vi.mock("@/lib/api", () => ({
  getMobileRelaySettings: (...args: unknown[]) => getSettings(...args),
  saveMobileRelaySettings: (...args: unknown[]) => saveSettings(...args),
  createMobileRelayPairing: (...args: unknown[]) => createPairing(...args),
  getMobileRelayPairingStatus: (...args: unknown[]) =>
    getPairingStatus(...args),
  confirmMobileRelayPairing: (...args: unknown[]) => confirmPairing(...args),
  rejectMobileRelayPairing: (...args: unknown[]) => rejectPairing(...args),
  revokeMobileRelayDevice: (...args: unknown[]) => revokeDevice(...args),
}))

const settings = {
  enabled: true,
  relayUrl: "wss://relay.example.test/v1/ws",
  desktopId: "d_test",
  relayTokenConfigured: true,
  bridgeRunning: true,
  devices: [
    {
      deviceId: "m_phone",
      name: "Android",
      createdAt: 1,
      lastSeenAt: null,
      revokedAt: null,
    },
  ],
}

describe("MobileRelaySettingsCard", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getSettings.mockResolvedValue(settings)
    saveSettings.mockResolvedValue(settings)
    createPairing.mockResolvedValue({
      pairId: "p_new",
      expiresAt: Math.floor(Date.now() / 1000) + 300,
      payload: JSON.stringify({ v: 2, pairing: "secret" }),
    })
    getPairingStatus.mockResolvedValue({
      status: "waiting",
      expiresAt: Math.floor(Date.now() / 1000) + 300,
      deviceId: null,
      deviceName: null,
      sas: null,
    })
    confirmPairing.mockResolvedValue(undefined)
    rejectPairing.mockResolvedValue(undefined)
    revokeDevice.mockResolvedValue(undefined)
  })

  it("creates a QR pairing and uses an in-app revoke confirmation", async () => {
    render(<MobileRelaySettingsCard />)

    expect(
      await screen.findByDisplayValue("wss://relay.example.test/v1/ws")
    ).toBeInTheDocument()
    expect(screen.getByText("Android")).toBeInTheDocument()
    expect(screen.getByText("自托管 Relay")).toBeInTheDocument()
    expect(screen.getByText("当前端点：relay.example.test")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "生成配对码" }))
    expect(await screen.findByText("扫描并配对手机")).toBeInTheDocument()
    expect(createPairing).toHaveBeenCalledWith("")

    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    await waitFor(() => expect(rejectPairing).toHaveBeenCalled())
    expect(rejectPairing.mock.calls[0]?.[0]).toBe("p_new")
    await waitFor(() =>
      expect(screen.queryByText("扫描并配对手机")).not.toBeInTheDocument()
    )
    fireEvent.click(screen.getByRole("button", { name: "撤销 Android" }))
    expect(await screen.findByText("撤销这台手机？")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "确认撤销" }))
    await waitFor(() => expect(revokeDevice).toHaveBeenCalledWith("m_phone"))
  })

  it("closes a pairing whose desktop confirmation succeeded after its response was lost", async () => {
    getPairingStatus.mockResolvedValue({
      status: "accepted",
      expiresAt: Math.floor(Date.now() / 1000) + 300,
      deviceId: "m_phone",
      deviceName: "Android",
      sas: null,
    })
    render(<MobileRelaySettingsCard />)

    await screen.findByDisplayValue("wss://relay.example.test/v1/ws")
    fireEvent.click(screen.getByRole("button", { name: "生成配对码" }))
    await waitFor(() => expect(getPairingStatus).toHaveBeenCalledWith("p_new"))
    await waitFor(() =>
      expect(screen.queryByText("扫描并配对手机")).not.toBeInTheDocument()
    )
  })
})
