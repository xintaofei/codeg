import { useRef } from "react"
import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const toastSuccess = vi.fn()
const toastError = vi.fn()
vi.mock("sonner", () => ({
  toast: {
    success: (m: string) => toastSuccess(m),
    error: (m: string) => toastError(m),
  },
}))

import { SelectionActionBubble } from "./selection-action-bubble"
import enMessages from "@/i18n/messages/en.json"

// The container's box. jsdom does no layout, so every rect the component reads
// is stubbed: the container via Element.prototype, the selection via the fake
// Range below.
const BOX = {
  top: 100,
  bottom: 600,
  left: 0,
  right: 400,
  width: 400,
  height: 500,
  x: 0,
  y: 100,
} as DOMRect

/** A selection rect 100px down from the container top — room for the bubble above. */
const SELECTION_RECT = {
  top: 200,
  bottom: 220,
  left: 100,
  right: 180,
  width: 80,
  height: 20,
  x: 100,
  y: 200,
} as DOMRect

const removeAllRanges = vi.fn()
const addRange = vi.fn()

/**
 * A stateful stand-in for the page selection, spied onto BOTH `window` and
 * `document` (the clipboard fallback in `lib/utils` reaches for
 * `document.getSelection`).
 *
 * Statefulness matters: `removeAllRanges` really empties it and `addRange`
 * really refills it, which is what lets a test tell "cleared and left cleared"
 * apart from "cleared, then handed back by the clipboard fallback's restore".
 */
function mockSelection(
  container: Node | null,
  text: string,
  rect: DOMRect = SELECTION_RECT
) {
  let cleared = false
  const range = {
    commonAncestorContainer: container,
    getBoundingClientRect: () => rect,
    cloneRange: () => range,
  }
  const selection = {
    get isCollapsed() {
      return cleared || text.length === 0
    },
    get rangeCount() {
      return cleared || !container ? 0 : 1
    },
    getRangeAt: () => range,
    toString: () => (cleared ? "" : text),
    removeAllRanges: () => {
      cleared = true
      removeAllRanges()
    },
    addRange: () => {
      cleared = false
      addRange()
    },
  } as unknown as Selection
  vi.spyOn(window, "getSelection").mockReturnValue(selection)
  vi.spyOn(document, "getSelection").mockReturnValue(selection)
}

/** Fire the browser event the component listens on, inside `act`. */
function selectionChanged() {
  act(() => {
    fireEvent(document, new Event("selectionchange"))
  })
}

/**
 * Dispatch a pointer event carrying a real `button`. jsdom has no
 * `PointerEvent`, so RTL's `fireEvent.pointerDown` falls back to a bare `Event`
 * with no `button` property at all — which can't distinguish a left-click from
 * a right-click. `MouseEvent` implements `button` properly, and Blink dispatches
 * pointer events as a `PointerEvent` (a `MouseEvent` subclass), so this is the
 * faithful shape for the button-sensitive cases.
 */
function firePointer(
  type: "pointerdown" | "pointerup" | "pointercancel",
  target: Element,
  init: MouseEventInit = {}
) {
  act(() => {
    target.dispatchEvent(
      new MouseEvent(type, { bubbles: true, cancelable: true, ...init })
    )
  })
}

function Harness({ onQuote }: { onQuote?: (text: string) => void }) {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <div ref={ref} data-testid="box">
        <p data-testid="para">hello world</p>
        <SelectionActionBubble containerRef={ref} onQuote={onQuote} />
      </div>
    </NextIntlClientProvider>
  )
}

let rectSpy: ReturnType<typeof vi.spyOn>

/**
 * jsdom lays nothing out, so the toolbar's own width — which the edge clamp
 * depends on — reads 0. Stub it, and return a restore fn.
 */
function mockToolbarWidth(width: number) {
  const original = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetWidth"
  )
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => width,
  })
  return () => {
    if (original) {
      Object.defineProperty(HTMLElement.prototype, "offsetWidth", original)
    } else {
      delete (HTMLElement.prototype as unknown as Record<string, unknown>)
        .offsetWidth
    }
  }
}

beforeEach(() => {
  removeAllRanges.mockClear()
  addRange.mockClear()
  toastSuccess.mockClear()
  toastError.mockClear()
  rectSpy = vi
    .spyOn(Element.prototype, "getBoundingClientRect")
    .mockReturnValue(BOX)
})

afterEach(() => {
  rectSpy.mockRestore()
  vi.restoreAllMocks()
})

