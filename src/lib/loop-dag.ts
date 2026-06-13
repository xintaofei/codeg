import type {
  LoopArtifactKind,
  LoopArtifactRow,
  LoopLinkKind,
  LoopLinkRow,
} from "@/lib/types"

/**
 * Fixed pipeline column order. Every artifact kind maps to one column, so the
 * graph always reads left → right as the loop's stages — and a skipped stage
 * (e.g. `design` on the `skip_design` route) shows up as an empty gap rather
 * than collapsing, which is exactly the provenance a reader wants to see.
 */
export const DAG_COLUMNS: LoopArtifactKind[] = [
  "issue",
  "requirement",
  "design",
  "task",
  "review",
  "result",
]

export interface DagNode {
  artifact: LoopArtifactRow
  /** Column index into {@link DAG_COLUMNS} (0–5). */
  col: number
  /** Row within the column (0-based, top to bottom). */
  row: number
}

export interface DagEdge {
  id: number
  /** Dependent artifact (child / reviewer / result) — the edge's tail. */
  from: number
  /** Referenced artifact (source / parent / subject) — the edge's head. */
  to: number
  kind: LoopLinkKind
  /** `skips_to` provenance renders dashed; everything else solid. */
  dashed: boolean
}

export interface DagLayout {
  /** Nodes sorted by (col, row) for deterministic rendering. */
  nodes: DagNode[]
  /** Edges whose both endpoints are present in this DAG. */
  edges: DagEdge[]
  /** Tallest column's node count (SVG height driver); 0 when empty. */
  rowCount: number
  /** Highest occupied column index + 1 (SVG width driver); 0 when empty. */
  colCount: number
}

const columnOf = (kind: LoopArtifactKind): number => {
  const i = DAG_COLUMNS.indexOf(kind)
  // Unknown kinds (forward-compat) park in the last column rather than vanish.
  return i < 0 ? DAG_COLUMNS.length - 1 : i
}

/**
 * Build a layered layout from a DagView's artifacts + links. Pure and stable:
 * columns are kind-fixed, rows pack each kind's artifacts top-to-bottom by
 * `sort` then `id`, and edges keep their stored direction (tail = dependent,
 * head = referenced). No I/O — unit-tested directly.
 */
export function buildDag(
  artifacts: LoopArtifactRow[],
  links: LoopLinkRow[]
): DagLayout {
  // Bucket artifacts by column.
  const byCol = new Map<number, LoopArtifactRow[]>()
  for (const a of artifacts) {
    const col = columnOf(a.kind)
    const bucket = byCol.get(col)
    if (bucket) bucket.push(a)
    else byCol.set(col, [a])
  }

  const nodes: DagNode[] = []
  let rowCount = 0
  let colCount = 0
  for (const [col, bucket] of byCol) {
    bucket.sort((x, y) => x.sort - y.sort || x.id - y.id)
    bucket.forEach((artifact, row) => nodes.push({ artifact, col, row }))
    if (bucket.length > rowCount) rowCount = bucket.length
    if (col + 1 > colCount) colCount = col + 1
  }
  nodes.sort((a, b) => a.col - b.col || a.row - b.row)

  // Drop edges that dangle (an endpoint not in this DAG) so the renderer never
  // draws to a missing node.
  const present = new Set(artifacts.map((a) => a.id))
  const edges: DagEdge[] = links
    .filter(
      (l) => present.has(l.from_artifact_id) && present.has(l.to_artifact_id)
    )
    .map((l) => ({
      id: l.id,
      from: l.from_artifact_id,
      to: l.to_artifact_id,
      kind: l.kind,
      dashed: isDashed(l.kind),
    }))

  return { nodes, edges, rowCount, colCount }
}

const isDashed = (kind: LoopLinkKind): boolean => kind === "skips_to"
