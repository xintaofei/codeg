import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getDelegationSettings: vi.fn(),
  setDelegationSettings: vi.fn(),
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
  getDelegationSettings,
  setDelegationSettings,
  type DelegationSettings,
} from "@/lib/api"

const mockGetDelegationSettings = vi.mocked(getDelegationSettings)
const mockSetDelegationSettings = vi.mocked(setDelegationSettings)

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <DelegationSettingsSection />
    </NextIntlClientProvider>
  )
}

function settings(
  overrides: Partial<DelegationSettings> = {}
): DelegationSettings {
  return {
    enabled: true,
    depth_limit: 2,
    agent_defaults: {},
    ...overrides,
  }
}

beforeEach(() => {
  mockGetDelegationSettings.mockReset()
  mockSetDelegationSettings.mockReset()
})

describe("DelegationSettingsSection", () => {
  it("renders the enable switch and depth input", async () => {
    mockGetDelegationSettings.mockResolvedValue(settings())

    renderWithIntl()

    expect(
      await screen.findByLabelText("Maximum delegation depth")
    ).toBeInTheDocument()
    expect(screen.getByLabelText("Enable delegation")).toBeInTheDocument()
    // No timeout knob anymore — cancel flows through MCP notifications.
    expect(screen.queryByLabelText(/timeout/i)).not.toBeInTheDocument()
  })

  it("saves the depth_limit and enabled flag", async () => {
    mockGetDelegationSettings.mockResolvedValue(settings())
    mockSetDelegationSettings.mockImplementation(async (next) => next)

    renderWithIntl()

    const depthInput = await screen.findByLabelText("Maximum delegation depth")
    fireEvent.change(depthInput, { target: { value: "5" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => {
      expect(mockSetDelegationSettings).toHaveBeenCalledWith({
        enabled: true,
        depth_limit: 5,
        agent_defaults: {},
      })
    })
  })
})
