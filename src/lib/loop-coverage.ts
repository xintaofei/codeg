import type {
  LoopArtifactDetail,
  LoopArtifactRow,
  LoopCoverageRow,
} from "@/lib/types"

/** One acceptance criterion's stable ordinal + text. */
export interface CriterionOrdinal {
  ordinal: string
  text: string
}

/**
 * Build the stable `R{i}.AC{j}` ordinal + text for every acceptance criterion
 * across an issue's requirements, keyed by criterion id. Requirements are
 * ordered by `(sort, id)` and their acceptance criteria by `(sort, id)` — the
 * same order the backend uses, so an ordinal shown here matches what `covers`
 * stored and the coverage gate reasons about.
 */
export function acceptanceOrdinalMap(
  requirements: LoopArtifactDetail[]
): Map<number, CriterionOrdinal> {
  const ordered = [...requirements].sort(
    (a, b) => a.sort - b.sort || a.id - b.id
  )
  const map = new Map<number, CriterionOrdinal>()
  ordered.forEach((r, ri) => {
    const acc = r.criteria
      .filter((c) => c.kind === "acceptance")
      .sort((a, b) => a.sort - b.sort || a.id - b.id)
    acc.forEach((c, ci) => {
      map.set(c.id, { ordinal: `R${ri + 1}.AC${ci + 1}`, text: c.text })
    })
  })
  return map
}

/** Titles of the LIVE tasks that cover a given criterion (empty ⇒ uncovered).
 * Coverage rows of superseded/cancelled tasks (e.g. a replanned-away task) do
 * not count — they're dead, so a criterion they once claimed is really a gap. */
export function coveringTaskTitles(
  criterionId: number,
  coverage: LoopCoverageRow[],
  artifacts: LoopArtifactRow[]
): string[] {
  return coverage
    .filter((c) => c.criterion_id === criterionId)
    .map((c) => artifacts.find((a) => a.id === c.task_artifact_id))
    .filter(
      (a): a is LoopArtifactRow =>
        a != null && a.status !== "superseded" && a.status !== "cancelled"
    )
    .map((a) => a.title)
}

/** The acceptance criteria a task covers, as `{ordinal, text}`, ordinal-sorted. */
export function taskCovers(
  taskId: number,
  coverage: LoopCoverageRow[],
  ordinals: Map<number, CriterionOrdinal>
): CriterionOrdinal[] {
  return (
    coverage
      .filter((c) => c.task_artifact_id === taskId)
      .map((c) => ordinals.get(c.criterion_id))
      .filter((x): x is CriterionOrdinal => x != null)
      // Numeric collation so R10.AC1 sorts after R2.AC1, not before it.
      .sort((a, b) =>
        a.ordinal.localeCompare(b.ordinal, undefined, { numeric: true })
      )
  )
}
