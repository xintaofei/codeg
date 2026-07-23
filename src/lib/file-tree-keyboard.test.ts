import { describe, expect, it } from "vitest"

import {
  buildVisibleTreeRows,
  resolveTreeKeyboardAction,
  type VisibleTreeRow,
} from "@/lib/file-tree-keyboard"
import type { FileTreeNode } from "@/lib/types"

const ROOT = "__root__"

//   src/
//     a.ts
//     utils/
//       b.ts
//   README.md
const NODES: FileTreeNode[] = [
  {
    kind: "dir",
    name: "src",
    path: "src",
    children: [
      { kind: "file", name: "a.ts", path: "src/a.ts" },
      {
        kind: "dir",
        name: "utils",
        path: "src/utils",
        children: [{ kind: "file", name: "b.ts", path: "src/utils/b.ts" }],
      },
    ],
  },
  { kind: "file", name: "README.md", path: "README.md" },
]

function rows(...expanded: string[]): VisibleTreeRow[] {
  return buildVisibleTreeRows(NODES, new Set(expanded), ROOT)
}

const paths = (r: VisibleTreeRow[]) => r.map((row) => row.path)

describe("buildVisibleTreeRows", () => {
  it("shows only the root row when the root is collapsed", () => {
    const r = rows()
    expect(paths(r)).toEqual([ROOT])
    expect(r[0]).toMatchObject({ parentPath: null, isExpanded: false })
  })

  it("descends only into expanded directories, in render order", () => {
    expect(paths(rows(ROOT))).toEqual([ROOT, "src", "README.md"])
    expect(paths(rows(ROOT, "src"))).toEqual([
      ROOT,
      "src",
      "src/a.ts",
      "src/utils",
      "README.md",
    ])
    expect(paths(rows(ROOT, "src", "src/utils"))).toEqual([
      ROOT,
      "src",
      "src/a.ts",
      "src/utils",
      "src/utils/b.ts",
      "README.md",
    ])
  })

  it("records parentPath and isExpanded per row", () => {
    const byPath = new Map(
      rows(ROOT, "src", "src/utils").map((row) => [row.path, row])
    )
    expect(byPath.get("src")).toMatchObject({
      parentPath: ROOT,
      kind: "dir",
      isExpanded: true,
    })
    expect(byPath.get("src/a.ts")).toMatchObject({
      parentPath: "src",
      kind: "file",
      isExpanded: false,
    })
    expect(byPath.get("src/utils/b.ts")).toMatchObject({
      parentPath: "src/utils",
      kind: "file",
    })
    expect(byPath.get("README.md")).toMatchObject({ parentPath: ROOT })
  })
})

describe("resolveTreeKeyboardAction — vertical movement", () => {
  const all = rows(ROOT, "src", "src/utils")

  it("selects the root when nothing is focused yet", () => {
    expect(resolveTreeKeyboardAction("ArrowDown", all, undefined)).toEqual({
      kind: "focus",
      path: ROOT,
    })
    expect(resolveTreeKeyboardAction("ArrowUp", all, undefined)).toEqual({
      kind: "focus",
      path: ROOT,
    })
  })

  it("moves to the next/previous visible row", () => {
    expect(resolveTreeKeyboardAction("ArrowDown", all, ROOT)).toEqual({
      kind: "focus",
      path: "src",
    })
    expect(resolveTreeKeyboardAction("ArrowUp", all, "src")).toEqual({
      kind: "focus",
      path: ROOT,
    })
  })

  it("clamps at the ends", () => {
    expect(resolveTreeKeyboardAction("ArrowUp", all, ROOT)).toEqual({
      kind: "focus",
      path: ROOT,
    })
    expect(resolveTreeKeyboardAction("ArrowDown", all, "README.md")).toEqual({
      kind: "focus",
      path: "README.md",
    })
  })

  it("supports Home and End", () => {
    expect(resolveTreeKeyboardAction("Home", all, "README.md")).toEqual({
      kind: "focus",
      path: ROOT,
    })
    expect(resolveTreeKeyboardAction("End", all, ROOT)).toEqual({
      kind: "focus",
      path: "README.md",
    })
  })

  it("falls back to the first row when the focused path is no longer visible", () => {
    expect(resolveTreeKeyboardAction("ArrowDown", all, "ghost/path")).toEqual({
      kind: "focus",
      path: ROOT,
    })
  })
})

