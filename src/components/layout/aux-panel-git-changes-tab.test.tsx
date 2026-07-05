import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

import {
  capChangeTreeToBudget,
  type ChangeTreeNode,
  countVisibleChangeRows,
  GitChangesTab,
  MAX_VISIBLE_ROWS,
} from "./aux-panel-git-changes-tab"
import enMessages from "@/i18n/messages/en.json"

// ---------------------------------------------------------------------------
// Pure helpers — the cap algorithm, tested without React.
// ---------------------------------------------------------------------------

const file = (name: string, path = name): ChangeTreeNode => ({
  kind: "file",
  name,
  path,
  change: { path, status: "M", additions: 0, deletions: 0 },
})

const dir = (
  name: string,
  children: ChangeTreeNode[],
  path = name
): ChangeTreeNode => ({ kind: "dir", name, path, children, fileCount: 0 })

describe("countVisibleChangeRows", () => {
  it("counts one row per flat file", () => {
    expect(
      countVisibleChangeRows([file("a"), file("b"), file("c")], new Set())
    ).toBe(3)
  })

  it("does not count a collapsed directory's children", () => {
    const tree = [dir("A", [file("A/1", "A/1"), file("A/2", "A/2")], "A")]
    // A collapsed → only A's own row counts.
    expect(countVisibleChangeRows(tree, new Set())).toBe(1)
  })

  it("counts an expanded directory's children", () => {
    const tree = [dir("A", [file("A/1", "A/1"), file("A/2", "A/2")], "A")]
    expect(countVisibleChangeRows(tree, new Set(["A"]))).toBe(3)
  })

  it("recurses only through the expanded path", () => {
    const tree = [
      dir(
        "A",
        [dir("B", [file("A/B/1", "A/B/1")], "A/B"), file("A/2", "A/2")],
        "A"
      ),
    ]
    // A expanded, B collapsed: A + B + A/2 = 3 (B's child hidden).
    expect(countVisibleChangeRows(tree, new Set(["A"]))).toBe(3)
    // Both expanded: A + B + A/B/1 + A/2 = 4.
    expect(countVisibleChangeRows(tree, new Set(["A", "A/B"]))).toBe(4)
  })
})

describe("capChangeTreeToBudget", () => {
  it("keeps only the first `budget` rows in display order", () => {
    const nodes = [file("a"), file("b"), file("c"), file("d"), file("e")]
    const { nodes: capped, remaining } = capChangeTreeToBudget(
      nodes,
      new Set(),
      2
    )
    expect(capped.map((n) => n.name)).toEqual(["a", "b"])
    expect(remaining).toBe(0)
  })

  it("keeps a collapsed directory's children intact without spending budget on them", () => {
    const big = dir(
      "A",
      Array.from({ length: 10 }, (_, i) => file(`A/${i}`, `A/${i}`)),
      "A"
    )
    const nodes = [big, file("x"), file("y")]
    // A is collapsed (not in the expanded set): A, x, y = 3 rows fit the budget.
    const { nodes: capped, remaining } = capChangeTreeToBudget(
      nodes,
      new Set(),
      3
    )
    expect(capped).toHaveLength(3)
    const first = capped[0]
    expect(first.kind).toBe("dir")
    // Children are preserved so expanding the directory later still works.
    expect(first.kind === "dir" && first.children).toHaveLength(10)
    expect(remaining).toBe(0)
  })

  it("spends budget on an expanded directory's children", () => {
    const tree = [
      dir(
        "A",
        [file("A/1", "A/1"), file("A/2", "A/2"), file("A/3", "A/3")],
        "A"
      ),
    ]
    // Budget 2: the directory row + its first child.
    const { nodes: capped } = capChangeTreeToBudget(tree, new Set(["A"]), 2)
    expect(capped).toHaveLength(1)
    const only = capped[0]
    expect(only.kind === "dir" && only.children.map((c) => c.name)).toEqual([
      "A/1",
    ])
  })

  it("returns everything when the budget exceeds the total", () => {
    const nodes = [file("a"), file("b"), file("c")]
    const { nodes: capped, remaining } = capChangeTreeToBudget(
      nodes,
      new Set(),
      10
    )
    expect(capped).toHaveLength(3)
    expect(remaining).toBe(7)
  })
})

// ---------------------------------------------------------------------------
// Component — the cap wiring, with Radix trees mocked to lightweight
// passthroughs so we can render hundreds of rows cheaply. The cap logic is
// independent of the file-tree / context-menu internals.
// ---------------------------------------------------------------------------

const store = vi.hoisted(() => ({
  git: [] as {
    path: string
    status: string
    additions: number
    deletions: number
  }[],
}))

vi.mock("@/hooks/use-workspace-state-store", () => ({
  useWorkspaceStateStore: () => ({
    rootPath: "/repo",
    seq: 1,
    version: 1,
    health: "healthy",
    tree: [],
    git: store.git,
    error: null,
    degraded: false,
    isGitRepo: true,
    requestResync: async () => {},
    restart: async () => {},
    subscribeEnvelopes: () => () => {},
  }),
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: { id: "f1", path: "/repo" } }),
}))

