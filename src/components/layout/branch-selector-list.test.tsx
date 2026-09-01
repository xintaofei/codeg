import { cleanup, render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  type ReactNode,
  type Ref,
} from "react"

// The list sizes its scroll window (and the action bubble) in rem, so it reads
// the live zoom level — which throws outside an AppearanceProvider. Pin it at
// 100%. Spread the real module so the other appearance hooks keep their real
// behaviour instead of silently resolving to `undefined`.
vi.mock("@/hooks/use-appearance", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/use-appearance")>()),
  useZoomLevel: () => ({ zoomLevel: 100, setZoomLevel: () => {} }),
}))

// virtua renders ZERO rows under jsdom (no layout), so render every child
// directly and forward a no-op scrollToIndex handle for keyboard navigation.
vi.mock("virtua", () => ({
  Virtualizer: forwardRef(function VirtualizerMock(
    props: { children?: ReactNode },
    ref: Ref<{ scrollToIndex: (i: number) => void }>
  ) {
    useImperativeHandle(ref, () => ({ scrollToIndex: vi.fn() }))
    return <>{props.children}</>
  }),
}))

// The list mounts virtua only once the OverlayScrollbars viewport exists, which
// it learns via `onViewportRef`. jsdom never lays out / initializes OS, so drive
// that callback with a real element so the rows render.
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

import { BranchSelectorList } from "./branch-selector-list"
import enMessages from "@/i18n/messages/en.json"
import {
  buildBranchTree,
  localBranchItems,
  worktreeBranchLeaves,
} from "@/lib/branch-tree"

const LOCAL = ["main", "feature/login", "task/132", "loop/x"]
const WORKTREES = ["task/132", "loop/x"]

function renderList(
  overrides: Partial<Parameters<typeof BranchSelectorList>[0]> = {}
) {
  const onLeafAction = vi.fn()
  const onRunOperation = vi.fn()
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BranchSelectorList
        operations={[{ id: "pull", label: "Pull code" }]}
        worktreeLeaves={worktreeBranchLeaves(WORKTREES, null)}
        localNodes={buildBranchTree(localBranchItems(LOCAL), "local")}
        remoteSections={[]}
        localCount={LOCAL.length}
        remoteCount={0}
        branch="main"
        worktreeBranchSet={new Set(WORKTREES)}
        mainWorktreeBranch={null}
        branchLoading={false}
        loading={false}
        onRunOperation={onRunOperation}
        onLeafAction={onLeafAction}
        {...overrides}
      />
    </NextIntlClientProvider>
  )
  return { onLeafAction, onRunOperation }
}

/** Row labels in render order, so section ordering is assertable. */
function optionLabels(): string[] {
  return screen
    .getAllByRole("option")
    .map((el) => el.textContent?.trim() ?? "")
    .filter(Boolean)
}

describe("BranchSelectorList — worktree section", () => {
  afterEach(() => cleanup())

  it("lists the other worktrees above the local branches", () => {
    renderList()
    const labels = optionLabels()
    expect(labels).toContain("Worktrees (2)")
    expect(labels).toContain("Local branches (4)")
    expect(labels.indexOf("Worktrees (2)")).toBeLessThan(
      labels.indexOf("Local branches (4)")
    )
    // Flat and whole-ref: no "task/" prefix group swallowing the two rows.
    expect(labels.slice(labels.indexOf("Worktrees (2)") + 1, 3 + 1)).toEqual([
      "loop/x",
      "task/132",
    ])
  })

  it("holds its place with an empty row for a repo with no other worktree", () => {
    renderList({ worktreeLeaves: [], worktreeBranchSet: new Set() })
    expect(optionLabels()).toContain("Worktrees (0)")
    expect(screen.getByText("No worktrees")).toBeInTheDocument()
  })

  it("offers switch and the worktree removals on a worktree row", async () => {
    const user = userEvent.setup()
    const { onLeafAction } = renderList()
    // The first "task/132" row is the worktree section's copy.
    await user.click(screen.getAllByRole("option", { name: "task/132" })[0])
    const bubble = screen.getByRole("menu", { name: "task/132" })
    expect(
      within(bubble)
        .getAllByRole("menuitem")
        .map((el) => el.textContent?.trim())
    ).toEqual([
      "Switch to this branch",
      "Merge task/132 into main",
      "Rebase main onto task/132",
      "Pull code",
      "Push",
      // Never plain "Delete branch": git refuses while a worktree holds the ref.
      "Delete worktree",
      "Delete worktree and branch",
    ])
    await user.click(within(bubble).getByRole("menuitem", { name: /Switch/ }))
    expect(onLeafAction).toHaveBeenCalledWith("switch", "task/132", false)
  })

  it("keeps searching across both sections", async () => {
    const user = userEvent.setup()
    renderList()
    await user.type(screen.getByRole("combobox"), "task")
    const labels = optionLabels()
    expect(labels).toEqual([
      "Worktrees (1)",
      "task/132",
      "Local branches (1)",
      "task/132",
    ])
  })

  it("collapses the section without touching the local tree", async () => {
    const user = userEvent.setup()
    renderList()
    await user.click(screen.getByRole("option", { name: "Worktrees (2)" }))
    const labels = optionLabels()
    expect(labels).toEqual([
      "Pull code",
      "Worktrees (2)",
      "Local branches (4)",
      // The local tree still lists both worktree branches.
      "feature/login",
      "loop/x",
      // The checked-out branch and its adjacent "Current" badge, unspaced.
      "mainCurrent",
      "task/132",
      "Remote branches (0)",
    ])
  })
})
