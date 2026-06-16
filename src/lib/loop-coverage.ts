import type {
  LoopArtifactDetail,
  LoopArtifactRow,
  LoopCoverageRow,
  LoopCriterionCheckRow,
  LoopGateDecisionRow,
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

/** Fail-dominant representative of a non-empty check list: a failing check if any
 * (so reviewer disagreement within a round shows `fail`, matching the unanimous
 * gate), else the highest-id pass. */
function failDominant(list: LoopCriterionCheckRow[]): LoopCriterionCheckRow {
  return (
    list.find((c) => c.verdict === "fail") ??
    list.reduce((a, b) => (b.id > a.id ? b : a))
  )
}

/** A criterion's current verdict for the coverage-matrix glyph, keyed by criterion
 * id. The authoritative round boundary is a gate decision's `input_check_ids` (the
 * exact checks that gate aggregated), so each criterion is scored from the LATEST
 * gate round that aggregated it — a later passing round supersedes an earlier
 * failing one (no stale fail across a rework), and an integration round supersedes
 * the task round. Within that one round it is **fail-dominant** (a single failing
 * reviewer shows `fail`, never a misleading `pass`). Criteria with checks not yet
 * in any decision (an in-flight first round) fall back to fail-dominant over their
 * latest scope. Conservative under the majority rule, so the authoritative gate
 * outcome is shown separately in the drawer's gate-decision summary. */
export function criterionCheckMap(
  checks: LoopCriterionCheckRow[],
  decisions: LoopGateDecisionRow[] = []
): Map<number, LoopCriterionCheckRow> {
  const byId = new Map(checks.map((c) => [c.id, c]))
  const map = new Map<number, LoopCriterionCheckRow>()

  // Newest gate round first; the first round that scored a criterion wins it.
  for (const d of [...decisions].sort((a, b) => b.id - a.id)) {
    const byCriterion = new Map<number, LoopCriterionCheckRow[]>()
    for (const id of d.input_check_ids) {
      const c = byId.get(id)
      if (!c || map.has(c.criterion_id)) continue
      const list = byCriterion.get(c.criterion_id)
      if (list) list.push(c)
      else byCriterion.set(c.criterion_id, [c])
    }
    for (const [criterionId, list] of byCriterion) {
      map.set(criterionId, failDominant(list))
    }
  }

  // Fallback: checks not yet aggregated by any decision (in-flight first round) —
  // fail-dominant within the criterion's latest scope.
  const undecided = new Map<number, LoopCriterionCheckRow[]>()
  for (const c of checks) {
    if (map.has(c.criterion_id)) continue
    const list = undecided.get(c.criterion_id)
    if (list) list.push(c)
    else undecided.set(c.criterion_id, [c])
  }
  for (const [criterionId, list] of undecided) {
    const latest = list.reduce((a, b) => (b.id > a.id ? b : a))
    map.set(
      criterionId,
      failDominant(
        list.filter((c) => c.scope_artifact_id === latest.scope_artifact_id)
      )
    )
  }
  return map
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
