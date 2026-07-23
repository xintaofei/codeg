import type { FileTreeNode } from "@/lib/types"

/**
 * Keyboard navigation model for the workspace file tree.
 *
 * The tree is rendered by plain recursion (not virtualized), so a row exists in
 * the DOM only when every ancestor is expanded. To drive Up/Down/Left/Right
 * navigation we first flatten the visible rows into render order, then resolve a
 * single intent per key press. Both steps are pure so they can be unit-tested in
 * isolation from the (large, stateful) panel component that applies the intent.
 */

export interface VisibleTreeRow {
  path: string
  kind: "file" | "dir"
  /** The parent row's path, or `null` for the synthetic root row. */
  parentPath: string | null
  /** Directories only: whether currently expanded. Always `false` for files. */
  isExpanded: boolean
}

/**
 * Flatten the tree into the exact order rows are rendered, including the
 * synthetic root row (`rootPath`) that wraps the top-level nodes. Only expanded
 * directories are descended into, mirroring what is actually mounted.
 */
export function buildVisibleTreeRows(
  nodes: readonly FileTreeNode[],
  expandedPaths: ReadonlySet<string>,
  rootPath: string
): VisibleTreeRow[] {
  const rows: VisibleTreeRow[] = []
  const rootExpanded = expandedPaths.has(rootPath)
  rows.push({
    path: rootPath,
    kind: "dir",
    parentPath: null,
    isExpanded: rootExpanded,
  })

  const walk = (items: readonly FileTreeNode[], parentPath: string) => {
    for (const item of items) {
      if (item.kind === "file") {
        rows.push({
          path: item.path,
          kind: "file",
          parentPath,
          isExpanded: false,
        })
        continue
      }
      const expanded = expandedPaths.has(item.path)
      rows.push({
        path: item.path,
        kind: "dir",
        parentPath,
        isExpanded: expanded,
      })
      if (expanded) walk(item.children, item.path)
    }
  }

  // Top-level nodes are children of the root row, so they only show when the
  // root itself is expanded.
  if (rootExpanded) walk(nodes, rootPath)
  return rows
}

export type TreeKeyboardActionKind =
  | "focus"
  | "expand"
  | "collapse"
  | "toggle"
  | "open"
  | "noop"

export interface TreeKeyboardAction {
  kind: TreeKeyboardActionKind
  /** Target path for the action; empty for `noop`. */
  path: string
}

const NOOP: TreeKeyboardAction = { kind: "noop", path: "" }

/**
 * Resolve a key press against the visible rows into a single navigation intent
 * (IntelliJ IDEA-style project tree behavior):
 *
 * - `ArrowDown`/`ArrowUp` — move focus to the next/previous visible row (clamped).
 * - `ArrowRight` — on a collapsed directory: expand; on an expanded directory:
 *   move to its first visible child; on a file: no-op.
 * - `ArrowLeft` — on an expanded directory: collapse; otherwise move to the
 *   parent directory.
 * - `Home`/`End` — jump to the first/last visible row.
 * - `Enter`/`Space` — open a file, or toggle a directory.
 *
 * Returns `null` for keys the tree does not own (so the caller does not
 * `preventDefault`), or a `{ kind: "noop" }` action for owned keys that produce
 * no change (so the caller can still swallow the key to stop the scroll area
 * from scrolling).
 */
export function resolveTreeKeyboardAction(
  key: string,
  rows: readonly VisibleTreeRow[],
  currentPath: string | undefined
): TreeKeyboardAction | null {
  if (rows.length === 0) return null

  const currentIndex =
    currentPath == null ? -1 : rows.findIndex((row) => row.path === currentPath)
  const current = currentIndex >= 0 ? rows[currentIndex] : undefined

  switch (key) {
    case "ArrowDown": {
      const next =
        currentIndex < 0 ? 0 : Math.min(rows.length - 1, currentIndex + 1)
      return { kind: "focus", path: rows[next].path }
    }
    case "ArrowUp": {
      const prev = currentIndex <= 0 ? 0 : currentIndex - 1
      return { kind: "focus", path: rows[prev].path }
    }
    case "ArrowRight": {
      if (!current) return { kind: "focus", path: rows[0].path }
      if (current.kind !== "dir") return NOOP
      if (!current.isExpanded) return { kind: "expand", path: current.path }
      // Already expanded: step into the first child if one is currently visible
      // (it always follows immediately in render order).
      const child = rows[currentIndex + 1]
      if (child && child.parentPath === current.path) {
        return { kind: "focus", path: child.path }
      }
      return NOOP
    }
    case "ArrowLeft": {
      if (!current) return { kind: "focus", path: rows[0].path }
      if (current.kind === "dir" && current.isExpanded) {
        return { kind: "collapse", path: current.path }
      }
      if (current.parentPath != null) {
        return { kind: "focus", path: current.parentPath }
      }
      return NOOP
    }
    case "Home":
      return { kind: "focus", path: rows[0].path }
    case "End":
      return { kind: "focus", path: rows[rows.length - 1].path }
    case "Enter":
    case " ": {
      if (!current) return NOOP
      if (current.kind === "file") return { kind: "open", path: current.path }
      return { kind: "toggle", path: current.path }
    }
    default:
      return null
  }
}
