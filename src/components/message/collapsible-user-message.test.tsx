import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it } from "vitest"

import { CollapsibleUserMessage } from "./collapsible-user-message"
import enMessages from "@/i18n/messages/en.json"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const TEXT_PART: AdaptedContentPart[] = [{ type: "text", text: "hello world" }]

// jsdom does no layout: scrollHeight/clientHeight (defined on Element, per
// Element-impl.js) both default to 0, which already reads as "not
// overflowing" for the short-content case below. The overflow cases patch
// both onto Element.prototype *before* rendering, so the component's
// synchronous mount-time measurement — it doesn't wait on a ResizeObserver
// callback, since the global jsdom stub in test-setup.ts never invokes one —
// picks up the mocked heights on its first read.
function mockScrollMetrics(scrollHeight: number, clientHeight: number) {
  const scrollHeightDescriptor = Object.getOwnPropertyDescriptor(
    Element.prototype,
    "scrollHeight"
  )
  const clientHeightDescriptor = Object.getOwnPropertyDescriptor(
    Element.prototype,
    "clientHeight"
  )
  Object.defineProperty(Element.prototype, "scrollHeight", {
    configurable: true,
    get: () => scrollHeight,
  })
  Object.defineProperty(Element.prototype, "clientHeight", {
    configurable: true,
    get: () => clientHeight,
  })
  return () => {
    if (scrollHeightDescriptor) {
      Object.defineProperty(
        Element.prototype,
        "scrollHeight",
        scrollHeightDescriptor
      )
    }
    if (clientHeightDescriptor) {
      Object.defineProperty(
        Element.prototype,
        "clientHeight",
        clientHeightDescriptor
      )
    }
  }
}

describe("CollapsibleUserMessage", () => {
  let restoreMetrics: (() => void) | null = null

  afterEach(() => {
    restoreMetrics?.()
    restoreMetrics = null
  })

  it("renders short content with no toggle", () => {
    renderWithIntl(<CollapsibleUserMessage parts={TEXT_PART} />)

    expect(screen.getByText("hello world")).toBeInTheDocument()
    expect(
      screen.queryByTestId("collapsible-user-message-toggle")
    ).not.toBeInTheDocument()
    expect(
      screen.getByTestId("collapsible-user-message-content")
    ).not.toHaveClass("collapsed-user-message-fade")
  })

  it("shows a Show more toggle when content overflows the collapsed height", () => {
    restoreMetrics = mockScrollMetrics(600, 240)

    renderWithIntl(<CollapsibleUserMessage parts={TEXT_PART} />)

    const content = screen.getByTestId("collapsible-user-message-content")
    const toggle = screen.getByTestId("collapsible-user-message-toggle")
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(toggle).toHaveTextContent("Show more")
    expect(toggle).toHaveAttribute("aria-controls", content.id)
    expect(content).toHaveClass("max-h-60", "collapsed-user-message-fade")
  })

  it("expands to Show less on click and removes the clamp", () => {
    restoreMetrics = mockScrollMetrics(600, 240)

    renderWithIntl(<CollapsibleUserMessage parts={TEXT_PART} />)

    fireEvent.click(screen.getByTestId("collapsible-user-message-toggle"))

    const toggle = screen.getByTestId("collapsible-user-message-toggle")
    expect(toggle).toHaveAttribute("aria-expanded", "true")
    expect(toggle).toHaveTextContent("Show less")
    const content = screen.getByTestId("collapsible-user-message-content")
    expect(content).not.toHaveClass("max-h-60")
    expect(content).not.toHaveClass("collapsed-user-message-fade")

    // Clicking again re-collapses.
    fireEvent.click(toggle)
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(toggle).toHaveTextContent("Show more")
  })
})
