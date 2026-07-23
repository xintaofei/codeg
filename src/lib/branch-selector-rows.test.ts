import { describe, expect, it } from "vitest"

import {
  buildBranchRows,
  isNavigableRow,
  type BranchOperationMeta,
  type BranchRow,
  type BuildBranchRowsInput,
} from "@/lib/branch-selector-rows"
import {
  buildBranchTree,
  buildRemoteBranchSections,
  localBranchItems,
  sectionKey,
  type BranchTreeNode,
} from "@/lib/branch-tree"

const OPS: BranchOperationMeta[] = [
  { id: "pull", label: "Pull code" },
  { id: "push", label: "Push..." },
]

function localTree(names: string[]): BranchTreeNode[] {
  return buildBranchTree(localBranchItems(names), "local")
}

// Compact, readable shape for sequence assertions.
function summarize(row: BranchRow): string {
  switch (row.kind) {
    case "operation":
      return `op:${row.opId}`
    case "separator":
      return "sep"
    case "section":
      return `section:${row.scope}(${row.count})${row.expanded ? "+" : "-"}`
    case "group":
      return `group:${row.label}@${row.depth}${row.expanded ? "+" : "-"}`
    case "leaf":
      return `leaf:${row.fullName}@${row.depth}${row.isCurrent ? "*" : ""}`
    case "empty":
      return `empty:${row.scope}`
  }
}

function baseInput(
  overrides: Partial<BuildBranchRowsInput> = {}
): BuildBranchRowsInput {
  return {
    operations: OPS,
    localNodes: [],
    remoteSections: [],
    localCount: 0,
    remoteCount: 0,
    branch: null,
    worktreeBranchSet: new Set(),
    collapsed: new Set(),
    query: "",
    ...overrides,
  }
}

const LOCAL = [
  "main",
  "feature/auth/login",
  "feature/auth/logout",
  "release/1.0",
]

describe("buildBranchRows — empty query (tree mode)", () => {
  it("puts operations first, a separator, then default-expanded sections + flattened tree", () => {
    const rows = buildBranchRows(
      baseInput({ localNodes: localTree(LOCAL), localCount: 4, remoteCount: 0 })
    )
    expect(rows.map(summarize)).toEqual([
      "op:pull",
      "op:push",
      "sep",
      "section:local(4)+",
      "group:feature/auth/@1+",
      "leaf:feature/auth/login@2",
      "leaf:feature/auth/logout@2",
      "leaf:main@1",
      "leaf:release/1.0@1",
      "section:remote(0)+",
      "empty:remote",
    ])
  })

  it("collapsing the local section hides all its children", () => {
    const rows = buildBranchRows(
      baseInput({
        localNodes: localTree(LOCAL),
        localCount: 4,
        collapsed: new Set([sectionKey("local")]),
      })
    )
    expect(rows.map(summarize)).toEqual([
      "op:pull",
      "op:push",
      "sep",
      "section:local(4)-",
      "section:remote(0)+",
      "empty:remote",
    ])
  })

  it("collapsing a prefix group hides only that subtree", () => {
    const nodes = localTree(LOCAL)
    const groupKey = nodes.find((n) => n.type === "group")!.key
    const rows = buildBranchRows(
      baseInput({
        localNodes: nodes,
        localCount: 4,
        collapsed: new Set([groupKey]),
      })
    )
    expect(rows.map(summarize)).toEqual([
      "op:pull",
      "op:push",
      "sep",
      "section:local(4)+",
      "group:feature/auth/@1-",
      "leaf:main@1",
      "leaf:release/1.0@1",
      "section:remote(0)+",
      "empty:remote",
    ])
  })
})

describe("buildBranchRows — leaf flags (drive the action bubble)", () => {
  it("emits no leaf-action rows — actions live in the right-side bubble now", () => {
    const rows = buildBranchRows(
      baseInput({ localNodes: localTree(LOCAL), localCount: 4, branch: "main" })
    )
    expect(rows.map(summarize)).toContain("leaf:main@1*")
    const validKinds = new Set([
      "operation",
      "separator",
      "section",
      "group",
      "leaf",
      "empty",
    ])
    expect(rows.every((r) => validKinds.has(r.kind))).toBe(true)
  })

  it("marks the remote leaf that tracks the current local branch (bubble hides its delete)", () => {
    const remoteSections = buildRemoteBranchSections([
      "origin/main",
      "origin/dev",
    ])
    const rows = buildBranchRows(
      baseInput({ remoteSections, remoteCount: 2, branch: "main" })
    )
    const tracking = rows.find(
      (r) => r.kind === "leaf" && r.fullName === "origin/main"
    )
    const other = rows.find(
      (r) => r.kind === "leaf" && r.fullName === "origin/dev"
    )
    expect(tracking).toMatchObject({ isTracking: true })
    expect(other).toMatchObject({ isTracking: false })
  })
})

