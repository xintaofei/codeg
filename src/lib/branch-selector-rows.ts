/**
 * Flat, virtualization-ready row model for the branch selector popup.
 *
 * The rich branch selector (`BranchDropdown`) renders operations (pull / fetch /
 * commit / push / new branch / worktree / stash / manage remotes) AND the full
 * local+remote branch tree as ONE searchable, virtualized, flat list — mirroring
 * the model picker's `flattenModelGroups` + `ModelOptionList` split. This module
 * is the pure half: it flattens the prefix-grouped {@link BranchTreeNode} trees
 * (from `@/lib/branch-tree`) plus the operation metadata into a linear
 * `BranchRow[]` the renderer maps 1:1 to virtua rows.
 *
 * Deliberately pure — no React, no callbacks, no icons, no i18n. The renderer
 * resolves icons/handlers by `kind`/`opId` and builds every translated string
 * (section headers by `scope`+`count`), so all display concerns stay out of
 * here and the flattening logic is unit-testable in isolation.
 */

import { sectionKey } from "@/lib/branch-tree"
import type {
  BranchTreeLeaf,
  BranchTreeNode,
  RemoteBranchSection,
} from "@/lib/branch-tree"

/** Container-supplied operation, resolved to icon + handler by the renderer. */
export interface BranchOperationMeta {
  id: string
  /** Already-translated label — the ONLY string search matches operations on. */
  label: string
  destructive?: boolean
  /** Emit a separator after this op (non-search) to visually group operations. */
  groupEnd?: boolean
}

export type BranchLeafAction =
  | "switch"
  | "merge"
  | "rebase"
  | "delete"
  | "deleteRemote"

export type BranchRow =
  | {
      kind: "operation"
      key: string
      opId: string
      label: string
      destructive: boolean
    }
  | { kind: "separator"; key: string }
  | {
      kind: "section"
      key: string
      scope: "local" | "remote"
      count: number
      expanded: boolean
    }
  | {
      kind: "group"
      key: string
      depth: number
      label: string
      count: number
      expanded: boolean
    }
  | {
      kind: "leaf"
      key: string
      depth: number
      fullName: string
      label: string
      isRemote: boolean
      isCurrent: boolean
      isTracking: boolean
      isWorktree: boolean
    }
  | { kind: "empty"; key: string; scope: "local" | "remote" }

export interface BuildBranchRowsInput {
  operations: BranchOperationMeta[]
  localNodes: BranchTreeNode[]
  remoteSections: RemoteBranchSection[]
  /** Total local branch count (for the section header's "(n)"). */
  localCount: number
  /** Total remote branch count (for the section header's "(n)"). */
  remoteCount: number
  /** Current branch ref (marks the current leaf, suppresses its actions). */
  branch: string | null
  /** Branch names checked out in a worktree — leaf gets the worktree icon. */
  worktreeBranchSet: Set<string>
  /** Group/section keys the user has collapsed (default-expanded = absent). */
  collapsed: Set<string>
  /** Search query; when non-empty the tree flattens to matched leaves. */
  query: string
}

const localSectionKey = sectionKey("local")
const remoteSectionKey = sectionKey("remote")

interface LeafContext {
  branch: string | null
  worktreeBranchSet: Set<string>
  collapsed: Set<string>
}

