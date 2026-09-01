import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import type { ReactNode } from "react"
import { describe, expect, it, vi } from "vitest"

vi.mock("streamdown", () => ({
  Streamdown: ({
    children,
    mode,
    parseIncompleteMarkdown,
  }: {
    children: ReactNode
    mode?: string
    parseIncompleteMarkdown?: boolean
  }) => (
    <div
      data-testid="streamdown-root"
      data-mode={mode}
      data-parse-incomplete={String(parseIncompleteMarkdown)}
    >
      {children}
    </div>
  ),
  defaultRemarkPlugins: {},
  defaultRehypePlugins: {},
}))

vi.mock("@streamdown/cjk", () => ({ cjk: {} }))
vi.mock("@streamdown/math", () => ({ createMathPlugin: () => ({}) }))
vi.mock("@streamdown/mermaid", () => ({ mermaid: {} }))
vi.mock("@streamdown/code", () => ({
  code: { highlight: vi.fn(), supportsLanguage: vi.fn(() => true) },
}))

vi.mock("@/components/ai-elements/link-safety", () => ({
  useStreamdownLinkSafety: () => ({ enabled: false }),
}))

import enMessages from "@/i18n/messages/en.json"
import { Reasoning, ReasoningContent, ReasoningTrigger } from "./reasoning"

function tree(isStreaming: boolean) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <Reasoning isStreaming={isStreaming} defaultOpen>
        <ReasoningTrigger />
        <ReasoningContent>{"see `tools/dsv4-c1/*` please."}</ReasoningContent>
      </Reasoning>
    </NextIntlClientProvider>
  )
}

function renderReasoning(isStreaming: boolean) {
  return render(tree(isStreaming))
}

describe("ReasoningContent", () => {
  // Reasoning auto-opens while the model is thinking, so this panel is on the
  // streaming hot path: pinning it static would re-parse the whole block on
  // every delta instead of only its tail.
  it("keeps the live block in streaming mode", () => {
    renderReasoning(true)

    const root = screen.getByTestId("streamdown-root")
    expect(root).toHaveAttribute("data-mode", "streaming")
    expect(root).toHaveAttribute("data-parse-incomplete", "true")
  })

  it("drops to static once thinking has settled, so remend cannot append leftover * / _", () => {
    renderReasoning(false)

    const root = screen.getByTestId("streamdown-root")
    expect(root).toHaveAttribute("data-mode", "static")
    expect(root).toHaveAttribute("data-parse-incomplete", "false")
  })

  // The transition is the case that actually matters, and it arrives through
  // context rather than props: `ReasoningContent` is memoized and its own props
  // (the text) do not change on the last delta, so only the context update can
  // repaint it without remend's leftover closer.
  it("repaints when thinking ends even though its own props did not change", () => {
    const { rerender } = renderReasoning(true)
    expect(screen.getByTestId("streamdown-root")).toHaveAttribute(
      "data-mode",
      "streaming"
    )

    rerender(tree(false))

    const root = screen.getByTestId("streamdown-root")
    expect(root).toHaveAttribute("data-mode", "static")
    expect(root).toHaveAttribute("data-parse-incomplete", "false")
  })
})
