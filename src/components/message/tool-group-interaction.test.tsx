import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import type { ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import { AUTO_CLOSE_DELAY_MS } from "@/lib/auto-collapse-timing"
import enMessages from "@/i18n/messages/en.json"

vi.mock("@/components/ai-elements/shimmer", () => ({
  Shimmer: ({ children }: { children?: ReactNode }) => <span>{children}</span>,
}))

import { ContentPartsRenderer } from "./content-parts-renderer"

type CommandState = "input-available" | "output-available"
type ToolCallPart = Extract<AdaptedContentPart, { type: "tool-call" }>

function commandPart(
  state: CommandState,
  output: string | null = null
): ToolCallPart {
  return {
    type: "tool-call",
    toolCallId: "call-1",
    toolName: "bash",
    input: "echo hi",
    state,
    output,
  }
}

function groupPart(
  isStreaming: boolean,
  items: ToolCallPart[]
): AdaptedContentPart {
  return { type: "tool-group", items, isStreaming }
}

function Harness({
  parts,
}: {
  parts: AdaptedContentPart | AdaptedContentPart[]
}) {
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ContentPartsRenderer
        parts={Array.isArray(parts) ? parts : [parts]}
        role="assistant"
      />
    </NextIntlClientProvider>
  )
}

function renderParts(parts: AdaptedContentPart | AdaptedContentPart[]) {
  return render(<Harness parts={parts} />)
}

function groupTrigger() {
  return screen.getByRole("button", { name: /Ran 1 command/ })
}

function commandTrigger() {
  return screen.getByRole("button", { name: /echo hi/ })
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe("tool-group auto-expand", () => {
  it("expands a streaming group while keeping command cards collapsed", () => {
    renderParts(groupPart(true, [commandPart("input-available")]))

    expect(groupTrigger()).toHaveAttribute("data-state", "open")
    expect(commandTrigger()).toHaveAttribute("data-state", "closed")
    expect(screen.queryByText(/\$ echo hi/)).not.toBeInTheDocument()
  })

  it("keeps a non-streaming group collapsed by default", () => {
    renderParts(groupPart(false, [commandPart("output-available", "hi")]))

    expect(groupTrigger()).toHaveAttribute("data-state", "closed")
    expect(
      screen.queryByRole("button", { name: /echo hi/ })
    ).not.toBeInTheDocument()
  })

  it("collapses one second after the last item settles", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS - 1)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")
    expect(
      screen.queryByRole("button", { name: /echo hi/ })
    ).not.toBeInTheDocument()
  })

  it("cancels the pending collapse when streaming resumes", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS / 2)
    })

    view.rerender(
      <Harness parts={groupPart(true, [commandPart("input-available")])} />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS * 2)
    })

    expect(groupTrigger()).toHaveAttribute("data-state", "open")
  })

  it("auto transitions never pollute the manual state", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")

    view.rerender(
      <Harness parts={groupPart(true, [commandPart("input-available")])} />
    )
    expect(groupTrigger()).toHaveAttribute("data-state", "open")
  })
})

describe("tool-group manual controls", () => {
  it("pins on the first click and closes on the second", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))
    const trigger = groupTrigger()

    fireEvent.click(trigger)
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    fireEvent.click(groupTrigger())
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    view.rerender(
      <Harness parts={groupPart(true, [commandPart("input-available")])} />
    )
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")
  })

  it("keeps a manually opened historical group open", () => {
    const view = renderParts(
      groupPart(false, [commandPart("output-available", "hi")])
    )

    fireEvent.click(groupTrigger())
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "bye")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "open")
  })
})

describe("tool-group child veto and focus", () => {
  it("defers auto-collapse while a child card is open", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))

    fireEvent.click(commandTrigger())
    expect(screen.getByText(/\$ echo hi/)).toBeInTheDocument()

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    fireEvent.click(commandTrigger())
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")
  })

  it("keeps the group open while focus is inside it", () => {
    const view = renderParts(groupPart(true, [commandPart("input-available")]))
    const child = commandTrigger()
    child.focus()

    view.rerender(
      <Harness
        parts={groupPart(false, [commandPart("output-available", "hi")])}
      />
    )
    act(() => {
      vi.advanceTimersByTime(AUTO_CLOSE_DELAY_MS)
    })
    expect(groupTrigger()).toHaveAttribute("data-state", "open")

    fireEvent.focusOut(child, { relatedTarget: document.body })
    expect(groupTrigger()).toHaveAttribute("data-state", "closed")
  })
})

describe("non-command cards inside a tool-group", () => {
  it("keep read and edit cards collapsed even while running", () => {
    const readPart: ToolCallPart = {
      type: "tool-call",
      toolCallId: "read-1",
      toolName: "read",
      input: JSON.stringify({ file_path: "a.txt" }),
      state: "input-available",
      output: null,
    }
    const editPart: ToolCallPart = {
      type: "tool-call",
      toolCallId: "edit-1",
      toolName: "edit",
      input: JSON.stringify({ file_path: "b.txt" }),
      state: "input-available",
      output: null,
    }

    renderParts(groupPart(true, [readPart, editPart]))

    expect(screen.getByRole("button", { name: /Read a\.txt/ })).toHaveAttribute(
      "data-state",
      "closed"
    )
    expect(screen.getByRole("button", { name: /Edit b\.txt/ })).toHaveAttribute(
      "data-state",
      "closed"
    )
  })
})
