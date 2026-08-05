export const OPEN_COMMIT_DIALOG_EVENT = "codeg:open-commit-dialog"
export const CLOSE_COMMIT_DIALOG_EVENT = "codeg:close-commit-dialog"

export interface OpenCommitDialogDetail {
  path: string
}

function getDialogEventTarget(): Window | null {
  if (typeof window === "undefined") return null

  try {
    if (window.top?.location.origin === window.location.origin) {
      return window.top
    }
  } catch {
    // A cross-origin embedding page is not allowed to receive these events.
  }

  return window
}

export function emitOpenCommitDialog(path: string): void {
  getDialogEventTarget()?.dispatchEvent(
    new CustomEvent<OpenCommitDialogDetail>(OPEN_COMMIT_DIALOG_EVENT, {
      detail: { path },
    })
  )
}

export function emitCloseCommitDialog(): void {
  getDialogEventTarget()?.dispatchEvent(new Event(CLOSE_COMMIT_DIALOG_EVENT))
}
