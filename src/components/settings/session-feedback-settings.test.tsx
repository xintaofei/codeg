import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getFeedbackSettings: vi.fn(),
  setFeedbackSettings: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

// Avoid mutating the shared module cache across tests.
vi.mock("@/hooks/use-feedback-enabled", () => ({
  primeFeedbackEnabled: vi.fn(),
}))

import { SessionFeedbackSettingsSection } from "./session-feedback-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  getFeedbackSettings,
  setFeedbackSettings,
  type FeedbackSettings,
} from "@/lib/api"
import { primeFeedbackEnabled } from "@/hooks/use-feedback-enabled"

const mockGet = vi.mocked(getFeedbackSettings)
const mockSet = vi.mocked(setFeedbackSettings)
const mockPrime = vi.mocked(primeFeedbackEnabled)

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SessionFeedbackSettingsSection />
    </NextIntlClientProvider>
  )
}

function settings(overrides: Partial<FeedbackSettings> = {}): FeedbackSettings {
  return { enabled: false, ...overrides }
}

beforeEach(() => {
  mockGet.mockReset()
  mockSet.mockReset()
  mockPrime.mockReset()
})

describe("SessionFeedbackSettingsSection", () => {
  it("reflects the backend default (disabled): switch off", async () => {
    mockGet.mockResolvedValue(settings())
    renderWithIntl()
    const sw = (await screen.findByLabelText(
      "Enable live feedback"
    )) as HTMLButtonElement
    expect(sw).toHaveAttribute("data-state", "unchecked")
  })

  it("saves the enabled flag and primes the cache", async () => {
    mockGet.mockResolvedValue(settings())
    mockSet.mockImplementation(async (next) => next)
    renderWithIntl()

    const sw = await screen.findByLabelText("Enable live feedback")
    fireEvent.click(sw) // flip to enabled
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() => {
      expect(mockSet).toHaveBeenCalledWith({ enabled: true })
    })
    // The module cache is primed so open conversations pick up the change.
    expect(mockPrime).toHaveBeenCalledWith(true)
  })
})
