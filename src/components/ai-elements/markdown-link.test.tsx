import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { LinkSafetyModalProps } from "streamdown"

const mocks = vi.hoisted(() => ({
  onLinkCheck: vi.fn<(url: string) => boolean | Promise<boolean>>(),
  renderModal: vi.fn((props: LinkSafetyModalProps) =>
    props.isOpen ? <div data-testid="link-modal">{props.url}</div> : null
  ),
}))

vi.mock("./link-safety", () => ({
  useStreamdownLinkSafety: () => ({
    enabled: true,
    onLinkCheck: mocks.onLinkCheck,
    renderModal: mocks.renderModal,
  }),
}))

import { MarkdownLink } from "./markdown-link"

describe("MarkdownLink", () => {
  beforeEach(() => {
    mocks.onLinkCheck.mockReset()
    mocks.renderModal.mockClear()
    vi.spyOn(window, "open").mockReturnValue(null)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it.each([
    ["https://example.com", "web"],
    ["file:///repo/src/app.ts", "file"],
    ["/repo/src/app.ts", "file"],
    ["mailto:hi@example.com", "email"],
    ["tel:+15550100", "phone"],
  ])("tags %s with a %s type icon", (href, kind) => {
    render(<MarkdownLink href={href}>{href}</MarkdownLink>)

    const button = screen.getByRole("button")
    expect(button).toHaveAttribute("data-resource-kind", kind)
    // The lucide icon renders an inline svg before the link text.
    expect(button.querySelector("svg")).not.toBeNull()
  })

  it.each([["#section"], ["src/main.rs"], ["vscode://file/repo/src/app.ts"]])(
    "renders %s without a type icon",
    (href) => {
      render(<MarkdownLink href={href}>{href}</MarkdownLink>)

      const button = screen.getByRole("button")
      expect(button).not.toHaveAttribute("data-resource-kind")
      expect(button.querySelector("svg")).toBeNull()
    }
  )

  it("opens external links directly when link-safety approves", async () => {
    mocks.onLinkCheck.mockReturnValue(true)

    render(<MarkdownLink href="https://example.com/docs">docs</MarkdownLink>)
    fireEvent.click(screen.getByRole("button"))

    await waitFor(() => {
      expect(window.open).toHaveBeenCalledWith(
        "https://example.com/docs",
        "_blank",
        "noreferrer"
      )
    })
    expect(screen.queryByTestId("link-modal")).not.toBeInTheDocument()
  })

  it("routes declined links through the link-safety modal hook", async () => {
    mocks.onLinkCheck.mockReturnValue(false)

    render(<MarkdownLink href="file:///repo/src/app.ts">app.ts</MarkdownLink>)
    fireEvent.click(screen.getByRole("button"))

    await waitFor(() => {
      expect(screen.getByTestId("link-modal")).toBeInTheDocument()
    })
    expect(window.open).not.toHaveBeenCalled()
  })

  it("does nothing when clicking an incomplete (streaming) link", async () => {
    render(
      <MarkdownLink href="streamdown:incomplete-link">partial</MarkdownLink>
    )

    const button = screen.getByRole("button")
    expect(button).not.toHaveAttribute("data-resource-kind")
    expect(button.querySelector("svg")).toBeNull()

    fireEvent.click(button)
    expect(window.open).not.toHaveBeenCalled()
    expect(mocks.onLinkCheck).not.toHaveBeenCalled()
    expect(screen.queryByTestId("link-modal")).not.toBeInTheDocument()
  })
})
