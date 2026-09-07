import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getDelegationSettings: vi.fn(),
  setDelegationSettings: vi.fn(),
  getContinuationSettings: vi.fn(),
  setContinuationSettings: vi.fn(),
  acpListAgents: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}))

import { DelegationSettingsSection } from "./delegation-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  acpListAgents,
  getContinuationSettings,
  getDelegationSettings,
  setContinuationSettings,
  setDelegationSettings,
  type DelegationSettings,
} from "@/lib/api"
import { toast } from "sonner"

const mockGetDelegationSettings = vi.mocked(getDelegationSettings)
const mockGetContinuationSettings = vi.mocked(getContinuationSettings)
const mockSetDelegationSettings = vi.mocked(setDelegationSettings)
const mockSetContinuationSettings = vi.mocked(setContinuationSettings)
const mockAcpListAgents = vi.mocked(acpListAgents)
const mockToast = vi.mocked(toast)

function settings(): DelegationSettings {
  return {
    enabled: true,
    depth_limit: 1,
    completed_cache_max_mb: 512,
    agent_defaults: {},
  }
}

function renderSettings() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <DelegationSettingsSection />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  mockAcpListAgents.mockResolvedValue([])
})

describe("continuation settings integration", () => {
  it("keeps Save disabled while the main settings read is pending", async () => {
    mockGetDelegationSettings.mockReturnValue(new Promise(() => {}))
    mockGetContinuationSettings.mockResolvedValue({
      continuable_delegation_enabled: false,
    })

    await act(async () => {
      renderSettings()
      await Promise.resolve()
    })

    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  })

  it("keeps Save disabled while the continuation read is pending", async () => {
    mockGetDelegationSettings.mockResolvedValue(settings())
    mockGetContinuationSettings.mockReturnValue(new Promise(() => {}))

    await act(async () => {
      renderSettings()
      await Promise.resolve()
    })

    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  })

  it("shows a main settings read failure and blocks Save", async () => {
    mockGetDelegationSettings.mockRejectedValue(new Error("main unavailable"))
    mockGetContinuationSettings.mockResolvedValue({
      continuable_delegation_enabled: false,
    })

    renderSettings()

    expect(await screen.findByText(/main unavailable/)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  })

  it("shows a continuation read failure and blocks Save", async () => {
    mockGetDelegationSettings.mockResolvedValue(settings())
    mockGetContinuationSettings.mockRejectedValue(new Error("flag unavailable"))

    renderSettings()

    expect(await screen.findByText(/flag unavailable/)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  })

  it("persists an enabled continuation flag through the shared Save", async () => {
    const mainSettings = settings()
    mockGetDelegationSettings.mockResolvedValue(mainSettings)
    mockGetContinuationSettings.mockResolvedValue({
      continuable_delegation_enabled: false,
    })
    mockSetDelegationSettings.mockResolvedValue(mainSettings)
    mockSetContinuationSettings.mockResolvedValue({
      continuable_delegation_enabled: true,
    })

    renderSettings()

    const continuationSwitch = await screen.findByLabelText(
      "Rework rounds (experimental)"
    )
    await waitFor(() => expect(continuationSwitch).toBeEnabled())
    fireEvent.click(continuationSwitch)
    expect(mockSetContinuationSettings).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => {
      expect(mockSetContinuationSettings).toHaveBeenCalledWith({
        continuable_delegation_enabled: true,
      })
      expect(mockToast.success).toHaveBeenCalledWith(
        "Delegation settings saved"
      )
    })
    expect(mockSetDelegationSettings.mock.invocationCallOrder[0]).toBeLessThan(
      mockSetContinuationSettings.mock.invocationCallOrder[0]
    )
  })

  it("reports a continuation persist failure and does not claim success", async () => {
    const mainSettings = settings()
    mockGetDelegationSettings.mockResolvedValue(mainSettings)
    mockGetContinuationSettings.mockResolvedValue({
      continuable_delegation_enabled: false,
    })
    mockSetDelegationSettings.mockResolvedValue(mainSettings)
    mockSetContinuationSettings.mockRejectedValue(new Error("persist failed"))

    renderSettings()

    const saveButton = screen.getByRole("button", { name: "Save" })
    await waitFor(() => expect(saveButton).toBeEnabled())
    fireEvent.click(saveButton)

    await waitFor(() => {
      expect(mockToast.error).toHaveBeenCalledWith(
        "Failed to save delegation settings",
        { description: "persist failed" }
      )
    })
    expect(mockToast.success).not.toHaveBeenCalled()
  })
})
