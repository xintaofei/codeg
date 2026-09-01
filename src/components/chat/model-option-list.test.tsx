import {
  render,
  screen,
  cleanup,
  within,
  fireEvent,
} from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, describe, expect, it, vi } from "vitest"
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  type ReactNode,
  type Ref,
} from "react"

// The list sizes its scroll window in rem, so it reads the live zoom level —
// which throws outside an AppearanceProvider. Pin it at 100% (1rem = 16px), the
// baseline every height assertion here is written against. Spread the real
// module so the other appearance hooks keep their real (provider-requiring)
// behaviour instead of silently resolving to `undefined` if something here
// starts using one.
vi.mock("@/hooks/use-appearance", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/use-appearance")>()),
  useZoomLevel: () => ({ zoomLevel: 100, setZoomLevel: () => {} }),
}))

// virtua renders ZERO rows under jsdom (no layout), so mock the `Virtualizer` to
// render every child directly — the established pattern (see logs-settings and
// sidebar-conversation-list tests). Forward a no-op scrollToIndex handle so the
// keyboard navigation doesn't throw.
vi.mock("virtua", () => ({
  Virtualizer: forwardRef(function VirtualizerMock(
    props: { children?: ReactNode },
    ref: Ref<{ scrollToIndex: (i: number) => void }>
  ) {
    useImperativeHandle(ref, () => ({ scrollToIndex: vi.fn() }))
    return <>{props.children}</>
  }),
}))

// The list mounts virtua only after the OverlayScrollbars viewport exists, which
// it learns via `onViewportRef`. jsdom never lays out / initializes OS, so drive
// that callback synchronously here (with a real element) so the options render.
vi.mock("@/components/ui/scroll-area", () => ({
  ScrollArea: ({
    children,
    onViewportRef,
  }: {
    children?: ReactNode
    onViewportRef?: (el: HTMLElement | null) => void
  }) => {
    useEffect(() => {
      onViewportRef?.(document.createElement("div"))
    }, [onViewportRef])
    return <>{children}</>
  },
}))

import { ModelOptionList } from "./model-option-list"
import type { ModelOptionGroup } from "@/lib/model-config-groups"

const GROUPS: ModelOptionGroup[] = [
  {
    key: "anthropic",
    name: "anthropic",
    options: [
      { value: "anthropic/opus", name: "opus", description: null },
      { value: "anthropic/sonnet", name: "sonnet", description: null },
    ],
  },
  {
    key: "openai",
    name: "openai",
    options: [{ value: "openai/gpt-4o", name: "gpt-4o", description: null }],
  },
]

function renderList(
  overrides: Partial<Parameters<typeof ModelOptionList>[0]> = {}
) {
  const onSelect = vi.fn()
  render(
    <ModelOptionList
      groups={GROUPS}
      currentValue="anthropic/opus"
      onSelect={onSelect}
      searchPlaceholder="Search models"
      searchAriaLabel="Search models"
      listAriaLabel="Models"
      emptyLabel="No models found"
      {...overrides}
    />
  )
  return { onSelect }
}

describe("ModelOptionList", () => {
  afterEach(() => cleanup())

  it("renders grouped options and marks the current value selected", () => {
    renderList()
    expect(screen.getByText("anthropic")).toBeInTheDocument()
    expect(screen.getByText("openai")).toBeInTheDocument()
    expect(screen.getByRole("option", { name: /opus/ })).toHaveAttribute(
      "aria-selected",
      "true"
    )
    expect(screen.getByRole("option", { name: /sonnet/ })).toHaveAttribute(
      "aria-selected",
      "false"
    )
  })

  it("filters options as you type (matching name or value)", async () => {
    const user = userEvent.setup()
    renderList()
    await user.type(screen.getByRole("combobox"), "gpt")
    expect(screen.getByRole("option", { name: /gpt-4o/ })).toBeInTheDocument()
    expect(screen.queryByRole("option", { name: /opus/ })).toBeNull()
    // The now-empty anthropic group drops its header too.
    expect(screen.queryByText("anthropic")).toBeNull()
  })

  it("shows the empty label when nothing matches", async () => {
    const user = userEvent.setup()
    renderList()
    await user.type(screen.getByRole("combobox"), "zzzz")
    expect(screen.getByText("No models found")).toBeInTheDocument()
    expect(screen.queryByRole("option")).toBeNull()
  })

  it("commits a value on click", async () => {
    const user = userEvent.setup()
    const { onSelect } = renderList()
    await user.click(screen.getByRole("option", { name: /sonnet/ }))
    expect(onSelect).toHaveBeenCalledWith("anthropic/sonnet")
  })

  it("navigates with the keyboard and commits on Enter", async () => {
    const user = userEvent.setup()
    const { onSelect } = renderList()
    const input = screen.getByRole("combobox")
    await user.click(input)
    // Cursor starts at the first option (opus); ArrowDown → sonnet, Enter picks.
    await user.keyboard("{ArrowDown}{Enter}")
    expect(onSelect).toHaveBeenCalledWith("anthropic/sonnet")
  })

  it("ignores Enter while an IME composition is in flight", () => {
    const { onSelect } = renderList()
    const input = screen.getByRole("combobox")
    // Enter during CJK composition confirms the candidate — it must NOT select.
    fireEvent.keyDown(input, { key: "Enter", isComposing: true })
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("points aria-activedescendant at the active option", async () => {
    const user = userEvent.setup()
    renderList()
    const input = screen.getByRole("combobox")
    const initial = input.getAttribute("aria-activedescendant")
    expect(initial).toBeTruthy()
    // The active descendant must resolve to a real option element.
    const listbox = screen.getByRole("listbox")
    expect(within(listbox).getByRole("option", { name: /opus/ }).id).toBe(
      initial
    )
    await user.click(input)
    await user.keyboard("{ArrowDown}")
    expect(input.getAttribute("aria-activedescendant")).not.toBe(initial)
  })
})