/** Strip a remote leaf's `<remote>/` prefix (local leaves are returned as-is). */
function localName(fullName: string, isRemote: boolean): string {
  return isRemote ? fullName.replace(/^[^/]+\//, "") : fullName
}

/**
 * Emit a single leaf row. Per-branch actions (switch/merge/rebase/delete) are
 * NOT rows — the renderer shows them in a right-side bubble when a leaf is
 * clicked (`isTracking` there hides delete for the tracked remote branch).
 */
function emitLeaf(
  out: BranchRow[],
  leaf: BranchTreeLeaf,
  depth: number,
  isRemote: boolean,
  ctx: LeafContext
): void {
  const b = leaf.fullName
  const isCurrent = b === ctx.branch
  const isTracking =
    isRemote && !!ctx.branch && localName(b, true) === ctx.branch
  const isWorktree = ctx.worktreeBranchSet.has(localName(b, isRemote))

  out.push({
    kind: "leaf",
    key: leaf.key,
    depth,
    fullName: b,
    label: leaf.label,
    isRemote,
    isCurrent,
    isTracking,
    isWorktree,
  })
}

/** Recursively flatten a prefix tree, descending only expanded groups. */
function emitTree(
  out: BranchRow[],
  nodes: BranchTreeNode[],
  depth: number,
  isRemote: boolean,
  ctx: LeafContext
): void {
  for (const node of nodes) {
    if (node.type === "leaf") {
      emitLeaf(out, node, depth, isRemote, ctx)
      continue
    }
    const expanded = !ctx.collapsed.has(node.key)
    out.push({
      kind: "group",
      key: node.key,
      depth,
      label: node.label,
      count: node.count,
      expanded,
    })
    if (expanded) emitTree(out, node.children, depth + 1, isRemote, ctx)
  }
}

/** All leaf descendants of `nodes`, in render order. */
function collectLeaves(nodes: BranchTreeNode[]): BranchTreeLeaf[] {
  const leaves: BranchTreeLeaf[] = []
  const walk = (list: BranchTreeNode[]) => {
    for (const node of list) {
      if (node.type === "leaf") leaves.push(node)
      else walk(node.children)
    }
  }
  walk(nodes)
  return leaves
}

function matchesLeaf(leaf: BranchTreeLeaf, q: string): boolean {
  return (
    leaf.fullName.toLowerCase().includes(q) ||
    leaf.label.toLowerCase().includes(q)
  )
}

/**
 * Flatten operations + branch trees into a single linear row list.
 *
 * - Empty query: operations block → separator → Local section (its prefix tree,
 *   descending only expanded groups) → Remote section (single-remote strips the
 *   wrapper; multi-remote nests each remote as a group). Sections default open.
 * - Non-empty query: operations whose label matches → separator → matched local
 *   leaves and matched remote leaves, flat under their section headers (groups
 *   dropped, collapse state ignored); empty sections omitted.
 *
 * Indentation depth: operations flat; a section header is depth 0; its children
 * are depth 1 (and deeper per nesting).
 */
export function buildBranchRows(input: BuildBranchRowsInput): BranchRow[] {
  const {
    operations,
    localNodes,
    remoteSections,
    localCount,
    remoteCount,
    branch,
    worktreeBranchSet,
    collapsed,
    query,
  } = input

  const q = query.trim().toLowerCase()
  const searching = q.length > 0
  const ctx: LeafContext = {
    branch,
    worktreeBranchSet,
    collapsed,
  }

  const rows: BranchRow[] = []

  // --- Operations ------------------------------------------------------------
  // Grouped by a separator after each `groupEnd` op (non-search only) to mirror
  // the old menu's pull/fetch | commit/push | … blocks.
  for (const op of operations) {
    if (searching && !op.label.toLowerCase().includes(q)) continue
    rows.push({
      kind: "operation",
      key: `op:${op.id}`,
      opId: op.id,
      label: op.label,
      destructive: op.destructive ?? false,
    })
    if (!searching && op.groupEnd) {
      rows.push({ kind: "separator", key: `sep:op:${op.id}` })
    }
  }
  const hasOperations = rows.some((row) => row.kind === "operation")

  // --- Branches --------------------------------------------------------------
  const branchRows: BranchRow[] = []

  if (searching) {
    const localMatches = collectLeaves(localNodes).filter((l) =>
      matchesLeaf(l, q)
    )
    if (localMatches.length > 0) {
      branchRows.push({
        kind: "section",
        key: localSectionKey,
        scope: "local",
        count: localMatches.length,
        expanded: true,
      })
      for (const leaf of localMatches) emitLeaf(branchRows, leaf, 1, false, ctx)
    }

    const remoteMatches: BranchTreeLeaf[] = []
    for (const section of remoteSections) {
      for (const leaf of collectLeaves(section.nodes)) {
        if (matchesLeaf(leaf, q)) remoteMatches.push(leaf)
      }
    }
    if (remoteMatches.length > 0) {
      branchRows.push({
        kind: "section",
        key: remoteSectionKey,
        scope: "remote",
        count: remoteMatches.length,
        expanded: true,
      })
      for (const leaf of remoteMatches) emitLeaf(branchRows, leaf, 1, true, ctx)
    }
  } else {
    // Local section
    const localExpanded = !collapsed.has(localSectionKey)
    branchRows.push({
      kind: "section",
      key: localSectionKey,
      scope: "local",
      count: localCount,
      expanded: localExpanded,
    })
    if (localExpanded) {
      if (localNodes.length === 0) {
        branchRows.push({ kind: "empty", key: "empty:local", scope: "local" })
      } else {
        emitTree(branchRows, localNodes, 1, false, ctx)
      }
    }

    // Remote section
    const remoteExpanded = !collapsed.has(remoteSectionKey)
    branchRows.push({
      kind: "section",
      key: remoteSectionKey,
      scope: "remote",
      count: remoteCount,
      expanded: remoteExpanded,
    })
    if (remoteExpanded) {
      if (remoteCount === 0) {
        branchRows.push({ kind: "empty", key: "empty:remote", scope: "remote" })
      } else {
        for (const section of remoteSections) {
          if (section.remoteName == null) {
            emitTree(branchRows, section.nodes, 1, true, ctx)
            continue
          }
          // Multiple remotes: each remote is a wrapper group toggled by its
          // own section key, its branches nested one level deeper.
          const wrapperExpanded = !collapsed.has(section.key)
          branchRows.push({
            kind: "group",
            key: section.key,
            depth: 1,
            label: section.remoteName,
            count: section.count,
            expanded: wrapperExpanded,
          })
          if (wrapperExpanded) {
            emitTree(branchRows, section.nodes, 2, true, ctx)
          }
        }
      }
    }
  }

  if (hasOperations && branchRows.length > 0) {
    rows.push({ kind: "separator", key: "sep:ops-branches" })
  }
  rows.push(...branchRows)

  return rows
}

/** Row kinds the keyboard cursor can land on (skips separators + empty rows). */
export function isNavigableRow(row: BranchRow): boolean {
  return row.kind !== "separator" && row.kind !== "empty"
}
