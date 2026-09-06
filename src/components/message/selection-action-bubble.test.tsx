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

// FE-5 owns the message files; the bubble's tests only need the selection keys,
// so the baseline is overlaid with whatever exists under `Folder.chat.messageList`
// plus the keys this slice renders. Pointing at `enMessages` directly would make
// every new FE-5 key a compile error here until it lands.
const messages = {
  ...enMessages,
  Folder: {
    ...enMessages.Folder,
    chat: {
      ...enMessages.Folder.chat,
      messageList: {
        ...enMessages.Folder.chat.messageList,
        selectionTranslate: "Translate",
        selectionTranslating: "Translating…",
        selectionTranslateFailed: "Translation failed",
        selectionTranslateTruncated: "Selection cut at {limit} characters",
        selectionTranslateOriginal: "Original",
        selectionTranslateClose: "Close translation",
      },
    },
  },
}

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
  type: "pointerdown" | "pointerup" | "pointercancel" | "pointermove",
  target: Element,
  init: MouseEventInit = {}
) {
  act(() => {
    target.dispatchEvent(
      new MouseEvent(type, { bubbles: true, cancelable: true, ...init })
    )
  })
}

function Harness({
  onQuote,
  onAsk,
  onTranslate,
}: {
  onQuote?: (text: string) => void
  onAsk?: (selection: string, question: string) => void
  onTranslate?: (
    text: string
  ) => Promise<{ text: string | null; error?: string } | null>
}) {
  const ref = useRef<HTMLDivElement>(null)
  return (
    <NextIntlClientProvider locale="en" messages={messages}>
      <div ref={ref} data-testid="box">
        <p data-testid="para">hello world</p>
        <SelectionActionBubble
          containerRef={ref}
          onQuote={onQuote}
          onAsk={onAsk}
          onTranslate={onTranslate}
        />
      </div>
    </NextIntlClientProvider>
  )
}

/** Select `text`, open the ask composer, and hand back its input. */
function openAskComposer(container: HTMLElement, text = "hello") {
  mockSelection(container.querySelector("[data-testid=para]"), text)
  selectionChanged()
  act(() => {
    fireEvent.click(screen.getByRole("button", { name: "Ask" }))
  })
  return screen.getByRole("textbox", { name: "Ask about this selection…" })
}

