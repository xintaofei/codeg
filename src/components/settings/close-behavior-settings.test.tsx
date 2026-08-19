import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getSystemCloseSettings: vi.fn(),
  updateSystemCloseSettings: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { error: vi.fn() },
}))

import { CloseBehaviorSettingsSection } from "./close-behavior-settings"
import enMessages from "@/i18n/messages/en.json"
import { getSystemCloseSettings, updateSystemCloseSettings } from "@/lib/api"
import { toast } from "sonner"

const mockGet = vi.mocked(getSystemCloseSettings)
const mockSet = vi.mocked(updateSystemCloseSettings)

const HIDE_LABEL = "Hide to tray (background)"
const EXIT_LABEL = "Exit application"
const TRAY_HINT = /No system tray was detected/

function renderSection() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <CloseBehaviorSettingsSection />
    </NextIntlClientProvider>
  )
}

/** The section's single control, labelled by the heading via `htmlFor`. */
function picker() {
  return screen.getByRole("combobox", { name: /close behavior/i })
}

/** Open the picker and commit one of its options. */
async function selectAction(label: string) {
  const user = userEvent.setup()
  await user.click(picker())
  await user.click(await screen.findByRole("option", { name: label }))
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe("CloseBehaviorSettingsSection", () => {
  it("shows the stored action once loaded", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray", tray_available: true })
    renderSection()

    await waitFor(() => expect(picker()).toHaveTextContent(HIDE_LABEL))
    expect(picker()).toBeEnabled()
  })

  it("persists a new choice and mirrors what the backend stored", async () => {
    mockGet.mockResolvedValue({ action: "exit", tray_available: true })
    mockSet.mockResolvedValue({ action: "hide_to_tray", tray_available: true })
    renderSection()

    await waitFor(() => expect(picker()).toHaveTextContent(EXIT_LABEL))
    await selectAction(HIDE_LABEL)

    await waitFor(() =>
      expect(mockSet).toHaveBeenCalledWith({ action: "hide_to_tray" })
    )
    await waitFor(() => expect(picker()).toHaveTextContent(HIDE_LABEL))
  })

  it("reverts to the previous action and reports when saving fails", async () => {
    mockGet.mockResolvedValue({ action: "exit", tray_available: true })
    mockSet.mockRejectedValue(new Error("boom"))
    renderSection()

    await waitFor(() => expect(picker()).toHaveTextContent(EXIT_LABEL))
    await selectAction(HIDE_LABEL)

    await waitFor(() => expect(toast.error).toHaveBeenCalled())
    expect(picker()).toHaveTextContent(EXIT_LABEL)
  })

  it("reports a failed load instead of passing the fallback off as stored", async () => {
    mockGet.mockRejectedValue(new Error("nope"))
    renderSection()

    const alert = await screen.findByRole("alert")
    expect(alert).toHaveTextContent(/Failed to load close behavior/)
    expect(alert).toHaveTextContent("nope")
  })

  it("clears a load error after a successful save", async () => {
    mockGet.mockRejectedValue(new Error("load failed"))
    mockSet.mockResolvedValue({ action: "hide_to_tray", tray_available: true })
    renderSection()

    // Load error appears
    const alert = await screen.findByRole("alert")
    expect(alert).toHaveTextContent(/Failed to load close behavior/)

    // User selects hide-to-tray (different from the fallback "exit") and saves
    await selectAction(HIDE_LABEL)

    // Wait for the save to complete
    await waitFor(() => expect(mockSet).toHaveBeenCalled())

    // Error is cleared
    await waitFor(() =>
      expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    )
  })

  it("warns about a missing tray only while hide-to-tray is selected", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray", tray_available: false })
    mockSet.mockResolvedValue({ action: "exit", tray_available: false })
    renderSection()

    expect(await screen.findByText(TRAY_HINT)).toBeInTheDocument()

    await selectAction(EXIT_LABEL)

    await waitFor(() =>
      expect(screen.queryByText(TRAY_HINT)).not.toBeInTheDocument()
    )
  })

  it("stays quiet when a tray is available", async () => {
    mockGet.mockResolvedValue({ action: "hide_to_tray", tray_available: true })
    renderSection()

    await waitFor(() => expect(picker()).toHaveTextContent(HIDE_LABEL))
    expect(screen.queryByText(TRAY_HINT)).not.toBeInTheDocument()
  })
})