describe("resolveTreeKeyboardAction — ArrowRight", () => {
  it("expands a collapsed directory in place", () => {
    const r = rows(ROOT) // src collapsed
    expect(resolveTreeKeyboardAction("ArrowRight", r, "src")).toEqual({
      kind: "expand",
      path: "src",
    })
  })

  it("steps into the first child of an expanded directory", () => {
    const r = rows(ROOT, "src")
    expect(resolveTreeKeyboardAction("ArrowRight", r, "src")).toEqual({
      kind: "focus",
      path: "src/a.ts",
    })
  })

  it("is a no-op on a file", () => {
    const r = rows(ROOT, "src")
    expect(resolveTreeKeyboardAction("ArrowRight", r, "src/a.ts")).toEqual({
      kind: "noop",
      path: "",
    })
  })

  it("is a no-op on an expanded directory whose children are not loaded yet", () => {
    const emptyDir: FileTreeNode[] = [
      { kind: "dir", name: "lazy", path: "lazy", children: [] },
    ]
    const r = buildVisibleTreeRows(emptyDir, new Set([ROOT, "lazy"]), ROOT)
    expect(resolveTreeKeyboardAction("ArrowRight", r, "lazy")).toEqual({
      kind: "noop",
      path: "",
    })
  })
})

describe("resolveTreeKeyboardAction — ArrowLeft", () => {
  it("collapses an expanded directory in place", () => {
    const r = rows(ROOT, "src")
    expect(resolveTreeKeyboardAction("ArrowLeft", r, "src")).toEqual({
      kind: "collapse",
      path: "src",
    })
  })

  it("jumps to the parent from a collapsed directory", () => {
    const r = rows(ROOT) // src collapsed
    expect(resolveTreeKeyboardAction("ArrowLeft", r, "src")).toEqual({
      kind: "focus",
      path: ROOT,
    })
  })

  it("jumps to the parent from a file", () => {
    const r = rows(ROOT, "src")
    expect(resolveTreeKeyboardAction("ArrowLeft", r, "src/a.ts")).toEqual({
      kind: "focus",
      path: "src",
    })
  })

  it("collapses the expanded root", () => {
    const r = rows(ROOT)
    expect(resolveTreeKeyboardAction("ArrowLeft", r, ROOT)).toEqual({
      kind: "collapse",
      path: ROOT,
    })
  })

  it("is a no-op on the collapsed root (no parent to move to)", () => {
    const r = rows() // root collapsed
    expect(resolveTreeKeyboardAction("ArrowLeft", r, ROOT)).toEqual({
      kind: "noop",
      path: "",
    })
  })
})

describe("resolveTreeKeyboardAction — activation & unhandled keys", () => {
  const all = rows(ROOT, "src", "src/utils")

  it("opens a focused file on Enter and Space", () => {
    expect(resolveTreeKeyboardAction("Enter", all, "src/a.ts")).toEqual({
      kind: "open",
      path: "src/a.ts",
    })
    expect(resolveTreeKeyboardAction(" ", all, "src/a.ts")).toEqual({
      kind: "open",
      path: "src/a.ts",
    })
  })

  it("toggles a focused directory on Enter", () => {
    expect(resolveTreeKeyboardAction("Enter", all, "src")).toEqual({
      kind: "toggle",
      path: "src",
    })
  })

  it("returns null for keys the tree does not own", () => {
    expect(resolveTreeKeyboardAction("a", all, "src")).toBeNull()
    expect(resolveTreeKeyboardAction("Tab", all, "src")).toBeNull()
    expect(resolveTreeKeyboardAction("Escape", all, "src")).toBeNull()
  })

  it("returns null when there are no rows", () => {
    expect(resolveTreeKeyboardAction("ArrowDown", [], undefined)).toBeNull()
  })
})
