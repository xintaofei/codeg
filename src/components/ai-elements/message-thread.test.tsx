import type { ReactNode } from "react"
import { act, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// A stand-in for the slice of `StickToBottomContext` the viewport sticker uses,
// so a test can drive `isAtBottom` / inspect `resizeDifference` directly.
const testState = vi.hoisted(() => ({
  scrollRef: { current: null as HTMLDivElement | null },
  scrollToBottom: vi.fn(),
  state: { isAtBottom: true, resizeDifference: 0 },
}))

vi.mock("use-stick-to-bottom", () => ({
  StickToBottom: ({
    children,
    ...props
  }: {
    children: ((context: unknown) => ReactNode) | ReactNode
  }) => (
    <div {...props}>
      {typeof children === "function" ? children(testState) : children}
    </div>
  ),
  useStickToBottomContext: () => testState,
}))

import { MessageThread } from "@/components/ai-elements/message-thread"

// jsdom has no ResizeObserver; capture the callback so a test can play a
// viewport resize through it, and record what got observed/disconnected.
let roCallback: ResizeObserverCallback | null = null
let observed: Element[] = []
let disconnects = 0

/** One entry shaped like what a viewport resize delivers. */
const resizeTo = (height: number) => {
  act(() => {
    roCallback?.(
      [{ contentRect: { height } } as unknown as ResizeObserverEntry],
      {} as ResizeObserver
    )
  })
}

/** Mount, then settle the observer's first (no-op) delivery at `height`. */
const mountThreadAt = (height: number) => {
  const result = render(
    <MessageThread>
      <span data-testid="thread-child">transcript</span>
    </MessageThread>
  )
  resizeTo(height)
  testState.scrollToBottom.mockClear()
  testState.state.resizeDifference = 0
  return result
}

beforeEach(() => {
  roCallback = null
  observed = []
  disconnects = 0
  testState.scrollRef.current = document.createElement("div")
  testState.scrollToBottom.mockReset()
  testState.state.isAtBottom = true
  testState.state.resizeDifference = 0
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(cb: ResizeObserverCallback) {
        roCallback = cb
      }
      observe(el: Element) {
        observed.push(el)
      }
      unobserve() {}
      disconnect() {
        disconnects += 1
      }
    }
  )
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("MessageThread viewport resize", () => {
  it("observes the scroll viewport and still renders its children", () => {
    mountThreadAt(400)

    expect(observed).toEqual([testState.scrollRef.current])
    expect(screen.getByTestId("thread-child")).toBeDefined()
  })

  it("ignores the observer's first delivery (a size, not a change)", () => {
    render(
      <MessageThread>
        <span />
      </MessageThread>
    )

    resizeTo(400)

    expect(testState.scrollToBottom).not.toHaveBeenCalled()
  })

  // The reported bug: the live-turn stats bar / restored terminal panel takes
  // vertical space away from the thread, which moves the bottom without moving
  // scrollTop — and fires neither a scroll event nor a content resize.
  it("re-pins to the bottom when the viewport shrinks under a pinned thread", () => {
    mountThreadAt(400)

    resizeTo(368)

    expect(testState.scrollToBottom).toHaveBeenCalledWith({
      animation: "instant",
      preserveScrollPosition: true,
    })
  })

  it("leaves a thread the user scrolled away from where it is", () => {
    mountThreadAt(400)
    testState.state.isAtBottom = false

    resizeTo(368)

    expect(testState.scrollToBottom).not.toHaveBeenCalled()
    expect(testState.state.resizeDifference).toBe(0)
  })

  // A growing viewport shrinks the maximum scroll offset, so the browser clamps
  // scrollTop and fires a scroll event that looks exactly like the user
  // scrolling up. `resizeDifference` is what makes the library discount it.
  it("marks the resize so the clamped scroll can't escape the lock", () => {
    mountThreadAt(368)

    resizeTo(400)

    expect(testState.state.resizeDifference).toBe(32)
  })

  // Real timers: the release rides a real `requestAnimationFrame`, which
  // vitest's fake timers leave alone.
  it("releases the resize mark a frame later", async () => {
    mountThreadAt(400)

    resizeTo(368)
    expect(testState.state.resizeDifference).toBe(-32)

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 100))
    })

    expect(testState.state.resizeDifference).toBe(0)
  })

  it("stops observing when the thread unmounts", () => {
    const { unmount } = mountThreadAt(400)

    unmount()

    expect(disconnects).toBe(1)
  })
})