vi.mock("@/contexts/tab-context", () => ({
  useTabStore: (selector: (s: { tabs: []; activeTabId: null }) => unknown) =>
    selector({ tabs: [], activeTabId: null }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceActions: () => ({
    openFilePreview: vi.fn(),
    openWorkingTreeDiff: vi.fn(),
  }),
}))

// Radix context menu → passthrough trigger, no lazy content.
vi.mock("@/components/ui/context-menu", () => ({
  ContextMenu: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  ContextMenuTrigger: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
  ContextMenuContent: () => null,
  ContextMenuItem: () => null,
}))

// File tree → render name + children directly (expansion is irrelevant to the
// cap for a flat root, and the component already passes pre-capped nodes).
vi.mock("@/components/ai-elements/file-tree", () => ({
  FileTree: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  FileTreeFolder: ({
    name,
    suffix,
    children,
  }: {
    name: string
    suffix?: React.ReactNode
    children: React.ReactNode
  }) => (
    <div>
      <span>{name}</span>
      {suffix}
      {children}
    </div>
  ),
  FileTreeFile: ({
    name,
    children,
  }: {
    name: string
    children?: React.ReactNode
  }) => <div>{children ?? name}</div>,
}))

function id(n: number): string {
  return `f${String(n).padStart(4, "0")}`
}

function flatGit(count: number) {
  return Array.from({ length: count }, (_, i) => ({
    path: id(i + 1),
    status: "M",
    additions: 1,
    deletions: 0,
  }))
}

function renderTab() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <GitChangesTab />
    </NextIntlClientProvider>
  )
}

describe("GitChangesTab render cap", () => {
  it("renders every row and no reveal control for a small change set", () => {
    store.git = flatGit(3)
    renderTab()

    expect(screen.getByText("f0001")).toBeInTheDocument()
    expect(screen.getByText("f0003")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more items/ })
    ).not.toBeInTheDocument()
  })

  it("builds an expanded directory's children", () => {
    // Tracked changes auto-expand every directory, so the nested files render.
    store.git = [
      { path: "pkg/a", status: "M", additions: 1, deletions: 0 },
      { path: "pkg/b", status: "M", additions: 1, deletions: 0 },
    ]
    renderTab()

    expect(screen.getByText("pkg")).toBeInTheDocument()
    expect(screen.getByText("a")).toBeInTheDocument()
    expect(screen.getByText("b")).toBeInTheDocument()
  })

  it("does not build a collapsed directory's children", () => {
    // Untracked changes default to a collapsed tree. The folder row renders,
    // but its children must not be constructed until it is expanded — otherwise
    // a huge collapsed subtree would still build thousands of elements.
    store.git = [
      { path: "pkg/a", status: "??", additions: 0, deletions: 0 },
      { path: "pkg/b", status: "??", additions: 0, deletions: 0 },
      { path: "pkg/c", status: "??", additions: 0, deletions: 0 },
    ]
    renderTab()

    expect(screen.getByText("pkg")).toBeInTheDocument()
    expect(screen.queryByText("a")).not.toBeInTheDocument()
    expect(screen.queryByText("c")).not.toBeInTheDocument()
  })

  it("caps a large change set and offers to reveal the rest", () => {
    const hidden = 50
    store.git = flatGit(MAX_VISIBLE_ROWS + hidden)
    renderTab()

    expect(screen.getByText(id(MAX_VISIBLE_ROWS))).toBeInTheDocument()
    expect(screen.queryByText(id(MAX_VISIBLE_ROWS + 1))).not.toBeInTheDocument()
    expect(
      screen.queryByText(id(MAX_VISIBLE_ROWS + hidden))
    ).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: `Show ${hidden} more items` })
    ).toBeInTheDocument()
  })

  it("reveals all rows once expanded", () => {
    const hidden = 50
    store.git = flatGit(MAX_VISIBLE_ROWS + hidden)
    renderTab()

    fireEvent.click(
      screen.getByRole("button", { name: `Show ${hidden} more items` })
    )

    expect(screen.getByText(id(MAX_VISIBLE_ROWS + hidden))).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /more items/ })
    ).not.toBeInTheDocument()
  })

  it("re-caps when a fresh, larger change set replaces the revealed one", () => {
    store.git = flatGit(MAX_VISIBLE_ROWS + 50)
    const { rerender } = renderTab()
    fireEvent.click(screen.getByRole("button", { name: "Show 50 more items" }))
    expect(screen.getByText(id(MAX_VISIBLE_ROWS + 50))).toBeInTheDocument()

    // A different, larger snapshot arrives (new git array identity). The reveal
    // must reset so the panel re-caps instead of mounting the whole tree.
    store.git = flatGit(MAX_VISIBLE_ROWS + 120)
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <GitChangesTab />
      </NextIntlClientProvider>
    )

    expect(
      screen.getByRole("button", { name: "Show 120 more items" })
    ).toBeInTheDocument()
    expect(screen.getByText(id(MAX_VISIBLE_ROWS))).toBeInTheDocument()
    expect(
      screen.queryByText(id(MAX_VISIBLE_ROWS + 120))
    ).not.toBeInTheDocument()
  })
})