describe("buildBranchRows — operation grouping separators", () => {
  it("inserts a separator after each groupEnd op (non-search)", () => {
    const ops: BranchOperationMeta[] = [
      { id: "pull", label: "Pull code" },
      { id: "fetch", label: "Fetch", groupEnd: true },
      { id: "commit", label: "Commit" },
    ]
    const rows = buildBranchRows(
      baseInput({
        operations: ops,
        localNodes: localTree(["main"]),
        localCount: 1,
      })
    )
    // pull, fetch, SEP(groupEnd), commit, SEP(ops↔branches), then branches.
    expect(rows.slice(0, 5).map(summarize)).toEqual([
      "op:pull",
      "op:fetch",
      "sep",
      "op:commit",
      "sep",
    ])
  })

  it("omits group separators while searching", () => {
    const ops: BranchOperationMeta[] = [
      { id: "pull", label: "Pull code" },
      { id: "fetch", label: "Fetch pull", groupEnd: true },
    ]
    const rows = buildBranchRows(baseInput({ operations: ops, query: "pull" }))
    expect(rows.map(summarize)).toEqual(["op:pull", "op:fetch"])
  })
})

describe("buildBranchRows — search mode", () => {
  it("flattens matched leaves under section headers, dropping groups", () => {
    const rows = buildBranchRows(
      baseInput({
        localNodes: localTree(LOCAL),
        localCount: 4,
        query: "feature",
      })
    )
    // "feature" matches no operation label, so no ops block and no separator.
    expect(rows.map(summarize)).toEqual([
      "section:local(2)+",
      "leaf:feature/auth/login@1",
      "leaf:feature/auth/logout@1",
    ])
  })

  it("filters operations by label and omits the branch side when nothing matches", () => {
    const rows = buildBranchRows(
      baseInput({ localNodes: localTree(LOCAL), localCount: 4, query: "push" })
    )
    expect(rows.map(summarize)).toEqual(["op:push"])
  })

  it("keeps the separator when both an operation and branches match", () => {
    const rows = buildBranchRows(
      baseInput({ localNodes: localTree(LOCAL), localCount: 4, query: "e" })
    )
    // "Pull code" matches "e"; "Push..." does not. "main" has no "e".
    expect(rows.map(summarize)).toEqual([
      "op:pull",
      "sep",
      "section:local(3)+",
      "leaf:feature/auth/login@1",
      "leaf:feature/auth/logout@1",
      "leaf:release/1.0@1",
    ])
  })
})

describe("buildBranchRows — multiple remotes", () => {
  it("nests each remote as a wrapper group one level deeper", () => {
    const remoteSections = buildRemoteBranchSections([
      "origin/main",
      "upstream/main",
      "origin/dev",
    ])
    const rows = buildBranchRows(
      baseInput({ operations: [], remoteSections, remoteCount: 3 })
    )
    expect(rows.map(summarize)).toEqual([
      "section:local(0)+",
      "empty:local",
      "section:remote(3)+",
      "group:origin@1+",
      "leaf:origin/dev@2",
      "leaf:origin/main@2",
      "group:upstream@1+",
      "leaf:upstream/main@2",
    ])
  })

  it("collapsing a remote wrapper hides just that remote's branches", () => {
    const remoteSections = buildRemoteBranchSections([
      "origin/main",
      "upstream/main",
    ])
    const originKey = remoteSections.find((s) => s.remoteName === "origin")!.key
    const rows = buildBranchRows(
      baseInput({
        operations: [],
        remoteSections,
        remoteCount: 2,
        collapsed: new Set([originKey]),
      })
    )
    expect(rows.map(summarize)).toEqual([
      "section:local(0)+",
      "empty:local",
      "section:remote(2)+",
      "group:origin@1-",
      "group:upstream@1+",
      "leaf:upstream/main@2",
    ])
  })
})

describe("isNavigableRow", () => {
  it("skips separators and empty rows, keeps everything else", () => {
    expect(isNavigableRow({ kind: "separator", key: "s" })).toBe(false)
    expect(isNavigableRow({ kind: "empty", key: "e", scope: "local" })).toBe(
      false
    )
    expect(
      isNavigableRow({
        kind: "operation",
        key: "o",
        opId: "pull",
        label: "Pull",
        destructive: false,
      })
    ).toBe(true)
  })
})
