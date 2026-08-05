export const OPEN_SETTINGS_DIALOG_EVENT = "codeg:open-settings-dialog"

export interface OpenSettingsDialogDetail {
  path: string
}

/**
 * Ask the top-level Codeg page to show settings in its modal. Settings are
 * rendered in a same-origin iframe, so events emitted from inside that frame
 * must target the top window to avoid opening a nested dialog.
 */
export function emitOpenSettingsDialog(path: string): void {
  if (typeof window === "undefined") return

  let target: Window = window
  try {
    if (window.top?.location.origin === window.location.origin) {
      target = window.top
    }
  } catch {
    // A cross-origin embedding page is not allowed to receive this event.
  }

  target.dispatchEvent(
    new CustomEvent<OpenSettingsDialogDetail>(OPEN_SETTINGS_DIALOG_EVENT, {
      detail: { path },
    })
  )
}
