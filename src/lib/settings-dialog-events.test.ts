import { describe, expect, it, vi } from "vitest"
import {
  emitOpenSettingsDialog,
  OPEN_SETTINGS_DIALOG_EVENT,
  type OpenSettingsDialogDetail,
} from "./settings-dialog-events"

describe("emitOpenSettingsDialog", () => {
  it("emits the settings path for the top-level dialog host", () => {
    const listener =
      vi.fn<(event: CustomEvent<OpenSettingsDialogDetail>) => void>()
    window.addEventListener(
      OPEN_SETTINGS_DIALOG_EVENT,
      listener as EventListener,
      { once: true }
    )

    emitOpenSettingsDialog("/settings/mcp")

    expect(listener).toHaveBeenCalledOnce()
    expect(listener.mock.calls[0][0].detail).toEqual({
      path: "/settings/mcp",
    })
  })
})
