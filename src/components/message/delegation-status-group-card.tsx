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
import {
  deriveBadge,
  parseStatusReport,
  parseTaskId,
  type ResolvedBadge,
  type StatusReport,
} from "@/lib/delegation-status"
import { DelegationStatusRow } from "@/components/message/delegation-status-row"

interface Props {
  polls: AdaptedToolCallPart[]
}

interface TaskRow {
  key: string
  taskId: string | null
  report: StatusReport
  badge: ResolvedBadge
  pollCount: number
}

interface ParsedPoll {
  poll: AdaptedToolCallPart
  report: StatusReport
}

// Group polls by task_id, preserving first-appearance order, and resolve each
// task to its LATEST poll. The call arguments are the primary task ref; when
// they're missing/garbled we fall back to the structured report's own task_id
// (a historical row can drop the input while the output still carries it). A
// poll we STILL can't attribute is keyed by its unique tool-call id rather than
// a shared bucket — collapsing distinct unknowns would let the latest hide
// earlier outcomes and merge unrelated parallel waits into one row.
function buildTaskRows(polls: AdaptedToolCallPart[]): TaskRow[] {
  const order: string[] = []
  const byKey = new Map<
    string,
    { taskId: string | null; polls: ParsedPoll[] }
  >()
  for (const poll of polls) {
    const report = parseStatusReport(poll.output, poll.errorText)
    const taskId = parseTaskId(poll.input) ?? report.taskId
    const key = taskId ?? `__unattributed__:${poll.toolCallId}`
    let entry = byKey.get(key)
    if (!entry) {
      entry = { taskId, polls: [] }
      byKey.set(key, entry)
      order.push(key)
    }
    entry.polls.push({ poll, report })
  }
  return order.map((key) => {
    const entry = byKey.get(key)!
    const latest = entry.polls[entry.polls.length - 1]
    const badge = deriveBadge(
      "status",
      latest.report,
      latest.poll.state,
      !!latest.poll.errorText
    )
    return {
      key,
      taskId: entry.taskId,
      report: latest.report,
      badge,
      pollCount: entry.polls.length,
    }
  })
}

export function DelegationStatusGroupCard({ polls }: Props) {
  const rows = useMemo(() => buildTaskRows(polls), [polls])

  if (rows.length === 0) return null

  // When every task ended in error, tint the whole card destructive (matching
  // the single card). Otherwise keep a neutral frame and tint only the failed
  // rows, so a mixed parallel wait reads per-task.
  const allError = rows.every((r) => r.badge.status === "err")

  return (
    <div
      data-testid="delegation-status-group"
      className={cn(
        "overflow-hidden rounded-lg border text-xs",
        allError
          ? "border-destructive/30 bg-destructive/5"
          : "border-border bg-card"
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
            pollCount={r.pollCount}
          />
        </div>
      ))}
    </div>
  )
}
