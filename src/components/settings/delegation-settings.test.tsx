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
  // Mirror the new backend default (DelegationSettings::default) so tests
  // that don't care about the toggle reflect the production wire shape.
  // Tests that need delegation active for save/depth assertions must
  // override explicitly.
  return {
    enabled: false,
    depth_limit: 1,
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
    // Depth input is disabled while `enabled` is false (the production
    // default), so this flow explicitly opts in.
    mockGetDelegationSettings.mockResolvedValue(settings({ enabled: true }))
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

  it("reflects backend default (disabled): switch off, depth input disabled", async () => {
    // Regression for the "default off" UX guarantee: when persistence has
    // never been written, the backend returns `enabled: false` and the
    // panel must surface that. Switch un-checked + depth input disabled
    // is what blocks the user from changing depth/agent-defaults before
    // they consciously opt in.
    mockGetDelegationSettings.mockResolvedValue(settings())

    renderWithIntl()

    const depthInput = (await screen.findByLabelText(
      "Maximum delegation depth"
    )) as HTMLInputElement
    const enableSwitch = screen.getByLabelText(
      "Enable delegation"
    ) as HTMLButtonElement

    expect(enableSwitch).toHaveAttribute("data-state", "unchecked")
    expect(depthInput).toBeDisabled()
  })
})
