/**
 * Splitter-tree model for conversation tab groups (IDEA-style editor splits).
 *
 * Leaves are tab groups; internal nodes are splitters with an orientation and
 * one ratio per child (summing to 1). Splitting in the parent's own
 * orientation inserts a sibling (flattened N-way split, one shared divider
 * row); splitting across it nests a new binary splitter. The tree is kept
 * canonical: no empty splits, no single-child splits, no same-orientation
 * nesting.
 *
 * Everything here is pure and immutable — the tab store owns the state, the
 * detail panel renders groups as absolutely-positioned siblings from
 * `computeRects` so layout changes never reparent (and thus never remount) a
 * live conversation view.
 */

export type SplitOrientation = "horizontal" | "vertical"

export interface GroupLeaf {
  type: "group"
  id: string
}

export interface SplitNode {
  type: "split"
  id: string
  /** "horizontal" = children side by side; "vertical" = stacked. */
  orientation: SplitOrientation
  children: LayoutNode[]
  /** One fraction per child, > 0, summing to 1. */
  ratios: number[]
}

export type LayoutNode = GroupLeaf | SplitNode

export type SplitDirection = "right" | "down"

/** Group id of the initial (and post-reset) single-leaf layout. */
export const ROOT_GROUP_ID = "g-main"

/** Smallest fraction a child may occupy inside its split node. */
export const MIN_SPLIT_RATIO = 0.15

