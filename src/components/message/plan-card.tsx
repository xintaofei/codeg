"use client"

import { memo } from "react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import type { PlanEntryInfo } from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  CheckCircle2Icon,
  CircleDashedIcon,
  CircleDotIcon,
  ListTodoIcon,
  Loader2Icon,
} from "lucide-react"

// ── Shared status/priority label + style helpers ──────────────────────
// Single source of truth for plan entry labels/colors, shared by <PlanCard>
// (inline message rendering) and <AgentPlanOverlay> (floating chip) so the
// two surfaces never drift.

export type StatusKey =
  | "status.completed"
  | "status.inProgress"
  | "status.pending"
  | "status.unknown"

export function getStatusKey(status: string): StatusKey {
  switch (status) {
    case "completed":
      return "status.completed"
    case "in_progress":
      return "status.inProgress"
    case "pending":
      return "status.pending"
    default:
      return "status.unknown"
  }
}

export type PriorityKey =
  | "priority.high"
  | "priority.medium"
  | "priority.low"
  | "priority.unknown"

export function getPriorityKey(priority: string): PriorityKey {
  switch (priority) {
    case "high":
      return "priority.high"
    case "medium":
      return "priority.medium"
    case "low":
      return "priority.low"
    default:
      return "priority.unknown"
  }
}

export function getPriorityClassName(priority: string): string {
  switch (priority) {
    case "high":
      return "text-red-700 bg-red-500/10 border-red-500/20 dark:text-red-300"
    case "medium":
      return "text-amber-700 bg-amber-500/10 border-amber-500/20 dark:text-amber-300"
    case "low":
      return "text-slate-700 bg-slate-500/10 border-slate-500/20 dark:text-slate-300"
    default:
      return "text-muted-foreground"
  }
}

function StatusIcon({
  status,
  isStreaming,
}: {
  status: string
  isStreaming: boolean
}) {
  if (status === "completed") {
    return <CheckCircle2Icon className="size-3.5 shrink-0 text-emerald-500" />
  }
  if (status === "in_progress") {
    return isStreaming ? (
      <Loader2Icon className="size-3.5 shrink-0 animate-spin text-blue-500" />
    ) : (
      <CircleDotIcon className="size-3.5 shrink-0 text-blue-500" />
    )
  }
  return (
    <CircleDashedIcon className="size-3.5 shrink-0 text-muted-foreground" />
  )
}

/**
 * The plan entry rows (checklist body). Shared by <PlanCard> and the
 * historical TodoWrite fallback so every plan surface renders identically.
 */
export const PlanEntriesList = memo(function PlanEntriesList({
  entries,
  isStreaming = false,
}: {
  entries: PlanEntryInfo[]
  isStreaming?: boolean
}) {
  const t = useTranslations("Folder.chat.agentPlanOverlay")

  if (entries.length === 0) return null

  return (
    <div className="space-y-1">
      {entries.map((entry, index) => (
        <div
          key={`${entry.content}-${index}`}
          className="flex items-start gap-2 px-1 py-1 text-sm"
        >
          <span className="mt-0.5">
            <StatusIcon status={entry.status} isStreaming={isStreaming} />
          </span>
          <span
            className={cn(
              "min-w-0 flex-1 leading-5 break-words [overflow-wrap:anywhere]",
              entry.status === "completed"
                ? "text-muted-foreground line-through"
                : "text-foreground"
            )}
          >
            {entry.content}
          </span>
          <Badge
            variant="outline"
            className={cn(
              "h-5 shrink-0 text-[10px] uppercase",
              getPriorityClassName(entry.priority)
            )}
          >
            {t(getPriorityKey(entry.priority))}
          </Badge>
        </div>
      ))}
    </div>
  )
})

/**
 * Inline plan / todo checklist card rendered in the message stream. Replaces
 * the previous behavior of showing the plan as a "Thinking…" reasoning block.
 */
export const PlanCard = memo(function PlanCard({
  entries,
  isStreaming = false,
}: {
  entries: PlanEntryInfo[]
  isStreaming?: boolean
}) {
  const t = useTranslations("Folder.chat.agentPlanOverlay")

  if (entries.length === 0) return null

  const completedCount = entries.filter(
    (entry) => entry.status === "completed"
  ).length

  return (
    <div className="overflow-hidden rounded-lg border bg-card/50 ws-msg-card">
      <div className="flex items-center gap-2 border-b px-3 py-2">
        <ListTodoIcon className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1 truncate text-sm font-medium">
          {t("title")}
        </span>
        <Badge variant="secondary" className="h-5 shrink-0">
          {completedCount}/{entries.length}
        </Badge>
      </div>
      <ScrollArea className="max-h-72 px-2 py-2">
        <PlanEntriesList entries={entries} isStreaming={isStreaming} />
      </ScrollArea>
    </div>
  )
})
