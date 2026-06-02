"use client"

/**
 * Inline card for the codeg-mcp delegation companion tools
 * `get_delegation_status` and `cancel_delegation`.
 *
 * A single collapsed line framed around the user's actual intent — "waiting for
 * task <id>'s result" (status) / "canceling task <id>" (cancel) — followed by
 * the task's execution time and a status badge, expandable to reveal the
 * result. Parsing + status resolution live in `@/lib/delegation-status` so this
 * card and the merged `DelegationStatusGroupCard` stay in lockstep.
 *
 * After the adapter collapses consecutive `get_delegation_status` polls into a
 * `delegation-status-group`, this card renders the `cancel` tool and serves as
 * a defensive fallback for a stray ungrouped status poll. The row itself
 * (`DelegationStatusRow`) is shared with the group card.
 */

import { useMemo } from "react"

import { cn } from "@/lib/utils"
import {
  deriveBadge,
  parseStatusReport,
  parseTaskId,
} from "@/lib/delegation-status"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import { DelegationStatusRow } from "@/components/message/delegation-status-row"

interface Props {
  /** Which companion tool this card represents — selects the label + icon. */
  kind: "status" | "cancel"
  /** Raw JSON arguments sent to the tool (`{ task_id, wait_ms? }`). */
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
}

export function DelegationStatusCard({
  kind,
  input,
  output,
  errorText,
  state,
}: Props) {
  const report = useMemo(
    () => parseStatusReport(output, errorText),
    [output, errorText]
  )
  // Prefer the call arguments; fall back to the structured report's own task_id
  // (a historical row can drop the input while the output still carries it).
  const taskId = useMemo(
    () => parseTaskId(input) ?? report.taskId,
    [input, report]
  )
  const badge = useMemo(
    () => deriveBadge(kind, report, state, !!errorText),
    [kind, report, state, errorText]
  )

  const isError = badge.status === "err"

  return (
    <div
      data-testid="delegation-status-card"
      className={cn(
        "overflow-hidden rounded-lg border text-xs",
        isError
          ? "border-destructive/30 bg-destructive/5"
          : "border-border bg-card"
      )}
    >
      <DelegationStatusRow
        kind={kind}
        taskId={taskId}
        report={report}
        badge={badge}
      />
    </div>
  )
}
