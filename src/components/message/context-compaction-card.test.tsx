import { render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { ContextCompactionCard } from "./context-compaction-card"
import enMessages from "@/i18n/messages/en.json"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

function renderCard(props: {
  state?: ToolCallState
  meta?: Record<string, unknown> | null
}) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ContextCompactionCard state={props.state} meta={props.meta} />
    </NextIntlClientProvider>
  )
}

describe("ContextCompactionCard", () => {
  it("shows the token delta from grok's top-level boolean-marker fields", () => {
    renderCard({
      state: "output-available",
      meta: { contextCompaction: true, tokensBefore: 51777, tokensAfter: 4616 },
    })
    expect(
      screen.getByText("Context compacted · 51,777 → 4,616 tokens")
    ).toBeInTheDocument()
  })

  it("falls back to the plain label for codex 1.3.0's bare versioned payload", () => {
    renderCard({
      state: "output-available",
      meta: { contextCompaction: { version: 1 } },
    })
    expect(screen.getByText("Context compacted")).toBeInTheDocument()
  })

  it("reads preTokens/postTokens and durationMs from the versioned payload", () => {
    renderCard({
      state: "output-available",
      meta: {
        contextCompaction: {
          version: 1,
          preTokens: 51777,
          postTokens: 4616,
          durationMs: 3200,
        },
      },
    })
    expect(
      screen.getByText("Context compacted · 51,777 → 4,616 tokens")
    ).toBeInTheDocument()
    expect(screen.getByText("· 3.2s")).toBeInTheDocument()
  })

  it("shows the failed label when the versioned payload carries an error", () => {
    renderCard({
      state: "output-available",
      meta: {
        contextCompaction: { version: 1, error: "compaction interrupted" },
      },
    })
    const label = screen.getByText("Context compaction failed")
    expect(label).toBeInTheDocument()
    // The raw adapter error string is a tooltip, never inline prose.
    expect(label.parentElement).toHaveAttribute(
      "title",
      "compaction interrupted"
    )
  })

  it("shows the failed label for an output-error lifecycle without payload error", () => {
    renderCard({
      state: "output-error",
      meta: { contextCompaction: { version: 1 } },
    })
    expect(screen.getByText("Context compaction failed")).toBeInTheDocument()
  })

  it("keeps the in-progress label while compacting", () => {
    renderCard({
      state: "input-available",
      meta: { contextCompaction: { version: 1 } },
    })
    expect(screen.getByText("Compacting context…")).toBeInTheDocument()
  })
})
