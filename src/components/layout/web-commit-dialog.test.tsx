import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({ useTranslations: () => () => "Commit" }))

import { WebCommitDialog } from "./web-commit-dialog"
import {
  emitCloseCommitDialog,
  emitOpenCommitDialog,
  OPEN_COMMIT_DIALOG_EVENT,
} from "@/lib/commit-dialog-events"

afterEach(() => cleanup())

describe("WebCommitDialog", () => {
  it("opens the requested commit route in an in-page dialog", () => {
    render(<WebCommitDialog />)

    act(() => emitOpenCommitDialog("/commit?folderId=42"))

    const frame = screen.getByTitle("Commit")
    expect(frame).toHaveAttribute("src", "/commit?folderId=42")
    expect(screen.getByRole("dialog")).toBeInTheDocument()
  })

  it("closes when the embedded commit page emits its close event", () => {
    render(<WebCommitDialog />)
    act(() => emitOpenCommitDialog("/commit?folderId=42"))

    act(() => emitCloseCommitDialog())

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.queryByTitle("Commit")).not.toBeInTheDocument()
  })

  it("closes on Escape while focus is inside the commit frame", () => {
    render(<WebCommitDialog />)
    act(() => emitOpenCommitDialog("/commit?folderId=42"))

    const frame = screen.getByTitle<HTMLIFrameElement>("Commit")
    fireEvent.load(frame)
    act(() => {
      frame.contentWindow?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape" })
      )
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("ignores navigation outside the commit route", () => {
    render(<WebCommitDialog />)

    act(() => {
      window.dispatchEvent(
        new CustomEvent(OPEN_COMMIT_DIALOG_EVENT, {
          detail: { path: "/login" },
        })
      )
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })
})
