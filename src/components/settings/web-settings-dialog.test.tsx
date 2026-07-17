import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({ useTranslations: () => () => "Settings" }))

import { WebSettingsDialog } from "./web-settings-dialog"
import {
  emitOpenSettingsDialog,
  OPEN_SETTINGS_DIALOG_EVENT,
} from "@/lib/settings-dialog-events"

afterEach(() => cleanup())

describe("WebSettingsDialog", () => {
  it("opens the requested settings route in an in-page dialog", () => {
    render(<WebSettingsDialog />)

    act(() => emitOpenSettingsDialog("/settings/agents?agent=codex"))

    const frame = screen.getByTitle("Settings")
    expect(frame).toHaveAttribute("src", "/settings/agents?agent=codex")
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })

  it("closes and removes the settings frame", () => {
    render(<WebSettingsDialog />)
    act(() => emitOpenSettingsDialog("/settings/appearance"))

    fireEvent.click(screen.getByRole("button", { name: "Close" }))

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.queryByTitle("Settings")).not.toBeInTheDocument()
  })

  it("closes on Escape while focus is inside the settings frame", () => {
    render(<WebSettingsDialog />)
    act(() => emitOpenSettingsDialog("/settings/appearance"))

    const frame = screen.getByTitle<HTMLIFrameElement>("Settings")
    fireEvent.load(frame)
    act(() => {
      frame.contentWindow?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape" })
      )
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("ignores navigation outside the settings routes", () => {
    render(<WebSettingsDialog />)

    act(() => {
      window.dispatchEvent(
        new CustomEvent(OPEN_SETTINGS_DIALOG_EVENT, {
          detail: { path: "/login" },
        })
      )
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })
})
