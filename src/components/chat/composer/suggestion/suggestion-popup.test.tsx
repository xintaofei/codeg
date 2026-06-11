import { act, render, screen, within } from "@testing-library/react"
import { createRef } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SuggestionPopup } from "./suggestion-popup"
import type {
  ReferenceSearch,
  SuggestionGroup,
  SuggestionPopupHandle,
} from "./types"

// Distinct, non-colliding text: a row's label must differ from its detail and
// from the agent icon's <title> ("Codex") so findByText is unambiguous.
const fileRef = {
  refType: "file" as const,
  id: "alpha.md",
  label: "alpha.md",
  uri: "file:///docs/alpha.md",
  meta: null,
}
const agentRef = {
  refType: "agent" as const,
  id: "codex",
  label: "Codex Helper",
  uri: null,
  meta: { agentType: "codex" as const },
}

const groups: SuggestionGroup[] = [
  {
    kind: "file",
    label: "Files",
    items: [{ reference: fileRef, detail: "docs/alpha.md" }],
  },
  { kind: "agent", label: "Agents", items: [{ reference: agentRef }] },
]

const search: ReferenceSearch = () => groups
const emptySearch: ReferenceSearch = () => []

const state = {
  query: "a",
  range: { from: 1, to: 3 },
  getClientRect: () => null,
}

function mountPopup(
  overrides: Partial<Parameters<typeof SuggestionPopup>[0]> = {}
) {
  const ref = createRef<SuggestionPopupHandle>()
  const onSelect = vi.fn()
  const onClose = vi.fn()
  render(
    <SuggestionPopup
      ref={ref}
      state={state}
      search={search}
      onSelect={onSelect}
      onClose={onClose}
      {...overrides}
    />
  )
  return { ref, onSelect, onClose }
}

function key(name: string): KeyboardEvent {
  return { key: name } as KeyboardEvent
}

