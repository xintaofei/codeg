"use client"

/**
 * One collapsible row for a delegation status / cancel poll. Renders the
 * intent label ("waiting for task <id>'s result" / "canceling task <id>"), an
 * optional check-count hint (`×N`, the number of times the task was polled),
 * the task execution time, and a status badge; expands inline to reveal the
 * result, Markdown-rendered. A task polled more than once paginates through
 * each check's result (latest first) via a `< N / M >` footer whose total
 * matches the `×N` hint.
 *
 * The row is presentation-only — it takes a pre-resolved `report` + `badge`
 * (the latest poll) plus the per-poll `results` list (one entry per check,
 * `null` where a check returned no text), so both the single
 * `DelegationStatusCard` and the merged `DelegationStatusGroupCard` (which
 * groups polls per task) share one row. The outer card owns the border /
 * rounded shape / error tint.
 */

import { useId, useMemo, useState } from "react"
import {
  Activity,
  Ban,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
} from "lucide-react"
import { useTranslations } from "next-intl"

import { cn } from "@/lib/utils"
import {
  formatDuration,
  type ResolvedBadge,
  type StatusReport,
} from "@/lib/delegation-status"
import { Button } from "@/components/ui/button"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import { StatusBadge } from "@/components/message/delegation-status-badge"

interface DelegationStatusRowProps {
  /** Which companion tool this row represents — selects the label + icon. */
  kind: "status" | "cancel"
  taskId: string | null
  report: StatusReport
  badge: ResolvedBadge
  /**
   * One entry per poll of this task, in chronological order (`null` where a
   * poll returned no result text). Its length is the `×N` header hint (when
   * >1) and the expanded panel paginates through the entries (latest shown
   * first), so the badge count and the pager total always equal the check
   * count. Falls back to the single `report.text` so the standalone card is
   * unchanged.
   */
  results?: (string | null)[]
}

export function DelegationStatusRow({
  kind,
  taskId,
  report,
  badge,
  results: resultsProp,
}: DelegationStatusRowProps) {
  const t = useTranslations("Folder.chat.delegation")
  const [expanded, setExpanded] = useState(false)
  const panelId = useId()

  // The group card passes one entry per poll (so `×N` reflects the actual
  // check count); a standalone card passes nothing and falls back to its
  // single report.
  const results = useMemo<(string | null)[]>(() => {
    if (resultsProp && resultsProp.length > 0) return resultsProp
    return report.text ? [report.text] : []
  }, [resultsProp, report.text])

  // `null` = follow the latest result (the default, matching the collapsed
  // header's latest poll); a number = a page the user explicitly navigated to.
  // Tracking "follow latest" rather than a fixed index matters because the row
  // instance is keyed by task id and survives streaming rerenders — pinning an
  // index at mount would strand a row first seen with one result on page 1/M
  // once more polls arrive. Clamp the explicit case so a shrinking list can
  // never index out of range.
  const [pageIdx, setPageIdx] = useState<number | null>(null)
  const lastIdx = Math.max(results.length - 1, 0)
  const idx =
    pageIdx == null ? lastIdx : Math.min(Math.max(pageIdx, 0), lastIdx)
  const resultText = results[idx]
  const hasResultText = resultText != null && resultText.trim() !== ""
  const hasPager = results.length > 1

  // Expandable as long as at least one poll captured result text; a lone
  // still-running poll with nothing to show stays a plain (non-expandable) row.
  const expandable = results.some((r) => r != null && r.trim() !== "")

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
      {results.length > 1 && (
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground/70">
          ×{results.length}
        </span>
      )}
      {duration && (
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {duration}
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
        <div id={panelId} className="border-t border-border">
          {/* Keyed by the page index so switching results remounts the scroll
              area, resetting it to the top (a scrolled long result would
              otherwise leave the next one scrolled mid-way). */}
          <div key={idx} className="max-h-80 overflow-auto px-3 pb-2 pt-2">
            {hasResultText ? (
              <div className='prose prose-sm max-w-none break-words text-xs dark:prose-invert [&_ol]:list-inside [&_ul]:list-inside [&_[data-streamdown="code-block-body"]]:max-h-96 [&_[data-streamdown="code-block-body"]]:overflow-auto'>
                <MessageResponse>{resultText}</MessageResponse>
              </div>
            ) : (
              <p className="text-xs italic text-muted-foreground">
                {t("noResultText")}
              </p>
            )}
          </div>
          {hasPager && (
            <div className="flex items-center justify-center gap-1 border-t border-border px-3 py-1">
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                disabled={idx <= 0}
                aria-label={t("prevResult")}
                title={t("prevResult")}
                onClick={() => setPageIdx(idx - 1)}
              >
                <ChevronLeft className="size-3" />
              </Button>
              <span className="min-w-12 text-center text-xs tabular-nums text-muted-foreground">
                {t("resultPageOf", {
                  current: idx + 1,
                  total: results.length,
                })}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="icon-xs"
                disabled={idx >= lastIdx}
                aria-label={t("nextResult")}
                title={t("nextResult")}
                onClick={() => setPageIdx(idx + 1)}
              >
                <ChevronRight className="size-3" />
              </Button>
            </div>
          )}
        </div>
      )}
    </>
  )
}
