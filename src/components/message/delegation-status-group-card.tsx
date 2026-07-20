"use client"

/**
 * Merged card for a run of consecutive `get_delegation_status` polls.
 *
 * When a delegated task runs past the 60s status-wait cap, the agent re-polls
 * repeatedly; the adapter collapses that run into a `delegation-status-group`
 * part, which this card renders as ONE card instead of N near-identical ones.
 * Polls are grouped by `task_id` and each task shows its LATEST poll — so:
 *   - a single task polled N times → one row with the final outcome (the N-1
 *     interim "running" snapshots are subsumed);
 *   - multiple tasks awaited in parallel (interleaved polls) → one row each.
 *
 * A returned "running" poll resolves to the neutral `checked` badge (see
 * `deriveBadge`), so superseded interim checks don't keep spinning.
 */

import { useMemo } from "react"

import { cn } from "@/lib/utils"
import type { AdaptedToolCallPart } from "@/lib/adapters/ai-elements-adapter"
import { buildDelegationTaskRows } from "@/lib/delegation-status"
import { DelegationStatusRow } from "@/components/message/delegation-status-row"

interface Props {
  polls: AdaptedToolCallPart[]
  /** Overrides the card's `data-testid`. The standalone `DelegationStatusCard`
   *  reuses this card for a stray ungrouped status poll and keeps its own
   *  `delegation-status-card` test id. */
  testId?: string
}

export function DelegationStatusGroupCard({
  polls,
  testId = "delegation-status-group",
}: Props) {
  // One row per task across all polls in the run. Each poll may carry a single
  // report or a whole batch (`task_ids`) — `buildDelegationTaskRows` attributes
  // every report to its task and groups them. See `@/lib/delegation-status`.
  const rows = useMemo(() => buildDelegationTaskRows(polls), [polls])

  if (rows.length === 0) return null

  // When every task ended in error, tint the whole card destructive (matching
  // the single card). Otherwise keep a neutral frame and tint only the failed
  // rows, so a mixed parallel wait reads per-task.
  const allError = rows.every((r) => r.badge.status === "err")

  return (
    <div
      data-testid={testId}
      className={cn(
        "overflow-hidden rounded-lg border text-xs",
        allError
          ? "border-destructive/30 bg-destructive/5"
          : "border-border bg-card ws-surface-capsule"
      )}
    >
      {rows.map((r, i) => (
        <div
          key={r.key}
          className={cn(
            i > 0 && "border-t border-border",
            !allError && r.badge.status === "err" && "bg-destructive/5"
          )}
        >
          <DelegationStatusRow
            kind="status"
            taskId={r.taskId}
            report={r.report}
            badge={r.badge}
            results={r.results}
          />
        </div>
      ))}
    </div>
  )
}
