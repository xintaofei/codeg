"use client"

/**
 * One collapsible row for a delegation status / cancel poll. Renders the
 * intent label ("waiting for task <id>'s result" / "canceling task <id>"), the
 * task execution time, an optional poll-count hint, and a status badge; expands
 * inline to reveal the result, Markdown-rendered.
 *
 * The row is presentation-only — it takes a pre-resolved `report` + `badge` so
 * both the single `DelegationStatusCard` and the merged
 * `DelegationStatusGroupCard` (which picks the latest poll per task) share one
 * row. The outer card owns the border / rounded shape / error tint.
 */

import { useId, useState } from "react"
import { Activity, Ban, ChevronDown, ChevronRight } from "lucide-react"
import { useTranslations } from "next-intl"

import { cn } from "@/lib/utils"
import {
  formatDuration,
  type ResolvedBadge,
  type StatusReport,
} from "@/lib/delegation-status"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import { StatusBadge } from "@/components/message/delegation-status-badge"

interface DelegationStatusRowProps {
  /** Which companion tool this row represents — selects the label + icon. */
  kind: "status" | "cancel"
  taskId: string | null
  report: StatusReport
  badge: ResolvedBadge
  /** Number of polls collapsed into this row (>1 surfaces a `×N` hint). */
  pollCount?: number
}

export function DelegationStatusRow({
  kind,
  taskId,
  report,
  badge,
  pollCount,
}: DelegationStatusRowProps) {
  const t = useTranslations("Folder.chat.delegation")
  const [expanded, setExpanded] = useState(false)
  const panelId = useId()

  const resultText = report.text
  const expandable = !!resultText
  const isError = badge.status === "err"
  const isRunning = badge.status === "running"
  const duration =
    report.durationMs != null ? formatDuration(report.durationMs) : null

  const shortId = taskId ? taskId.slice(0, 8) : null
  const label =
    kind === "cancel"
      ? shortId
        ? t("cancelTask", { task: `#${shortId}` })
        : t("cancelTaskNoTask")
      : shortId
        ? t("waitForResult", { task: `#${shortId}` })
        : t("waitForResultNoTask")

  const Icon = kind === "cancel" ? Ban : Activity

  const row = (
    <>
      <Icon
        className={cn(
          "h-3.5 w-3.5 shrink-0",
          isError ? "text-destructive" : "text-muted-foreground"
        )}
      />
      <span
        className="min-w-0 truncate text-xs font-medium text-foreground"
        title={taskId ?? undefined}
      >
        {isRunning ? (
          <Shimmer as="span" duration={1} shineColor="var(--primary)">
            {label}
          </Shimmer>
        ) : (
          label
        )}
      </span>
      {duration && (
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {duration}
        </span>
      )}
      {pollCount != null && pollCount > 1 && (
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground/70">
          ×{pollCount}
        </span>
      )}
      <StatusBadge status={badge.status} errorCode={badge.errorCode} />
      {expandable &&
        (expanded ? (
          <ChevronDown className="ml-auto h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="ml-auto h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ))}
    </>
  )

  return (
    <>
      {expandable ? (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          // The panel is only mounted while expanded (keeps the heavy Markdown
          // renderer out of the collapsed tree), so only reference it then —
          // avoids a dangling `aria-controls` target while collapsed.
          aria-controls={expanded ? panelId : undefined}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-muted/50"
        >
          {row}
        </button>
      ) : (
        <div className="flex w-full items-center gap-2 px-3 py-2">{row}</div>
      )}
      {expandable && expanded && (
        <div
          id={panelId}
          className="max-h-80 overflow-auto border-t border-border px-3 pb-2 pt-2"
        >
          <div className='prose prose-sm max-w-none break-words text-xs dark:prose-invert [&_ol]:list-inside [&_ul]:list-inside [&_[data-streamdown="code-block-body"]]:max-h-96 [&_[data-streamdown="code-block-body"]]:overflow-auto'>
            <MessageResponse>{resultText}</MessageResponse>
          </div>
        </div>
      )}
    </>
  )
}