/** Select `text`, open the translation card, and hand back the toolbar. */
function openTranslateCard(container: HTMLElement, text = "hello") {
  mockSelection(container.querySelector("[data-testid=para]"), text)
  selectionChanged()
  act(() => {
    fireEvent.click(screen.getByRole("button", { name: "Translate" }))
  })
  return screen.getByRole("toolbar")
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

/**
 * Like {@link mockToolbarWidth}, but the toolbar reports a DIFFERENT width for
 * each face it can show — which is the whole reason the clamp has to be
 * recomputed when one opens. The face is read off the element's own content, so
 * no test has to sequence the widths by hand: the ask composer owns the input,
 * and the translation card grows again once the result (the only selectable
 * text in the bubble) replaces its spinner.
 */
function mockToolbarWidthByMode(
  buttonsWidth: number,
  askWidth: number,
  card?: { loading: number; done: number }
) {
  const original = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetWidth"
  )
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get(this: HTMLElement) {
      if (this.querySelector("input")) return askWidth
      if (card && this.querySelector("[role=status], [data-selectable]")) {
        return this.querySelector("[data-selectable]")
          ? card.done
          : card.loading
      }
      return buttonsWidth
    },
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

  it("omits the ask action when no ask handler is given", () => {
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    expect(screen.queryByRole("button", { name: "Ask" })).toBeNull()
  })

  it("swaps the buttons for a question box and asks with the selection", () => {
    const onAsk = vi.fn()
    const { container } = render(<Harness onQuote={vi.fn()} onAsk={onAsk} />)
    const input = openAskComposer(container, "first line\nsecond line")

    // The composer replaces the actions — the toolbar can't do both at once.
    expect(screen.queryByRole("button", { name: "Copy Text" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Quote" })).toBeNull()

    act(() => {
      fireEvent.change(input, { target: { value: "  what does this mean?  " } })
      fireEvent.keyDown(input, { key: "Enter" })
    })

    // Selection verbatim (the host quotes it), question trimmed.
    expect(onAsk).toHaveBeenCalledWith(
      "first line\nsecond line",
      "what does this mean?"
    )
    expect(removeAllRanges).toHaveBeenCalled()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("asks when the submit button is clicked", () => {
    const onAsk = vi.fn()
    const { container } = render(<Harness onAsk={onAsk} />)
    const input = openAskComposer(container)

    act(() => {
      fireEvent.change(input, { target: { value: "why?" } })
      fireEvent.click(
        screen.getByRole("button", { name: "Ask in a new conversation" })
      )
    })

    expect(onAsk).toHaveBeenCalledWith("hello", "why?")
  })

  it("does not ask on an empty or whitespace-only question", () => {
    const onAsk = vi.fn()
    const { container } = render(<Harness onAsk={onAsk} />)
    const input = openAskComposer(container)

    act(() => {
      fireEvent.keyDown(input, { key: "Enter" })
      fireEvent.change(input, { target: { value: "   " } })
      fireEvent.keyDown(input, { key: "Enter" })
    })

    expect(onAsk).not.toHaveBeenCalled()
    // The composer stays up rather than dismissing — there is nothing to send,
    // so the press simply hasn't done anything yet.
    expect(screen.queryByRole("textbox")).not.toBeNull()
    expect(
      screen.getByRole("button", { name: "Ask in a new conversation" })
    ).toHaveProperty("disabled", true)
  })

  it("ignores Enter while an IME candidate is being composed", () => {
    // Every CJK input method commits its candidate with Enter. Submitting on
    // that would send a half-typed question.
    const onAsk = vi.fn()
    const { container } = render(<Harness onAsk={onAsk} />)
    const input = openAskComposer(container)

    act(() => {
      fireEvent.change(input, { target: { value: "这是" } })
      fireEvent.compositionStart(input)
      fireEvent.keyDown(input, { key: "Enter" })
    })
    expect(onAsk).not.toHaveBeenCalled()

    act(() => {
      fireEvent.compositionEnd(input)
      fireEvent.keyDown(input, { key: "Enter" })
    })
    expect(onAsk).toHaveBeenCalledWith("hello", "这是")
  })

  it("keeps the question box up after focusing it drops the page selection", () => {
    // Focusing the input collapses the page selection, and the frame loop /
    // selectionchange tracker would measure nothing and unmount the box the user
    // is typing into. Opening the composer freezes both.
    const onAsk = vi.fn()
    const { container } = render(<Harness onAsk={onAsk} />)
    const input = openAskComposer(container)

    mockSelection(null, "")
    selectionChanged()
    expect(screen.queryByRole("textbox")).not.toBeNull()
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    act(() => {
      fireEvent.change(input, { target: { value: "still here?" } })
      fireEvent.keyDown(input, { key: "Enter" })
    })
    // The text was captured when the composer opened, so it survives the
    // selection going away underneath.
    expect(onAsk).toHaveBeenCalledWith("hello", "still here?")
  })

  it.each([
    [
      "Escape",
      (input: HTMLElement) => {
        act(() => {
          fireEvent.keyDown(input, { key: "Escape" })
        })
      },
    ],
    [
      "a press outside",
      () => {
        act(() => {
          fireEvent.pointerDown(document.body)
        })
      },
    ],
  ])("abandons the question on %s", (_label, cancel) => {
    const onAsk = vi.fn()
    const { container } = render(<Harness onAsk={onAsk} />)
    const input = openAskComposer(container)
    act(() => {
      fireEvent.change(input, { target: { value: "never mind" } })
    })

    cancel(input)

    expect(onAsk).not.toHaveBeenCalled()
    expect(screen.queryByRole("toolbar")).toBeNull()
  })

  it("opens a fresh question box after one was abandoned", () => {
    // The abandoned text must not come back with the next selection — the
    // composer is per-question, not a persistent draft.
    const { container } = render(<Harness onAsk={vi.fn()} />)
    const input = openAskComposer(container)
    act(() => {
      fireEvent.change(input, { target: { value: "never mind" } })
      fireEvent.keyDown(input, { key: "Escape" })
    })

    expect(openAskComposer(container)).toHaveProperty("value", "")
  })

  it("re-clamps for the wider question box before it freezes", () => {
    // The composer is much wider than the button row, and the frame loop is
    // frozen from the moment it opens — so opening it is the last chance to keep
    // it off the container edge, where overflow-hidden would shear it.
    //
    // 160 wide → half 80 → x clamped into [88, 312];
    // 300 wide → half 150 → x clamped into [158, 242].
    const restore = mockToolbarWidthByMode(160, 300)
    try {
      const { container } = render(<Harness onAsk={vi.fn()} />)
      // Hard against the container's left edge, so both clamps actually bite.
      mockSelection(container.querySelector("[data-testid=para]"), "hello", {
        ...SELECTION_RECT,
        left: 0,
        right: 40,
        width: 40,
        x: 0,
      } as DOMRect)
      selectionChanged()
      // The first measure runs before the toolbar exists (offsetWidth 0), so the
      // clamped value settles on the second pass.
      selectionChanged()
      expect(screen.getByRole("toolbar").style.left).toBe("88px")

      act(() => {
        fireEvent.click(screen.getByRole("button", { name: "Ask" }))
      })
      expect(screen.getByRole("toolbar").style.left).toBe("158px")
    } finally {
      restore()
    }
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

  it("drops the dragged-card offset when a new selection is made", () => {
    // Drag the translation card away, close it, select again: the fresh
    // button row must sit at the NEW selection, not stay pinned to wherever
    // the previous card was dragged. The offset is visual state of one card,
    // not of the bubble.
    const onTranslate = vi.fn().mockResolvedValue({ text: "hola" })
    const { container } = render(
      <Harness onQuote={vi.fn()} onTranslate={onTranslate} />
    )
    const restoreWidth = mockToolbarWidth(100)

    // Open the card and drag it by the header.
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Translate" }))
    })
    const handle = screen
      .getByRole("toolbar")
      .querySelector("[data-drag-handle]") as HTMLElement
    firePointer("pointerdown", handle, {
      button: 0,
      clientX: 100,
      clientY: 100,
    })
    firePointer("pointermove", handle, { clientX: 400, clientY: 500 })
    firePointer("pointerup", handle)
    const dragged = screen.getByRole("toolbar")
    const draggedLeft = dragged.style.left
    expect(Number(draggedLeft.replace("px", ""))).toBeGreaterThan(300)

    // Dismiss the card, then make a NEW selection: the button row's left
    // must be back at the selection's centre (offset zeroed).
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Close translation" }))
    })
    mockSelection(container.querySelector("[data-testid=para]"), "world")
    selectionChanged()
    const fresh = screen.getByRole("toolbar")
    expect(fresh.style.left).toBe("140px")
    expect(fresh.style.top).toBe("92px")

    restoreWidth()
  })

  it("omits the translate action when no translate handler is given", () => {
    // Translation off in settings means the host passes no handler at all.
    const { container } = render(<Harness onQuote={vi.fn()} />)
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    expect(screen.getByRole("button", { name: "Copy Text" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Translate" })).toBeNull()
  })

  it("places translate as the toolbar's rightmost action", () => {
    const { container } = render(
      <Harness onQuote={vi.fn()} onAsk={vi.fn()} onTranslate={vi.fn()} />
    )
    mockSelection(container.querySelector("[data-testid=para]"), "hello")
    selectionChanged()

    const names = Array.from(
      screen.getByRole("toolbar").querySelectorAll("button")
    ).map((button) => button.textContent?.trim())
    expect(names[names.length - 1]).toBe("Translate")
  })

  it("swaps the buttons for a card and keeps the selection while translating", async () => {
    const onTranslate = vi.fn().mockResolvedValue({ text: "hola" })
    const { container } = render(
      <Harness onQuote={vi.fn()} onTranslate={onTranslate} />
    )
    const toolbar = openTranslateCard(container, "hello")

    // The card replaces the actions — the toolbar can't do both at once — and
    // reports progress while the request is out.
    expect(screen.queryByRole("button", { name: "Copy Text" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Quote" })).toBeNull()
    expect(screen.getByRole("status", { name: "Translating…" })).toBeTruthy()

    await act(async () => {})

    expect(onTranslate).toHaveBeenCalledWith("hello")
    expect(screen.getByText("hola")).toBeTruthy()
    // Unlike every other action, translating does NOT dismiss: the result is
    // the toolbar, and the selection it belongs to stays put underneath it.
    expect(removeAllRanges).not.toHaveBeenCalled()
    expect(toolbar.isConnected).toBe(true)
    expect(screen.getByText("Original")).toBeTruthy()
  })

  it.each([
    ["resolves null", () => vi.fn().mockResolvedValue(null)],
    ["rejects", () => vi.fn().mockRejectedValue(new Error("offline"))],
  ])("reports the failure inline when the handler %s", async (_l, make) => {
    const { container } = render(<Harness onTranslate={make()} />)
    openTranslateCard(container)

    await act(async () => {})

    // Inline, not a toast: unlike copy, the surface that would show the
    // confirmation is still on screen.
    expect(screen.getByText("Translation failed")).toBeTruthy()
    expect(toastError).not.toHaveBeenCalled()
    expect(screen.queryByRole("status")).toBeNull()
    expect(screen.getByRole("toolbar")).toBeTruthy()
  })

  it("shows the failure reason and offers a retry on the card", async () => {
    const onTranslate = vi
      .fn()
      .mockResolvedValueOnce({
        text: null,
        error: "The translation request failed (NetworkError)",
      })
      .mockResolvedValueOnce({ text: "hola" })
    const { container } = render(<Harness onTranslate={onTranslate} />)
    openTranslateCard(container)

    await act(async () => {})

    // The endpoint's own reason shows below the failure line...
    expect(
      screen.getByText("The translation request failed (NetworkError)")
    ).toBeTruthy()
    // ...and a retry re-runs the SAME original text.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    })
    expect(onTranslate).toHaveBeenCalledTimes(2)
    expect(onTranslate).toHaveBeenLastCalledWith("hello")
    await act(async () => {})
    expect(screen.getByText("hola")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull()
  })

  it("maps a gate code to its message on the failure card", async () => {
    const { container } = render(
      <Harness
        onTranslate={vi
          .fn()
          .mockResolvedValue({ text: null, error: "ECHO_OR_REFUSAL" })}
      />
    )
    openTranslateCard(container)

    await act(async () => {})

    expect(
      screen.getByText("The endpoint echoed the source or refused the request.")
    ).toBeTruthy()
  })

  it("cuts an overlong selection to the cap and says it did", async () => {
    const onTranslate = vi.fn().mockResolvedValue({ text: "translated" })
    const { container } = render(<Harness onTranslate={onTranslate} />)
    openTranslateCard(container, "a".repeat(2500))

    await act(async () => {})

    // The handler receives the cut text, never the full selection...
    expect(onTranslate).toHaveBeenCalledWith("a".repeat(2000))
    // ...and the user is told, so a short translation of a long selection
    // doesn't read as a broken one.
    expect(screen.getByRole("toolbar").textContent).toMatch(/2,?000/)
    expect(screen.getByText("translated")).toBeTruthy()
  })

  it.each([
    [
      "Escape",
      () => {
        act(() => {
          fireEvent.keyDown(document.body, { key: "Escape" })
        })
      },
    ],
    [
      "a press outside",
      () => {
        act(() => {
          fireEvent.pointerDown(document.body)
        })
      },
    ],
    [
      "the card's close button",
      () => {
        act(() => {
          fireEvent.click(
            screen.getByRole("button", { name: "Close translation" })
          )
        })
      },
    ],
  ])("closes the translation card on %s", async (_label, close) => {
    const { container } = render(
      <Harness onTranslate={vi.fn().mockResolvedValue({ text: "hola" })} />
    )
    openTranslateCard(container)
    await act(async () => {})
    expect(screen.getByText("hola")).toBeTruthy()

    close()

    expect(screen.queryByRole("toolbar")).toBeNull()
    // And it stays closed: a stale card must not come back on the next
    // selectionchange the page happens to fire.
    selectionChanged()
    expect(screen.queryByText("hola")).toBeNull()
  })

  it("shows the question box and the translation card one at a time", async () => {
    const { container } = render(
      <Harness
        onAsk={vi.fn()}
        onTranslate={vi.fn().mockResolvedValue({ text: "hola" })}
      />
    )
    openTranslateCard(container)
    await act(async () => {})

    // Translating: no composer, and no way to open one.
    expect(screen.queryByRole("textbox")).toBeNull()
    expect(screen.queryByRole("button", { name: "Ask" })).toBeNull()

    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Close translation" }))
    })

    // Asking: no card, and no way to open one.
    openAskComposer(container)
    expect(screen.queryByText("Original")).toBeNull()
    expect(screen.queryByRole("button", { name: "Translate" })).toBeNull()
  })

  it("drops a translation that lands after its card was replaced", async () => {
    // Content-addressed translations can take a while, so a slow first request
    // can easily outlive the card that asked for it. Letting it land would
    // overwrite a newer answer — or resurrect a dismissed card entirely.
    const resolvers: Array<(attempt: { text: string }) => void> = []
    const onTranslate = vi.fn(
      () =>
        new Promise<{ text: string }>((resolve) => {
          resolvers.push(resolve)
        })
    )
    const { container } = render(<Harness onTranslate={onTranslate} />)
    openTranslateCard(container, "hello")
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Close translation" }))
    })

    openTranslateCard(container, "second selection")
    await act(async () => {
      resolvers[1]({ text: "second translation" })
    })
    expect(screen.getByText("second translation")).toBeTruthy()

    await act(async () => {
      resolvers[0]({ text: "first translation" })
    })
    expect(screen.queryByText("first translation")).toBeNull()
    expect(screen.getByText("second translation")).toBeTruthy()
  })

  it("re-clamps again when the result widens the card", async () => {
    // The card opens narrow (a spinner) and grows when the translation lands,
    // and the frame loop is frozen throughout — so both steps have to clamp
    // themselves or the card ends up sheared by the panel's overflow-hidden.
    //
    // 160 wide → half 80 → x clamped into [88, 312];
    // 300 wide → half 150 → x clamped into [158, 242].
    const restore = mockToolbarWidthByMode(160, 300, {
      loading: 160,
      done: 300,
    })
    try {
      const { container } = render(
        <Harness onTranslate={vi.fn().mockResolvedValue({ text: "hola" })} />
      )
      // Hard against the container's left edge, so both clamps actually bite.
      mockSelection(container.querySelector("[data-testid=para]"), "hello", {
        ...SELECTION_RECT,
        left: 0,
        right: 40,
        width: 40,
        x: 0,
      } as DOMRect)
      selectionChanged()
      selectionChanged()
      expect(screen.getByRole("toolbar").style.left).toBe("88px")

      act(() => {
        fireEvent.click(screen.getByRole("button", { name: "Translate" }))
      })
      expect(screen.getByRole("toolbar").style.left).toBe("88px")

      await act(async () => {})
      expect(screen.getByRole("toolbar").style.left).toBe("158px")
    } finally {
      restore()
    }
  })

  it("freezes every tracker while the translation card is open", async () => {
    // The regression this guards: the freeze predicate has three readers
    // (selectionchange, the deferred pointerup read, and the frame loop), and a
    // mode that teaches only some of them to stand down leaves the card hanging
    // at stale coordinates — or tears it down mid-request.
    const { container } = render(
      <Harness onTranslate={vi.fn().mockResolvedValue({ text: "hola" })} />
    )
    const para = container.querySelector("[data-testid=para]")
    openTranslateCard(container)
    await act(async () => {})
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    // The selection collapses underneath (a tap on touch does exactly this).
    mockSelection(null, "")
    selectionChanged()
    expect(screen.getByText("hola")).toBeTruthy()
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    // Same selection, new geometry — as after a scroll.
    mockSelection(para, "hello", {
      ...SELECTION_RECT,
      top: 260,
      bottom: 280,
      y: 260,
    } as DOMRect)
    selectionChanged()
    expect(screen.getByRole("toolbar").style.top).toBe("92px")

    // The deferred pointerup read and the frame loop honour it too.
    vi.useFakeTimers()
    try {
      act(() => {
        fireEvent.pointerUp(document)
        vi.runAllTimers()
        vi.advanceTimersByTime(50)
      })
    } finally {
      vi.useRealTimers()
    }
    expect(screen.getByRole("toolbar").style.top).toBe("92px")
  })
})
