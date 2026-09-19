"use client"

import { useTranslations } from "next-intl"
import {
  StatusIcon,
  getPriorityClassName,
  getPriorityKey,
  getStatusKey,
} from "@/components/message/plan-card"
import { Badge } from "@/components/ui/badge"
import type { PlanEntryInfo } from "@/lib/types"
import { cn } from "@/lib/utils"

// Shared between `AgentPlanOverlay` (the floating card over the message area)
// and the aux panel's session-details "tasks" section: same row, same status
// icon, same status/priority badges — the two surfaces render one plan's
// entries identically. The status icon itself comes from <PlanCard>'s
// `StatusIcon` (single source of truth for plan surfaces).

export function PlanEntryRow({
  entry,
  isStreaming,
  className,
}: {
  entry: PlanEntryInfo
  isStreaming: boolean
  className?: string
}) {
  const t = useTranslations("Folder.chat.agentPlanOverlay")
  return (
    <div
      className={cn("rounded-lg border bg-transparent px-2.5 py-2", className)}
    >
      <div className="flex items-start gap-2">
        <StatusIcon status={entry.status} isStreaming={isStreaming} />
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
        <Badge variant="outline" className="h-5 text-3xs uppercase">
          {t(getStatusKey(entry.status))}
        </Badge>
        <Badge
          variant="outline"
          className={cn(
            "h-5 text-3xs uppercase",
            getPriorityClassName(entry.priority)
          )}
        >
          {t(getPriorityKey(entry.priority))}
        </Badge>
      </div>
    </div>
  )
}
