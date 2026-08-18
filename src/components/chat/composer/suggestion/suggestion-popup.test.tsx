import { act, fireEvent, render, screen, within } from "@testing-library/react"
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
  uri: "codeg://agent/codex",
  meta: { agentType: "codex" as const },
}
const agentRef2 = {
  refType: "agent" as const,
  id: "claude_code",
  // Label must differ from the AgentIcon's <title> ("Claude Code") so a plain
  // text query is unambiguous (the title text is in the DOM even when decorative).
  label: "Claude Helper",
  uri: "codeg://agent/claude_code",
  meta: { agentType: "claude_code" as const },
}

// The provider keeps file-first order; the panel reorders to agent-first tabs.
const groups: SuggestionGroup[] = [
  {
    kind: "file",
    label: "Files",
    items: [{ reference: fileRef, detail: "docs/alpha.md" }],
  },
  {
    kind: "agent",
    label: "Agents",
    items: [{ reference: agentRef }, { reference: agentRef2 }],
  },
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

function key(name: string, shiftKey = false): KeyboardEvent {
  return { key: name, shiftKey } as KeyboardEvent
}

describe("SuggestionPopup", () => {
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("renders the active (agent-first) tab's options plus a four-tab strip", async () => {
    mountPopup()
    // Agent is the first non-empty tab, so its options show by default.
    expect(await screen.findByText("Codex Helper")).toBeInTheDocument()
    expect(screen.getByText("Claude Helper")).toBeInTheDocument()
    // The file tab's option is hidden until that tab is active.
    expect(screen.queryByText("alpha.md")).toBeNull()
    // Four fixed tabs (no skill tab), agent selected.
    expect(screen.getAllByRole("tab")).toHaveLength(4)
    expect(screen.getByRole("tab", { selected: true })).toHaveAccessibleName(
      /Agents/
    )
  })

  it("shows an empty state (but keeps the tabs) when there are no matches", async () => {
    mountPopup({ search: emptySearch, emptyLabel: "Nothing" })
    const panel = screen.getByTestId("mention-popup")
    expect(await within(panel).findByText("Nothing")).toBeInTheDocument()
    expect(screen.getAllByRole("tab")).toHaveLength(4)
  })

  it("selects the active tab's highlighted row on Enter (default = first agent)", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => {
      expect(ref.current?.onKeyDown(key("Enter"))).toBe(true)
    })
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
  })

  it("moves the selection with ArrowDown within the active tab", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowDown")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef2, state.range)
  })

  it("wraps the selection with ArrowUp from the first row", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowUp")))
    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).toHaveBeenCalledWith(agentRef2, state.range)
  })

  it("switches to the next tab with Tab and reveals its options", async () => {
    const { ref, onSelect } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => {
      expect(ref.current?.onKeyDown(key("Tab"))).toBe(true)
    })
    // agent → file; the file option appears and the agent options are gone.
    expect(await screen.findByText("alpha.md")).toBeInTheDocument()
    expect(screen.queryByText("Codex Helper")).toBeNull()
    expect(screen.getByRole("tab", { selected: true })).toHaveAccessibleName(
      /Files/
    )
    // Tab does not select.
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("wraps to the last tab with Shift+Tab", async () => {
    const { ref } = mountPopup()
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("Tab", true)))
    // agent (first) wraps backwards to commit (last in tab order); it's empty.
    expect(screen.getByRole("tab", { selected: true })).toHaveAccessibleName(
      /Commits/
    )
  })

  it("switches tabs on click, preventing default on mousedown to keep editor focus", async () => {
    mountPopup()
    await screen.findByText("Codex Helper")
    const filesTab = screen.getByRole("tab", { name: /Files/ })
    // mousedown preventDefault keeps focus in the editor (no blur)...
    const down = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    })
    act(() => {
      filesTab.dispatchEvent(down)
    })
    expect(down.defaultPrevented).toBe(true)
    // ...and the click performs the switch (so AT / synthetic click works too).
    act(() => {
      fireEvent.click(filesTab)
    })
    expect(await screen.findByText("alpha.md")).toBeInTheDocument()
    expect(screen.queryByText("Codex Helper")).toBeNull()
  })

  it("closes on Escape and reports the key as consumed", async () => {
    const { ref, onClose } = mountPopup()
    await screen.findByText("Codex Helper")
    let consumed = false
    act(() => {
      consumed = ref.current?.onKeyDown(key("Escape")) ?? false
    })
    expect(consumed).toBe(true)
    expect(onClose).toHaveBeenCalled()
  })

  it("does not consume unrelated keys", async () => {
    const { ref } = mountPopup()
    await screen.findByText("Codex Helper")
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
    await screen.findByText("Codex Helper") // fresh results for "a"

    // Query advances; the shown results now answer the *previous* query.
    rerender(view("ab", 3))
    expect(screen.queryByText("Codex Helper")).toBeNull()
    expect(
      within(screen.getByTestId("mention-popup")).getByText("Loading")
    ).toBeInTheDocument()

    act(() => ref.current?.onKeyDown(key("Enter")))
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("selects on click (mousedown) and prevents default to keep editor focus", async () => {
    const { onSelect } = mountPopup()
    const label = await screen.findByText("Codex Helper")
    const button = label.closest("button")
    expect(button).not.toBeNull()
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    })
    act(() => {
      button?.dispatchEvent(event)
    })
    expect(onSelect).toHaveBeenCalledWith(agentRef, state.range)
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
    await screen.findByText("Codex Helper")
    const container = screen.getByTestId("mention-popup")
      .parentElement as HTMLElement
    // The layout effect measured the panel and clamped/flipped it into view.
    expect(container.style.visibility).toBe("visible")
    expect(container.style.position).toBe("fixed")
    expect(container.dataset.placement).toBeTruthy()
    // The panel portals to `body`, which a modal Radix layer (a Dialog or Sheet
    // hosting the composer) sets to `pointer-events: none`. Without its own
    // `auto` the panel is click-dead inside one, and the press lands on the
    // document — which that layer reads as an outside press and closes itself.
    expect(container.style.pointerEvents).toBe("auto")
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
    await screen.findByText("Codex Helper")
    const container = screen.getByTestId("mention-popup")
      .parentElement as HTMLElement
    // left clamps to 1024 - 320 - 8 = 696 (not the raw caret x of 1000).
    expect(container.style.left).toBe("696px")
    // Room above (600px) fits → placed above: 600 - 4 - 288 = 308.
    expect(container.style.top).toBe("308px")
    expect(container.dataset.placement).toBe("above")
  })

  it("adopts the anchor box's width and left edge, opening above it", async () => {
    // The panel measures itself through the prototype; the anchor shadows that
    // with its own rect (an own property wins over the spied prototype).
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
      width: 320,
      height: 288,
    } as DOMRect)
    const box = document.createElement("div")
    document.body.appendChild(box)
    box.getBoundingClientRect = () =>
      ({ left: 40, top: 500, bottom: 620, width: 600 }) as DOMRect
    render(
      <SuggestionPopup
        ref={createRef<SuggestionPopupHandle>()}
        state={{
          query: "a",
          range: { from: 1, to: 3 },
          // Caret far from the box: the anchor wins, so this never shows up.
          getClientRect: () =>
            ({ left: 400, top: 610, bottom: 630 }) as DOMRect,
        }}
        search={search}
        onSelect={vi.fn()}
        onClose={vi.fn()}
        anchorRef={{ current: box }}
      />
    )
    await screen.findByText("Codex Helper")
    const panel = screen.getByTestId("mention-popup")
    const container = panel.parentElement as HTMLElement
    // Same width as the composer, not the default w-80.
    expect(panel.style.width).toBe("600px")
    // Left edge follows the box (not the caret's 400)…
    expect(container.style.left).toBe("40px")
    // …and it sits above the box's TOP (500 - 4 - 288), clear of the composer,
    // rather than hugging the caret line inside it.
    expect(container.style.top).toBe("208px")
    expect(container.dataset.placement).toBe("above")
    box.remove()
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
    await screen.findByText("Codex Helper")
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
    await screen.findByText("Codex Helper")
    // The listbox names the active tab and owns only that tab's options.
    const listbox = screen.getByRole("listbox", { name: "Mentions: Agents" })
    expect(listbox).toHaveAttribute("id", "mention-listbox")
    const options = within(listbox).getAllByRole("option")
    expect(options).toHaveLength(2)
    expect(options[0]).toHaveAttribute("aria-selected", "true")
    expect(options[0]).toHaveAttribute("id", "mention-option-agent-0")
    expect(options[1]).toHaveAttribute("aria-selected", "false")
    expect(options[1]).toHaveAttribute("id", "mention-option-agent-1")
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
    await screen.findByText("Codex Helper")
    act(() => ref.current?.onKeyDown(key("ArrowDown")))
    const options = screen
      .getByTestId("mention-popup")
      .querySelectorAll('[role="option"]')
    expect(options[0]).toHaveAttribute("aria-selected", "false")
    expect(options[1]).toHaveAttribute("aria-selected", "true")
  })

  it("announces the active tab + result count via a polite live region", async () => {
    mountPopup()
    await screen.findByText("Codex Helper")
    const status = screen.getByRole("status")
    expect(status).toHaveAttribute("aria-live", "polite")
    expect(status).toHaveTextContent("Agents: 2 results")
  })

  it("reports the active option id to the host for aria-activedescendant", async () => {
    const onActiveOptionChange = vi.fn()
    mountPopup({ onActiveOptionChange })
    await screen.findByText("Codex Helper")
    expect(onActiveOptionChange).toHaveBeenLastCalledWith(
      "mention-option-agent-0"
    )
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

  it("shows a non-selectable, aria-hidden hint for a truncated active tab", async () => {
    const truncatedSearch: ReferenceSearch = () => [
      {
        kind: "agent",
        label: "Agents",
        items: [{ reference: agentRef }],
        truncated: true,
      },
    ]
    mountPopup({ search: truncatedSearch, moreLabel: "More — keep typing" })
    await screen.findByText("Codex Helper")
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