describe("SelectionActionBubble", () => {
  it("renders nothing without a selection", () => {
    mockSelection(null, "")
    render(<Harness onQuote={vi.fn()} />)
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("renders nothing for a whitespace-only selection", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "  \n ")
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("ignores a selection made outside the container", () => {
    const outside = document.createElement("div")
    document.body.appendChild(outside)
    render(<Harness onQuote={vi.fn()} />)
    mockSelection(outside, "elsewhere")
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()
    outside.remove()
  })

  it("shows copy and quote above the selection", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    const toolbar = screen.getByRole("toolbar")
    expect(screen.getByRole("button", { name: "Copy Text" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Quote" })).toBeTruthy()
    // Centred on the selection (140 = 100 + 80/2), pinned 8px above its top
    // (92 = 200 - 100 - 8) and pulled fully above by the transform.
    expect(toolbar.style.left).toBe("140px")
    expect(toolbar.style.top).toBe("92px")
    expect(toolbar.style.transform).toBe("translate(-50%, -100%)")
  })

  it("flips below a selection with no room above it", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello", {
      ...SELECTION_RECT,
      top: 110,
      bottom: 130,
      y: 110,
    } as DOMRect)
    selectionChanged()

    const toolbar = screen.getByRole("toolbar")
    expect(toolbar.style.top).toBe("38px") // 130 - 100 + 8
    expect(toolbar.style.transform).toBe("translate(-50%, 0)")
  })

  it("hides while the selection is scrolled out of the message area", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello", {
      ...SELECTION_RECT,
      top: 20,
      bottom: 40,
      y: 20,
    } as DOMRect)
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("omits the quote action when no quote handler is given", () => {
    const { container } = render(<Harness />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    expect(screen.getByRole("button", { name: "Copy Text" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Quote" })).toBeNull()
  })

  it("quotes the raw selected text, then clears the selection and hides", () => {
    const onQuote = vi.fn()
    const { container } = render(<Harness onQuote={onQuote} />)
    mockSelection(
      container.querySelector("[data-testid=para]"),
      "first line\nsecond line"
    )
    selectionChanged()

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Quote" }))
    })

    // The bubble hands over the selection verbatim — turning it into Markdown is
    // the host's job (buildQuotedMarkdown).
    expect(onQuote).toHaveBeenCalledWith("first line\nsecond line")
    expect(removeAllRanges).toHaveBeenCalled()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("copies the selected text, then clears the selection, hides and toasts", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    })
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello world")
    selectionChanged()

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy Text" }))
    })

    expect(writeText).toHaveBeenCalledWith("hello world")
    // The toolbar is gone, so the confirmation has to be a toast — there is no
    // button left to turn into a checkmark.
    expect(removeAllRanges).toHaveBeenCalled()
    expect(screen.queryByRole("toolbar")).toBeNull()
    expect(toastSuccess).toHaveBeenCalledWith("Copied")
    expect(toastError).not.toHaveBeenCalled()
  })

  it("toasts an error when the clipboard write fails", async () => {
    // Non-secure contexts (the web build over plain HTTP) have no
    // navigator.clipboard, and the legacy execCommand path can refuse too.
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("denied")) },
    })
    const execCommand = vi.fn().mockReturnValue(false)
    Object.defineProperty(document, "execCommand", {
      configurable: true,
      value: execCommand,
    })
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello world")
    selectionChanged()

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy Text" }))
    })

    expect(toastError).toHaveBeenCalledWith("Copy failed")
    expect(toastSuccess).not.toHaveBeenCalled()
    expect(screen.queryByRole("toolbar")).toBeNull()

    // This write went down the legacy path, which snapshots the page selection
    // around its hidden textarea and restores it afterwards. Because the bubble
    // clears the selection BEFORE starting the write, that snapshot is empty and
    // no restore is attempted at all — so nothing can hand the selection, and
    // with it the bubble, back.
    expect(execCommand).toHaveBeenCalledWith("copy")
    expect(addRange).not.toHaveBeenCalled()
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("survives a tap that clears the selection before the click lands", () => {
    // The touch path: no mousedown to preventDefault, so the tap itself drops
    // the selection. Tearing the bubble down on that selectionchange would
    // unmount the button before its click is dispatched.
    const onQuote = vi.fn()
    const { container } = render(<Harness onQuote={onQuote} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    const quote = screen.getByRole("button", { name: "Quote" })
    act(() => {
      fireEvent.pointerDown(quote)
    })
    mockSelection(null, "")
    selectionChanged()
    expect(screen.queryByRole("toolbar")).not.toBeNull()

    act(() => {
      fireEvent.pointerUp(quote)
      fireEvent.click(quote)
    })
    expect(onQuote).toHaveBeenCalledWith("hello")
  })

  it("keeps its whole box inside the container near the edges", () => {
    // The toolbar is centred on `x` by a -50% transform, so clamping the anchor
    // alone lets half of it hang past the edge, where the panel's
    // overflow-hidden shears it off (measured in Chrome: 35px of a button gone
    // in a 300px-wide tiled column).
    const restore = mockToolbarWidth(160) // half = 80, so x must stay in [88, 312]
    try {
      const { container } = render(<Harness onQuote={vi.fn()} />)
      const para = container.querySelector("[data-testid=para]")

      // Hard against the left edge of the 400px-wide container.
      mockSelection(para, "hello", {
        ...SELECTION_RECT,
        left: 0,
        right: 40,
        width: 40,
        x: 0,
      } as DOMRect)
      selectionChanged()
      // The first measure runs before the toolbar exists (offsetWidth 0), so it
      // takes a second pass — the frame loop's — to settle on the clamped value.
      selectionChanged()
      expect(screen.getByRole("toolbar").style.left).toBe("88px")

      // ...and against the right edge.
      mockSelection(para, "hello", {
        ...SELECTION_RECT,
        left: 360,
        right: 400,
        width: 40,
        x: 360,
      } as DOMRect)
      selectionChanged()
      expect(screen.getByRole("toolbar").style.left).toBe("312px")
    } finally {
      restore()
    }
  })

  it("centres a toolbar too wide to fit rather than clamping it off-screen", () => {
    const restore = mockToolbarWidth(900) // wider than the 400px container
    try {
      const { container } = render(<Harness onQuote={vi.fn()} />)
      mockSelection(container.querySelector("[data-testid=para]"), "hello")
      selectionChanged()
      selectionChanged()
      expect(screen.getByRole("toolbar").style.left).toBe("200px")
    } finally {
      restore()
    }
  })

  it.each([
    ["a cancelled press", "pointerCancel" as const, 0],
    ["a right-click", "pointerDown" as const, 2],
  ])("does not freeze tracking after %s inside it", (_label, kind, button) => {
    // Neither a pointercancel nor a right-click is followed by a `click`, so the
    // press-inside guard has nothing to release it unless these paths clear it
    // themselves — the bubble would stay stuck at its old coordinates.
    const { container } = render(<Harness onQuote={vi.fn()} />)
    const para = container.querySelector("[data-testid=para]")
    mockSelection(para, "hello")
    selectionChanged()
    const quote = screen.getByRole("button", { name: "Quote" })
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    if (kind === "pointerCancel") {
      firePointer("pointerdown", quote)
      firePointer("pointercancel", quote)
    } else {
      firePointer("pointerdown", quote, { button })
      firePointer("pointerup", quote, { button })
    }

    // Same selection, new geometry — as after a scroll.
    mockSelection(para, "hello", {
      ...SELECTION_RECT,
      top: 260,
      bottom: 280,
      y: 260,
    } as DOMRect)
    selectionChanged()
    expect(screen.getByRole("toolbar").style.top).toBe("152px")
  })

  it("keeps tracking the selection after a press on its chrome", async () => {
    // Regression (caught in a real browser): the tap guard above stayed armed
    // once the click had landed, so the bubble stopped following the text —
    // scrolling the thread left it stranded at its original coordinates.
    //
    // Pressing the toolbar's own padding rather than a button is what still
    // exercises this: both actions dismiss the bubble and clear the guard
    // themselves, so the shared `click` release only matters for a press that
    // runs no action at all.
    const { container } = render(<Harness onQuote={vi.fn()} />)
    const para = container.querySelector("[data-testid=para]")
    mockSelection(para, "hello")
    selectionChanged()
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    const toolbar = screen.getByRole("toolbar")
    await act(async () => {
      fireEvent.pointerDown(toolbar)
      fireEvent.pointerUp(toolbar)
      fireEvent.click(toolbar)
    })

    // Same selection, new geometry — as after a scroll.
    mockSelection(para, "hello", {
      ...SELECTION_RECT,
      top: 260,
      bottom: 280,
      y: 260,
    } as DOMRect)
    selectionChanged()
    expect(screen.getByRole("toolbar").style.top).toBe("152px") // 260 - 100 - 8
  })

  it("stays hidden while a drag is in flight and appears on release", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    const para = container.querySelector("[data-testid=para]")

    act(() => {
      fireEvent.pointerDown(document)
    })
    mockSelection(para, "hello")
    selectionChanged()
    expect(screen.queryByRole("toolbar")).toBeNull()

    // pointerup re-reads the finalised selection on the next task.
    vi.useFakeTimers()
    try {
      act(() => {
        fireEvent.pointerUp(document)
        vi.runAllTimers()
      })
    } finally {
      vi.useRealTimers()
    }
    expect(screen.getByRole("toolbar")).toBeTruthy()
  })
})
