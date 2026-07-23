import type { ReactNode } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const testState = vi.hoisted(() => ({
  scrollRef: { current: null as HTMLDivElement | null },
}))

vi.mock("use-stick-to-bottom", () => ({
  useStickToBottomContext: () => ({ scrollRef: testState.scrollRef }),
}))

vi.mock("virtua", () => ({
  Virtualizer: ({ children }: { children: ReactNode }) => <>{children}</>,
}))

vi.mock("@/components/ai-elements/message-thread", () => ({
  MessageThreadContent: ({
    children,
    scrollClassName,
  }: {
    children: ReactNode
    scrollClassName?: string
  }) => (
    <div
      ref={(element) => {
        testState.scrollRef.current = element
      }}
      className={scrollClassName}
      data-testid="viewport"
    >
      {children}
    </div>
  ),
}))

import { VirtualizedMessageThread } from "@/components/message/virtualized-message-thread"

function renderThread(
  content: ReactNode = <div data-testid="content">text</div>
) {
  return render(
    <VirtualizedMessageThread
      items={[{ id: "message-1" }]}
      getItemKey={(item) => item.id}
      renderItem={() => content}
    />
  )
}

function pointerDown(element: HTMLElement, button: number) {
  fireEvent(element, new MouseEvent("pointerdown", { bubbles: true, button }))
}

function keyDown(element: HTMLElement, key: string) {
  fireEvent.keyDown(element, { key })
}

beforeEach(() => {
  testState.scrollRef.current = null
})

describe("VirtualizedMessageThread focus origin", () => {
  it("marks pointer-origin focus and clears it on blur", () => {
    renderThread()
    const viewport = screen.getByTestId("viewport")

    pointerDown(screen.getByTestId("content"), 0)

    expect(document.activeElement).toBe(viewport)
    expect(viewport).toHaveAttribute("data-focus-origin", "pointer")
    expect(viewport.className).toContain(
      "data-[focus-origin=pointer]:focus-visible:ring-0"
    )

    fireEvent.blur(viewport)
    expect(viewport).not.toHaveAttribute("data-focus-origin")
  })

  it("clears the pointer marker on keyboard input so the ring returns", () => {
    renderThread()
    const viewport = screen.getByTestId("viewport")

    pointerDown(screen.getByTestId("content"), 0)
    expect(viewport).toHaveAttribute("data-focus-origin", "pointer")

    // Switching to keyboard scrolling drops the marker, so the suppressing
    // `data-[focus-origin=pointer]` selector no longer matches and the
    // keyboard focus ring becomes visible again.
    keyDown(viewport, "ArrowDown")
    expect(viewport).not.toHaveAttribute("data-focus-origin")
    expect(document.activeElement).toBe(viewport)
  })

  it("keeps keyboard-origin focus distinguishable", () => {
    renderThread()
    const viewport = screen.getByTestId("viewport")

    viewport.focus()

    expect(document.activeElement).toBe(viewport)
    expect(viewport).not.toHaveAttribute("data-focus-origin")
    expect(viewport.className).toContain("focus-visible:ring-2")
  })

  it("does not mark focus when an interactive control is clicked", () => {
    renderThread(<button data-testid="action">Action</button>)
    const viewport = screen.getByTestId("viewport")

    pointerDown(screen.getByTestId("action"), 0)

    expect(viewport).not.toHaveAttribute("data-focus-origin")
    expect(document.activeElement).not.toBe(viewport)
  })

  it("does not mark focus for a right click", () => {
    renderThread()
    const viewport = screen.getByTestId("viewport")

    pointerDown(screen.getByTestId("content"), 2)

    expect(viewport).not.toHaveAttribute("data-focus-origin")
    expect(document.activeElement).not.toBe(viewport)
  })
})
