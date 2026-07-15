import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { MobileRelaySettingsCard } from "./mobile-relay-settings"

const getSettings = vi.fn()
const saveSettings = vi.fn()
const createPairing = vi.fn()
const revokeDevice = vi.fn()

vi.mock("@/lib/api", () => ({
  getMobileRelaySettings: (...args: unknown[]) => getSettings(...args),
  saveMobileRelaySettings: (...args: unknown[]) => saveSettings(...args),
  createMobileRelayPairing: (...args: unknown[]) => createPairing(...args),
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
      deviceId: "m_new",
      expiresAt: Math.floor(Date.now() / 1000) + 300,
      payload: JSON.stringify({ v: 1, pairing: "secret" }),
    })
    revokeDevice.mockResolvedValue(undefined)
  })

  it("creates a QR pairing and uses an in-app revoke confirmation", async () => {
    render(<MobileRelaySettingsCard />)

    expect(
      await screen.findByDisplayValue("wss://relay.example.test/v1/ws")
    ).toBeInTheDocument()
    expect(screen.getByText("Android")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "生成配对码" }))
    expect(await screen.findByText("扫描并配对手机")).toBeInTheDocument()
    expect(createPairing).toHaveBeenCalledWith("")

    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    fireEvent.click(screen.getByRole("button", { name: "撤销 Android" }))
    expect(await screen.findByText("撤销这台手机？")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "确认撤销" }))
    await waitFor(() => expect(revokeDevice).toHaveBeenCalledWith("m_phone"))
  })
})
