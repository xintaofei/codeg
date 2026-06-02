import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { ShadcnPreview } from "./shadcn-preview"

// useTranslations 在 mock 下原样返回 key，遮罩文案即 "preview.loading"。
vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

const OVERLAY = "preview.loading"

function getIframe(container: HTMLElement): HTMLIFrameElement {
  const iframe = container.querySelector("iframe")
  if (!iframe) throw new Error("iframe not found")
  return iframe
}

describe("ShadcnPreview", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.clearAllTimers()
    vi.useRealTimers()
  })

  it("clears the overlay on load and never re-raises it after the debounce window", () => {
    // 回归原 bug：load 后遮罩消失，且越过 debounce 窗口也不再被顶起。
    const { container } = render(<ShadcnPreview previewUrl="https://x/a" />)
    expect(screen.getByText(OVERLAY)).toBeInTheDocument()

    fireEvent.load(getIframe(container))
    expect(screen.queryByText(OVERLAY)).toBeNull()

    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(screen.queryByText(OVERLAY)).toBeNull()
  })

  it("debounces a URL change, then shows the overlay until the new page loads", () => {
    const { container, rerender } = render(
      <ShadcnPreview previewUrl="https://x/a" />
    )
    fireEvent.load(getIframe(container))
    expect(screen.queryByText(OVERLAY)).toBeNull()

    rerender(<ShadcnPreview previewUrl="https://x/b" />)
    // debounce 窗口内：仍展示 A，无遮罩。
    act(() => {
      vi.advanceTimersByTime(300)
    })
    expect(getIframe(container).getAttribute("src")).toBe("https://x/a")
    expect(screen.queryByText(OVERLAY)).toBeNull()

    // 越过 500ms：提交 B 并出现遮罩。
    act(() => {
      vi.advanceTimersByTime(300)
    })
    expect(getIframe(container).getAttribute("src")).toBe("https://x/b")
    expect(screen.getByText(OVERLAY)).toBeInTheDocument()

    fireEvent.load(getIframe(container))
    expect(screen.queryByText(OVERLAY)).toBeNull()
  })

  it("coalesces rapid URL changes to the final value", () => {
    const { container, rerender } = render(
      <ShadcnPreview previewUrl="https://x/a" />
    )
    fireEvent.load(getIframe(container))

    rerender(<ShadcnPreview previewUrl="https://x/b" />)
    act(() => {
      vi.advanceTimersByTime(200)
    })
    rerender(<ShadcnPreview previewUrl="https://x/c" />)
    act(() => {
      vi.advanceTimersByTime(500)
    })

    expect(getIframe(container).getAttribute("src")).toBe("https://x/c")
  })

  it("hides the overlay via the timeout fallback if load never fires", () => {
    render(<ShadcnPreview previewUrl="https://x/a" />)
    expect(screen.getByText(OVERLAY)).toBeInTheDocument()

    act(() => {
      vi.advanceTimersByTime(15000)
    })
    expect(screen.queryByText(OVERLAY)).toBeNull()
  })
})
