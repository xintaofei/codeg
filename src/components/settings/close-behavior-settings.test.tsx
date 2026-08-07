import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getSystemCloseSettings: vi.fn(),
  updateSystemCloseSettings: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { error: vi.fn() },
}))

import { CloseBehaviorSettings } from "./close-behavior-settings"
import enMessages from "@/i18n/messages/en.json"
import { getSystemCloseSettings, updateSystemCloseSettings } from "@/lib/api"

const mockGet = vi.mocked(getSystemCloseSettings)
const mockSet = vi.mocked(updateSystemCloseSettings)

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <CloseBehaviorSettings />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  mockGet.mockReset()
  mockSet.mockReset()
})

describe("CloseBehaviorSettings", () => {
  it("loads the backend default and selects hide-to-tray", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray" })
    renderWithIntl()
    const hide = (await screen.findByLabelText(
      "Hide to tray (background)"
    )) as HTMLInputElement
    expect(hide.checked).toBe(true)
  })

  it("switches to exit and persists the choice", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray" })
    mockSet.mockImplementation(async (next) => next)
    renderWithIntl()

    const exit = await screen.findByLabelText("Exit application")
    fireEvent.click(exit)

    await waitFor(() => {
      expect(mockSet).toHaveBeenCalledWith({ action: "exit" })
    })
    expect((exit as HTMLInputElement).checked).toBe(true)
  })

  it("reverts the radio when saving fails", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray" })
    mockSet.mockRejectedValue(new Error("boom"))
    renderWithIntl()

    const exit = await screen.findByLabelText("Exit application")
    fireEvent.click(exit)

    await waitFor(() => {
      expect(mockSet).toHaveBeenCalledWith({ action: "exit" })
    })
    const hide = screen.getByLabelText(
      "Hide to tray (background)"
    ) as HTMLInputElement
    expect(hide.checked).toBe(true)
  })
})
