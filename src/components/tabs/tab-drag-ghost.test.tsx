import { describe, it, expect, beforeEach, afterEach, vi } from "vitest"
import { act, render, screen } from "@testing-library/react"
import { TabDragGhost } from "./tab-drag-ghost"
import { useTabStore } from "@/contexts/tab-context"

const dragTo = (overGroupId: string | null, x = 120, y = 64) =>
  act(() => {
    useTabStore.getState().updateTabDrag({
      tabId: "conv-1-codex-9",
      title: "Refactor the parser",
      x,
      y,
      overGroupId,
    })
  })

const release = () =>
  act(() => {
    useTabStore.getState().endTabDrag()
  })

describe("TabDragGhost", () => {
  beforeEach(() => {
    document.body.classList.remove("select-none")
  })

  afterEach(() => {
    release()
    vi.restoreAllMocks()
  })

  it("shows the floating chip only while hovering a FOREIGN group", () => {
    render(<TabDragGhost />)
    expect(screen.queryByText("Refactor the parser")).toBeNull()

    // Dragging over the tab's own strip: no chip (the real tab is visible).
    dragTo(null)
    expect(screen.queryByText("Refactor the parser")).toBeNull()

    dragTo("g-2")
    expect(screen.getByText("Refactor the parser")).toBeTruthy()

    dragTo(null)
    expect(screen.queryByText("Refactor the parser")).toBeNull()
  })

  it("suppresses document-wide text selection for the whole drag, not just while the chip shows", () => {
    render(<TabDragGhost />)
    expect(document.body.classList.contains("select-none")).toBe(false)

    dragTo(null)
    expect(document.body.classList.contains("select-none")).toBe(true)
    dragTo("g-2")
    expect(document.body.classList.contains("select-none")).toBe(true)

    release()
    expect(document.body.classList.contains("select-none")).toBe(false)
  })

  it("clears any selection smear that slipped in before suppression kicked in", () => {
    const removeAllRanges = vi.fn()
    vi.spyOn(window, "getSelection").mockReturnValue({
      removeAllRanges,
    } as unknown as Selection)

    render(<TabDragGhost />)
    dragTo(null)
    expect(removeAllRanges).not.toHaveBeenCalled()

    release()
    expect(removeAllRanges).toHaveBeenCalledTimes(1)
  })
})
