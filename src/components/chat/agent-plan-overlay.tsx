"use client"

import { memo, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { PlanEntryInfo } from "@/lib/types"
import { cn } from "@/lib/utils"
import {
  CheckCircle2Icon,
  ChevronDownIcon,
  ChevronUpIcon,
  CircleDashedIcon,
  ListTodoIcon,
  Loader2Icon,
} from "lucide-react"

interface AgentPlanOverlayProps {
  message?: LiveMessage | null
  entries?: PlanEntryInfo[] | null
  planKey?: string | null
  visible?: boolean
  defaultExpanded?: boolean
  className?: string
}

export function getLatestPlanEntries(message: LiveMessage | null): PlanEntryInfo[] {
  if (!message) return []

  for (let i = message.content.length - 1; i >= 0; i -= 1) {
    const block = message.content[i]
    if (block.type === "plan") {
      return block.entries
    }
  }

  return []
}

function getStatusKey(
  status: string
):
  | "status.completed"
  | "status.inProgress"
  | "status.pending"
  | "status.unknown" {
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

type PriorityKey =
  | "priority.high"
  | "priority.medium"
  | "priority.low"
  | "priority.unknown"

function getPriorityKey(priority: string): PriorityKey {
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

function getPriorityClassName(priority: string): string {
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

function StatusIcon({ status }: { status: string }) {
  if (status === "completed") {
    return <CheckCircle2Icon className="h-3.5 w-3.5 text-emerald-500" />
  }

  if (status === "in_progress") {
    return <Loader2Icon className="h-3.5 w-3.5 text-blue-500 animate-spin" />
  }

  return <CircleDashedIcon className="h-3.5 w-3.5 text-muted-foreground" />
}

export const AgentPlanOverlay = memo(function AgentPlanOverlay({
  message,
  entries,
  planKey,
  visible = true,
  defaultExpanded = true,
  className,
}: AgentPlanOverlayProps) {
  const t = useTranslations("Folder.chat.agentPlanOverlay")
  const liveEntries = useMemo(
    () => getLatestPlanEntries(message ?? null),
    [message]
  )
  const resolvedEntries = useMemo(
    () => (liveEntries.length > 0 ? liveEntries : (entries ?? [])),
    [liveEntries, entries]
  )
  const hasPlan = visible && resolvedEntries.length > 0
  const fallbackPlanKey = useMemo(() => {
    if (resolvedEntries.length === 0) return null
    return resolvedEntries
      .map((entry) => `${entry.status}:${entry.priority}:${entry.content}`)
      .join("|")
  }, [resolvedEntries])
  const currentPlanKey = planKey ?? message?.id ?? fallbackPlanKey

  const completedCount = useMemo(
    () =>
      resolvedEntries.filter((entry) => entry.status === "completed").length,
    [resolvedEntries]
  )
  const hasIncompleteEntries = completedCount < resolvedEntries.length
  const resolvedDefaultExpanded = defaultExpanded && hasIncompleteEntries
  const currentPlanStateKey = currentPlanKey ?? "__plan__default__"
  const [collapsedByPlanKey, setCollapsedByPlanKey] = useState<
    Record<string, boolean>
  >({})
  const isExpanded = !(
    collapsedByPlanKey[currentPlanStateKey] ?? !resolvedDefaultExpanded
  )

  if (!hasPlan) {
    return null
  }

  if (!isExpanded) {
    return (
      <div className={cn("pointer-events-auto flex", className)}>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          className="cursor-pointer shadow-md bg-secondary/70 hover:bg-secondary"
          onClick={() =>
            setCollapsedByPlanKey((prev) => ({
              ...prev,
              [currentPlanStateKey]: false,
            }))
          }
        >
          <ListTodoIcon className="h-4 w-4" />
          {t("collapsedSummary", {
            completed: completedCount,
            total: resolvedEntries.length,
          })}
          <ChevronUpIcon className="h-4 w-4" />
        </Button>
      </div>
    )
  }

  return (
    <div
      className={cn("pointer-events-auto flex w-full sm:w-72", className)}
      data-plan-key={currentPlanKey ?? undefined}
    >
      <div className="w-full max-w-full rounded-xl border bg-card/60 shadow-lg backdrop-blur transition-colors hover:bg-card/95 supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <div className="flex items-center gap-2 min-w-0">
            <ListTodoIcon className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm font-medium truncate">{t("title")}</span>
            <Badge variant="secondary" className="h-5">
              {completedCount}/{resolvedEntries.length}
            </Badge>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label={t("collapsePlanAria")}
            onClick={() =>
              setCollapsedByPlanKey((prev) => ({
                ...prev,
                [currentPlanStateKey]: true,
              }))
            }
          >
            <ChevronDownIcon className="h-4 w-4" />
          </Button>
        </div>

        <div className="max-h-96 overflow-y-auto p-3 space-y-2">
          {resolvedEntries.map((entry, index) => (
            <div
              key={`${entry.content}-${index}`}
              className="rounded-lg border bg-transparent px-2.5 py-2"
            >
              <div className="flex items-start gap-2">
                <StatusIcon status={entry.status} />
                <p
                  className={cn(
                    "min-w-0 flex-1 text-sm leading-5 break-words [overflow-wrap:anywhere]",
                    entry.status === "completed"
                      ? "text-muted-foreground line-through"
                      : "text-foreground"
                  )}
                >
                  {entry.content}
                </p>
              </div>
              <div className="mt-2 flex items-center gap-1.5 pl-5">
                <Badge variant="outline" className="h-5 text-[10px] uppercase">
                  {t(getStatusKey(entry.status))}
                </Badge>
                <Badge
                  variant="outline"
                  className={cn(
                    "h-5 text-[10px] uppercase",
                    getPriorityClassName(entry.priority)
                  )}
                >
                  {t(getPriorityKey(entry.priority))}
                </Badge>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
})