export function makeGroupId(): string {
  return `g-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export function singleGroupLayout(id: string = ROOT_GROUP_ID): GroupLeaf {
  return { type: "group", id }
}

/** Leaf (group) ids in traversal order — the canonical group numbering. */
export function leafIds(tree: LayoutNode): string[] {
  if (tree.type === "group") return [tree.id]
  return tree.children.flatMap(leafIds)
}

export function firstLeafId(tree: LayoutNode): string {
  return tree.type === "group" ? tree.id : firstLeafId(tree.children[0])
}

/** The split node a group directly lives in, or null for a root leaf. */
export function parentSplitOf(
  tree: LayoutNode,
  groupId: string
): SplitNode | null {
  if (tree.type === "group") return null
  for (const child of tree.children) {
    if (child.type === "group") {
      if (child.id === groupId) return tree
    } else {
      const found = parentSplitOf(child, groupId)
      if (found) return found
    }
  }
  return null
}

/** Merge target when a group is dissolved: previous leaf in traversal order,
 *  else the next one. Null when the group is alone (or unknown). */
export function neighborGroupId(
  tree: LayoutNode,
  groupId: string
): string | null {
  const ids = leafIds(tree)
  const idx = ids.indexOf(groupId)
  if (idx < 0 || ids.length < 2) return null
  return idx > 0 ? ids[idx - 1] : ids[idx + 1]
}

function normalizeRatios(ratios: number[]): number[] {
  const positive = ratios.map((r) => (Number.isFinite(r) && r > 0 ? r : 0))
  const sum = positive.reduce((a, b) => a + b, 0)
  if (sum <= 0) return ratios.map(() => 1 / ratios.length)
  return positive.map((r) => r / sum)
}

/**
 * Canonicalize structure: splice same-orientation child splits into their
 * parent (scaling ratios) and collapse single-child splits. Leaves untouched.
 */
function collapse(node: LayoutNode): LayoutNode {
  if (node.type === "group") return node
  const children: LayoutNode[] = []
  const ratios: number[] = []
  node.children.forEach((rawChild, i) => {
    const child = collapse(rawChild)
    const ratio = node.ratios[i] ?? 0
    if (child.type === "split" && child.orientation === node.orientation) {
      child.children.forEach((grand, j) => {
        children.push(grand)
        ratios.push(ratio * (child.ratios[j] ?? 0))
      })
    } else {
      children.push(child)
      ratios.push(ratio)
    }
  })
  if (children.length === 1) return children[0]
  return { ...node, children, ratios: normalizeRatios(ratios) }
}

/**
 * Split `groupId` toward `direction`, placing the new group `newGroupId` after
 * it. Same-orientation parent → sibling insert halving the split group's
 * share; otherwise the leaf is replaced by a binary split (deterministic node
 * id derived from the new group id). Unknown group → same tree reference.
 */
export function splitGroup(
  tree: LayoutNode,
  groupId: string,
  direction: SplitDirection,
  newGroupId: string
): LayoutNode {
  const orientation: SplitOrientation =
    direction === "right" ? "horizontal" : "vertical"

  const walk = (node: LayoutNode): LayoutNode => {
    if (node.type === "group") {
      if (node.id !== groupId) return node
      return {
        type: "split",
        id: `s-${newGroupId}`,
        orientation,
        children: [node, singleGroupLayout(newGroupId)],
        ratios: [0.5, 0.5],
      }
    }
    if (node.orientation === orientation) {
      const idx = node.children.findIndex(
        (c) => c.type === "group" && c.id === groupId
      )
      if (idx >= 0) {
        const children = [...node.children]
        const ratios = [...node.ratios]
        const share = ratios[idx] / 2
        ratios[idx] = share
        children.splice(idx + 1, 0, singleGroupLayout(newGroupId))
        ratios.splice(idx + 1, 0, share)
        return { ...node, children, ratios }
      }
    }
    let changed = false
    const children = node.children.map((child) => {
      const next = walk(child)
      if (next !== child) changed = true
      return next
    })
    return changed ? collapse({ ...node, children }) : node
  }

  if (!leafIds(tree).includes(groupId)) return tree
  return walk(tree)
}

/** Remove a group's leaf and re-canonicalize. Removing the last leaf (or an
 *  unknown id) returns the tree unchanged. */
export function removeGroup(tree: LayoutNode, groupId: string): LayoutNode {
  const prune = (node: LayoutNode): LayoutNode | null => {
    if (node.type === "group") return node.id === groupId ? null : node
    const children: LayoutNode[] = []
    const ratios: number[] = []
    node.children.forEach((child, i) => {
      const next = prune(child)
      if (next) {
        children.push(next)
        ratios.push(node.ratios[i] ?? 0)
      }
    })
    if (children.length === 0) return null
    if (children.length === 1) return children[0]
    return { ...node, children, ratios: normalizeRatios(ratios) }
  }

  const ids = leafIds(tree)
  if (!ids.includes(groupId) || ids.length < 2) return tree
  const pruned = prune(tree)
  return pruned ? collapse(pruned) : tree
}

/**
 * Drop every leaf not in `liveGroupIds`, then re-canonicalize. If nothing
 * survives, fall back to the tree's first leaf so the id stays stable for
 * device-local per-group state.
 */
export function normalizeTree(
  tree: LayoutNode,
  liveGroupIds: ReadonlySet<string>
): LayoutNode {
  const prune = (node: LayoutNode): LayoutNode | null => {
    if (node.type === "group") return liveGroupIds.has(node.id) ? node : null
    const children: LayoutNode[] = []
    const ratios: number[] = []
    node.children.forEach((child, i) => {
      const next = prune(child)
      if (next) {
        children.push(next)
        ratios.push(node.ratios[i] ?? 0)
      }
    })
    if (children.length === 0) return null
    if (children.length === 1) return children[0]
    return { ...node, children, ratios: normalizeRatios(ratios) }
  }

  const pruned = prune(tree)
  if (!pruned) return singleGroupLayout(firstLeafId(tree))
  const next = collapse(pruned)
  return sameTree(next, tree) ? tree : next
}

function sameTree(a: LayoutNode, b: LayoutNode): boolean {
  if (a === b) return true
  if (a.type === "group" || b.type === "group") {
    return a.type === "group" && b.type === "group" && a.id === b.id
  }
  return (
    a.id === b.id &&
    a.orientation === b.orientation &&
    a.children.length === b.children.length &&
    a.ratios.every((r, i) => r === b.ratios[i]) &&
    a.children.every((c, i) => sameTree(c, b.children[i]))
  )
}

/** Flip the orientation of the split node the group lives in (root leaf →
 *  unchanged), then re-canonicalize (the flip may enable a merge with the
 *  parent or children of the same orientation). */
export function toggleOrientation(
  tree: LayoutNode,
  groupId: string
): LayoutNode {
  const parent = parentSplitOf(tree, groupId)
  if (!parent) return tree
  const flipWalk = (node: LayoutNode): LayoutNode => {
    if (node.type === "group") return node
    if (node.id === parent.id) {
      return {
        ...node,
        orientation:
          node.orientation === "horizontal" ? "vertical" : "horizontal",
      }
    }
    let changed = false
    const children = node.children.map((child) => {
      const next = flipWalk(child)
      if (next !== child) changed = true
      return next
    })
    return changed ? { ...node, children } : node
  }
  return collapse(flipWalk(tree))
}

/**
 * Move the boundary between children `handleIndex` and `handleIndex + 1` of
 * split `splitId` so it lands at `boundaryFraction` (0..1 within the node's
 * extent along its axis). The pair is clamped so each keeps at least
 * `MIN_SPLIT_RATIO` (or half the pair when the pair is already smaller);
 * other siblings are untouched.
 */
export function resizeSplitAt(
  tree: LayoutNode,
  splitId: string,
  handleIndex: number,
  boundaryFraction: number
): LayoutNode {
  // A non-finite fraction (degenerate drag geometry — a zero-extent container
  // divides to NaN/Infinity) must never reach the ratios: JSON.stringify
  // serializes NaN as `null`, the persisted blob then fails validation on the
  // next launch and the user silently loses their whole layout.
  if (!Number.isFinite(boundaryFraction)) return tree
  const walk = (node: LayoutNode): LayoutNode => {
    if (node.type === "group") return node
    if (node.id === splitId) {
      if (handleIndex < 0 || handleIndex >= node.children.length - 1)
        return node
      const ratios = [...node.ratios]
      const prefix = ratios.slice(0, handleIndex).reduce((a, b) => a + b, 0)
      const pairSum = ratios[handleIndex] + ratios[handleIndex + 1]
      const effMin = Math.min(MIN_SPLIT_RATIO, pairSum / 2)
      const left = Math.min(
        Math.max(boundaryFraction - prefix, effMin),
        pairSum - effMin
      )
      if (Math.abs(left - ratios[handleIndex]) < 1e-4) return node
      ratios[handleIndex] = left
      ratios[handleIndex + 1] = pairSum - left
      return { ...node, ratios }
    }
    let changed = false
    const children = node.children.map((child) => {
      const next = walk(child)
      if (next !== child) changed = true
      return next
    })
    return changed ? { ...node, children } : node
  }
  return walk(tree)
}

export interface GroupRect {
  /** Percentages of the container, 0..100. */
  x: number
  y: number
  w: number
  h: number
}

export interface HandleRect {
  splitId: string
  /** Boundary between children[index] and children[index + 1]. */
  index: number
  /** Orientation of the SPLIT — "horizontal" means the divider line is
   *  vertical (col-resize). */
  orientation: SplitOrientation
  /** Line anchor (top-left), percentages. */
  x: number
  y: number
  /** Line length along the divider's own axis, percentage. */
  length: number
  /** The owning node's start/extent along the split axis, for drag math. */
  nodeStart: number
  nodeExtent: number
}

/** Absolute percentage rects for every group plus every divider between
 *  adjacent children — the render model for the flat-sibling shells. */
export function computeRects(tree: LayoutNode): {
  groups: Map<string, GroupRect>
  handles: HandleRect[]
} {
  const groups = new Map<string, GroupRect>()
  const handles: HandleRect[] = []

  const walk = (node: LayoutNode, rect: GroupRect) => {
    if (node.type === "group") {
      groups.set(node.id, rect)
      return
    }
    const horizontal = node.orientation === "horizontal"
    let cursor = horizontal ? rect.x : rect.y
    // Normalize per node rather than trusting the stored ratios to be positive
    // and sum to 1 — a legacy/hand-edited blob would otherwise paint groups at
    // zero size or leave dead space. Degenerate input falls back to even shares.
    const safeRatios = node.children.map((_, i) => {
      const r = node.ratios[i]
      return typeof r === "number" && Number.isFinite(r) && r > 0 ? r : 0
    })
    const ratioTotal = safeRatios.reduce((a, b) => a + b, 0)
    node.children.forEach((child, i) => {
      const share =
        ratioTotal > 0 ? safeRatios[i] / ratioTotal : 1 / node.children.length
      const extent = (horizontal ? rect.w : rect.h) * share
      if (i > 0) {
        handles.push({
          splitId: node.id,
          index: i - 1,
          orientation: node.orientation,
          x: horizontal ? cursor : rect.x,
          y: horizontal ? rect.y : cursor,
          length: horizontal ? rect.h : rect.w,
          nodeStart: horizontal ? rect.x : rect.y,
          nodeExtent: horizontal ? rect.w : rect.h,
        })
      }
      walk(
        child,
        horizontal
          ? { x: cursor, y: rect.y, w: extent, h: rect.h }
          : { x: rect.x, y: cursor, w: rect.w, h: extent }
      )
      cursor += extent
    })
  }

  walk(tree, { x: 0, y: 0, w: 100, h: 100 })
  return { groups, handles }
}

/** Defensive validator for the untrusted persisted blob. */
export function isLayoutNode(value: unknown): value is LayoutNode {
  if (typeof value !== "object" || value === null) return false
  const node = value as Record<string, unknown>
  if (node.type === "group") return typeof node.id === "string"
  if (node.type !== "split") return false
  if (typeof node.id !== "string") return false
  if (node.orientation !== "horizontal" && node.orientation !== "vertical")
    return false
  if (!Array.isArray(node.children) || !Array.isArray(node.ratios)) return false
  if (node.children.length < 2) return false
  if (node.children.length !== node.ratios.length) return false
  // Strictly positive: a `0` (or negative) share would hydrate a group that
  // owns tabs into a zero-size shell with no way to drag it back open.
  if (
    !node.ratios.every(
      (r) => typeof r === "number" && Number.isFinite(r) && r > 0
    )
  )
    return false
  return node.children.every(isLayoutNode)
}
