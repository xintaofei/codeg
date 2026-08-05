import { describe, expect, it, vi } from "vitest"
import {
  CLOSE_COMMIT_DIALOG_EVENT,
  emitCloseCommitDialog,
  emitOpenCommitDialog,
  OPEN_COMMIT_DIALOG_EVENT,
  type OpenCommitDialogDetail,
} from "./commit-dialog-events"

describe("commit dialog events", () => {
  it("emits the commit path for the top-level dialog host", () => {
    const listener =
      vi.fn<(event: CustomEvent<OpenCommitDialogDetail>) => void>()
    window.addEventListener(
      OPEN_COMMIT_DIALOG_EVENT,
      listener as EventListener,
      {
        once: true,
      }
    )

    emitOpenCommitDialog("/commit?folderId=7")

    expect(listener).toHaveBeenCalledOnce()
    expect(listener.mock.calls[0][0].detail).toEqual({
      path: "/commit?folderId=7",
    })
  })

  it("emits a close request for the top-level dialog host", () => {
    const listener = vi.fn()
    window.addEventListener(CLOSE_COMMIT_DIALOG_EVENT, listener, { once: true })

    emitCloseCommitDialog()

    expect(listener).toHaveBeenCalledOnce()
  })
})
