import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it } from "vitest"

import { GitLogCommitMessage } from "./git-log-commit-message"
import enMessages from "@/i18n/messages/en.json"

function renderWithIntl(ui: ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

const MESSAGE = "feat: add a thing\n\nWith a body."

// jsdom does no layout: scrollHeight/clientHeight both default to 0, which
// already reads as "not overflowing" for the short-message case. The overflow
// cases patch both onto Element.prototype *before* rendering so the
// synchronous mount-time measurement picks them up (the global ResizeObserver
// stub in test-setup.ts never invokes its callback). Mirrors
// collapsible-user-message.test.tsx.
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

describe("GitLogCommitMessage", () => {
  let restoreMetrics: (() => void) | null = null

  afterEach(() => {
    restoreMetrics?.()
    restoreMetrics = null
  })

  it("renders a short message clamped but with no toggle", () => {
    renderWithIntl(<GitLogCommitMessage message={MESSAGE} />)

    const content = screen.getByTestId("git-log-commit-message-content")
    expect(content).toHaveTextContent("feat: add a thing")
    expect(
      screen.queryByTestId("git-log-commit-message-toggle")
    ).not.toBeInTheDocument()
    expect(content).not.toHaveClass("collapsed-content-fade")
  })

  it("shows a Show more toggle when the message overflows the cap", () => {
    restoreMetrics = mockScrollMetrics(900, 192)

    renderWithIntl(<GitLogCommitMessage message={MESSAGE} />)

    const content = screen.getByTestId("git-log-commit-message-content")
    const toggle = screen.getByTestId("git-log-commit-message-toggle")
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(toggle).toHaveTextContent("Show more")
    expect(toggle).toHaveAttribute("aria-controls", content.id)
    expect(content).toHaveClass("max-h-48", "collapsed-content-fade")
  })

  it("expands to Show less on click and removes the clamp", () => {
    restoreMetrics = mockScrollMetrics(900, 192)

    renderWithIntl(<GitLogCommitMessage message={MESSAGE} />)

    fireEvent.click(screen.getByTestId("git-log-commit-message-toggle"))

    const toggle = screen.getByTestId("git-log-commit-message-toggle")
    expect(toggle).toHaveAttribute("aria-expanded", "true")
    expect(toggle).toHaveTextContent("Show less")
    const content = screen.getByTestId("git-log-commit-message-content")
    expect(content).not.toHaveClass("max-h-48")
    expect(content).not.toHaveClass("collapsed-content-fade")

    // Clicking again re-collapses.
    fireEvent.click(toggle)
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(toggle).toHaveTextContent("Show more")
  })
})