describe("SuggestionPopup", () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("renders grouped results from the search provider", async () => {
    mountPopup()
    expect(await screen.findByText("alpha.md")).toBeInTheDocument()
    expect(screen.getByText("Files")).toBeInTheDocument()
    expect(screen.getByText("Agents")).toBeInTheDocument()
    expect(screen.getByText("Codex Helper")).toBeInTheDocument()
  })

  it("shows an empty state when there are no matches", async () => {
    mountPopup({ search: emptySearch, emptyLabel: "Nothing" })
    // Scope to the listbox: the sr-only live region carries the same text.
    const panel = screen.getByTestId("mention-popup")
    expect(await within(panel).findByText("Nothing")).toBeInTheDocument()
  })

  it("selects the highlighted row on Enter (default = first)", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("alpha.md")
    act(() => {
      expect(ref.current?.onKeyDown(key("Enter"))).toBe(true)
    })
    expect(onSelect).toHaveBeenCalledWith(fileRef, state.range)
  })

  it("moves the selection with ArrowDown before selecting", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowDown")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
  })

  it("wraps the selection with ArrowUp from the first row", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowUp")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
  })

  it("closes on Escape and reports the key as consumed", async () => {
    const { ref, onClose } = mountPopup()
    await screen.findByText("alpha.md")
    let consumed = false
    act(() => {
      consumed = ref.current?.onKeyDown(key("Escape")) ?? false
    })
    expect(consumed).toBe(true)
    expect(onClose).toHaveBeenCalled()
  })

  it("does not consume unrelated keys", async () => {
    const { ref } = mountPopup()
    await screen.findByText("alpha.md")
    expect(ref.current?.onKeyDown(key("x"))).toBe(false)
  })

  it("does not select stale results after the query changes", async () => {
    const ref = createRef<SuggestionPopupHandle>()
    const onSelect = vi.fn()
    const view = (query: string, to: number) => (
      <SuggestionPopup
        ref={ref}
        state={{ query, range: { from: 1, to }, getClientRect: () => null }}
        search={search}
        onSelect={onSelect}
        onClose={vi.fn()}
        loadingLabel="Loading"
      />
    )
    const { rerender } = render(view("a", 2))
    await screen.findByText("alpha.md") // fresh results for "a"

    // Query advances; the shown results now answer the *previous* query.
    rerender(view("ab", 3))
    expect(screen.queryByText("alpha.md")).toBeNull()
    expect(
      within(screen.getByTestId("mention-popup")).getByText("Loading")
    ).toBeInTheDocument()

    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("selects on click (mousedown) and prevents default to keep editor focus", async () => {
    const { onSelect } = mountPopup()
    const label = await screen.findByText("alpha.md")
    const button = label.closest("button")
    expect(button).not.toBeNull()
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    })
    act(() => {
      button?.dispatchEvent(event)
    })
    expect(onSelect).toHaveBeenCalledWith(fileRef, state.range)
    // preventDefault keeps focus in the editor rather than the popup button.
    expect(event.defaultPrevented).toBe(true)
  })

  it("positions and reveals the caret-anchored panel once measured", async () => {
    render(
      <SuggestionPopup
        ref={createRef<SuggestionPopupHandle>()}
        state={{
          query: "a",
          range: { from: 1, to: 3 },
          getClientRect: () =>
            ({ left: 100, top: 600, bottom: 620 }) as DOMRect,
        }}
        search={search}
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />
    )
    await screen.findByText("alpha.md")
    const container = screen.getByTestId("mention-popup")
      .parentElement as HTMLElement
    // The layout effect measured the panel and clamped/flipped it into view.
    expect(container.style.visibility).toBe("visible")
    expect(container.style.position).toBe("fixed")
    expect(container.dataset.placement).toBeTruthy()
  })

  it("clamps the rendered panel coordinates into the viewport", async () => {
    // A real (nonzero) panel size lets the viewport clamp actually bite.
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
      width: 320,
      height: 288,
    } as DOMRect)
    render(
      <SuggestionPopup
        ref={createRef<SuggestionPopupHandle>()}
        state={{
          query: "a",
          range: { from: 1, to: 3 },
          // Caret hard against the right edge of the jsdom 1024px viewport.
          getClientRect: () =>
            ({ left: 1000, top: 600, bottom: 620 }) as DOMRect,
        }}
        search={search}
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />
    )
    await screen.findByText("alpha.md")
    const container = screen.getByTestId("mention-popup")
      .parentElement as HTMLElement
    // left clamps to 1024 - 320 - 8 = 696 (not the raw caret x of 1000).
    expect(container.style.left).toBe("696px")
    // Room above (600px) fits → placed above: 600 - 4 - 288 = 308.
    expect(container.style.top).toBe("308px")
    expect(container.dataset.placement).toBe("above")
  })

  it("re-anchors to the live caret rect on resize (not a stale snapshot)", async () => {
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
      width: 320,
      height: 288,
    } as DOMRect)
    let caretLeft = 100
    const getClientRect = vi.fn(
      () => ({ left: caretLeft, top: 600, bottom: 620 }) as DOMRect
    )
    render(
      <SuggestionPopup
        ref={createRef<SuggestionPopupHandle>()}
        state={{ query: "a", range: { from: 1, to: 3 }, getClientRect }}
        search={search}
        onSelect={vi.fn()}
        onClose={vi.fn()}
      />
    )
    await screen.findByText("alpha.md")
    const container = screen.getByTestId("mention-popup")
      .parentElement as HTMLElement
    expect(container.style.left).toBe("100px")
    // The caret reflows; a resize must re-read the live getter, not a snapshot.
    const before = getClientRect.mock.calls.length
    caretLeft = 300
    act(() => {
      window.dispatchEvent(new Event("resize"))
    })
    expect(getClientRect.mock.calls.length).toBeGreaterThan(before)
    expect(container.style.left).toBe("300px")
  })

  it("exposes listbox + option roles with the active option selected", async () => {
    mountPopup({ listboxLabel: "Mentions" })
    await screen.findByText("alpha.md")
    // The listbox is a child of the (testid) panel and owns only options.
    const listbox = screen.getByRole("listbox", { name: "Mentions" })
    expect(listbox).toHaveAttribute("id", "mention-listbox")
    const options = within(listbox).getAllByRole("option")
    expect(options).toHaveLength(2)
    expect(options[0]).toHaveAttribute("aria-selected", "true")
    expect(options[0]).toHaveAttribute("id", "mention-option-0")
    expect(options[1]).toHaveAttribute("aria-selected", "false")
  })

  it("keeps the decorative icon out of the option's accessible name", async () => {
    mountPopup()
    await screen.findByText("Codex Helper")
    // The agent row's AgentIcon is a titled <svg>; if it weren't decorative the
    // option would be named "Codex Codex Helper". The name must be just label.
    expect(
      screen.getByRole("option", { name: "Codex Helper" })
    ).toBeInTheDocument()
  })

  it("moves aria-selected with the keyboard", async () => {
    const { ref } = mountPopup()
    await screen.findByText("alpha.md")
    act(() => ref.current?.onKeyDown(key("ArrowDown")))
    const options = screen
      .getByTestId("mention-popup")
      .querySelectorAll('[role="option"]')
    expect(options[0]).toHaveAttribute("aria-selected", "false")
    expect(options[1]).toHaveAttribute("aria-selected", "true")
  })

  it("announces the result count via a polite live region", async () => {
    mountPopup()
    await screen.findByText("alpha.md")
    const status = screen.getByRole("status")
    expect(status).toHaveAttribute("aria-live", "polite")
    expect(status).toHaveTextContent("2 results")
  })

  it("reports the active option id to the host for aria-activedescendant", async () => {
    const onActiveOptionChange = vi.fn()
    mountPopup({ onActiveOptionChange })
    await screen.findByText("alpha.md")
    expect(onActiveOptionChange).toHaveBeenLastCalledWith("mention-option-0")
  })

  it("reports a null active option while loading or empty", async () => {
    const onActiveOptionChange = vi.fn()
    mountPopup({
      search: emptySearch,
      onActiveOptionChange,
      emptyLabel: "None",
    })
    const panel = screen.getByTestId("mention-popup")
    await within(panel).findByText("None")
    expect(onActiveOptionChange).toHaveBeenLastCalledWith(null)
  })

  it("shows a non-selectable, aria-hidden hint for a truncated group", async () => {
    const truncatedSearch: ReferenceSearch = () => [
      {
        kind: "file",
        label: "Files",
        items: [{ reference: fileRef, detail: "docs/alpha.md" }],
        truncated: true,
      },
    ]
    mountPopup({ search: truncatedSearch, moreLabel: "More — keep typing" })
    await screen.findByText("alpha.md")
    const panel = screen.getByTestId("mention-popup")
    const hint = within(panel).getByText("More — keep typing")
    // Decorative: hidden from AT (the live region announces truncation) and not
    // an option, so arrow/Enter can never land on it.
    expect(hint).toHaveAttribute("aria-hidden", "true")
    expect(panel.querySelectorAll('[role="option"]')).toHaveLength(1)
    // The polite live region also conveys truncation to screen readers.
    expect(screen.getByRole("status")).toHaveTextContent("More — keep typing")
  })
})
