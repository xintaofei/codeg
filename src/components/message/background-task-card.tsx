"use client"

/**
 * Merged card for a run of consecutive Claude Code background-task polls.
 *
 * Claude Code launches a background shell with `Bash(run_in_background)` and
 * then re-polls it with `TaskOutput` until it settles (first timeout/running,
 * then success/completed). Rather than stack N near-identical "task" cards —
 * the old behaviour, which also dumped the raw XML envelope and ANSI-garbled
 * output through the Markdown renderer — the adapter collapses the run into a
 * `background-task-group` part and this card renders ONE row per `task_id`,
 * showing the latest outcome (status badge + exit code) with the shell output
 * routed through the ANSI-aware <Terminal>.
 *
 * Mirrors `DelegationStatusGroupCard` (the `get_delegation_status` analogue);
 * see `@/lib/background-task` for the parsing + grouping.
 */

import { useId, useMemo, useState } from "react"
import {
  Ban,
  CheckCircleIcon,
  ChevronDown,
  ChevronRight,
  Clock,
  Loader2,
  TerminalIcon,
  XCircleIcon,
} from "lucide-react"
import { useTranslations } from "next-intl"

import { cn } from "@/lib/utils"
import type { AdaptedToolCallPart } from "@/lib/adapters/ai-elements-adapter"
import {
  buildBackgroundTaskRows,
  type BackgroundTaskRow,
} from "@/lib/background-task"
import { Badge } from "@/components/ui/badge"
import { Shimmer } from "@/components/ai-elements/shimmer"
import { Terminal } from "@/components/ai-elements/terminal"

export function BackgroundTaskCard({
  polls,
}: {
  polls: AdaptedToolCallPart[]
}) {
  const rows = useMemo(() => buildBackgroundTaskRows(polls), [polls])

  if (rows.length === 0) return null

  // When every task failed, tint the whole card destructive (matching the
  // delegation card). Otherwise keep a neutral frame and tint only failed rows.
  const allFailed = rows.every((r) => r.badge === "failed")

  return (
    <div
      data-testid="background-task-group"
      className={cn(
        "overflow-hidden rounded-lg border text-xs ws-msg-card",
        allFailed
          ? "border-destructive/30 bg-destructive/5"
          : "border-border bg-card"
      )}
    >
      {rows.map((r, i) => (
        <div
          key={r.key}
          className={cn(
            i > 0 && "border-t border-border",
            !allFailed && r.badge === "failed" && "bg-destructive/5"
          )}
        >
          <BackgroundTaskRowView row={r} />
        </div>
      ))}
    </div>
  )
}

function BackgroundTaskRowView({ row }: { row: BackgroundTaskRow }) {
  const t = useTranslations("Folder.chat.contentParts.backgroundTask")
  const [expanded, setExpanded] = useState(false)
  const panelId = useId()

  const hasOutput = !!(row.output && row.output.trim() !== "")
  const shortId = row.taskId ? row.taskId.slice(0, 9) : null
  const title = row.command
    ? row.command.split("\n")[0].slice(0, 80)
    : shortId
      ? t("titleWithId", { id: shortId })
      : t("title")
  const isError = row.badge === "failed"
  const isRunning = row.badge === "running"

  const header = (
    <>
      <TerminalIcon
        className={cn(
          "size-3.5 shrink-0",
          isError ? "text-destructive" : "text-muted-foreground"
        )}
      />
      <span
        className="min-w-0 truncate font-medium text-foreground"
        title={row.command ?? row.taskId ?? undefined}
      >
        {isRunning && row.isInFlight ? (
          <Shimmer as="span" duration={1} shineColor="var(--primary)">
            {title}
          </Shimmer>
        ) : (
          title
        )}
      </span>
      {row.pollCount > 1 && (
        <span
          className="shrink-0 tabular-nums text-muted-foreground/70"
          title={t("polledTimes", { count: row.pollCount })}
        >
          ×{row.pollCount}
        </span>
      )}
      {isError && row.exitCode != null && (
        <span className="shrink-0 tabular-nums text-destructive">
          {t("exitCode", { code: row.exitCode })}
        </span>
      )}
      <TaskBadge badge={row.badge} isInFlight={row.isInFlight} />
      {hasOutput &&
        (expanded ? (
          <ChevronDown className="ml-auto size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="ml-auto size-3.5 shrink-0 text-muted-foreground" />
        ))}
    </>
  )

  return (
    <>
      {hasOutput ? (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          aria-controls={expanded ? panelId : undefined}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-muted/50"
        >
          {header}
        </button>
      ) : (
        <div className="flex w-full items-center gap-2 px-3 py-2">{header}</div>
      )}
      {hasOutput && expanded && (
        <div id={panelId} className="border-t border-border">
          <Terminal
            output={row.output ?? ""}
            isStreaming={row.isInFlight}
            className="max-h-80 rounded-none border-0"
          />
        </div>
      )}
    </>
  )
}

function TaskBadge({
  badge,
  isInFlight,
}: {
  badge: BackgroundTaskRow["badge"]
  isInFlight: boolean
}) {
  const t = useTranslations("Folder.chat.contentParts.backgroundTask")
  const className = "gap-1.5 rounded-full text-xs"
  if (badge === "completed") {
    return (
      <Badge className={className} variant="secondary">
        <CheckCircleIcon className="text-green-600" />
        {t("completed")}
      </Badge>
    )
  }
  if (badge === "failed") {
    return (
      <Badge className={className} variant="secondary">
        <XCircleIcon className="text-red-600" />
        {t("failed")}
      </Badge>
    )
  }
  if (badge === "stopped") {
    return (
      <Badge className={className} variant="secondary">
        <Ban className="text-muted-foreground" />
        {t("stopped")}
      </Badge>
    )
  }
  // running
  return (
    <Badge className={className} variant="secondary">
      {isInFlight ? (
        <Loader2 className="animate-spin" />
      ) : (
        <Clock className="text-muted-foreground" />
      )}
      {t("running")}
    </Badge>
  )
}
