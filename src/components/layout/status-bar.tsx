"use client"

import { StatusBarStats } from "@/components/layout/status-bar-stats"
import { StatusBarSessionInfo } from "@/components/layout/status-bar-session-info"
import { StatusBarTasks } from "@/components/layout/status-bar-tasks"
import { StatusBarTokens } from "@/components/layout/status-bar-tokens"
import { StatusBarConnection } from "@/components/layout/status-bar-connection"
import { StatusBarAlerts } from "@/components/layout/status-bar-alerts"
import { StatusBarUpdate } from "@/components/layout/status-bar-update"
import { useIsMobile } from "@/hooks/use-mobile"

export function StatusBar() {
  const isMobile = useIsMobile()

  if (isMobile) {
    return (
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border bg-background px-3 py-1.5 text-xs text-muted-foreground">
        <div className="flex h-9 min-w-0 flex-1 items-center rounded-xl border bg-card px-3">
          <StatusBarConnection />
        </div>
        <div className="flex h-9 min-w-0 flex-[1.35] items-center justify-end gap-2 overflow-hidden rounded-xl border bg-card px-3">
          <StatusBarUpdate />
          <StatusBarTasks />
          <StatusBarAlerts />
        </div>
      </div>
    )
  }

  return (
    <div className="h-8 shrink-0 border-t border-border bg-muted px-4 flex items-center justify-between text-xs text-muted-foreground">
      <div className="flex items-center">
        <StatusBarStats />
      </div>
      <div className="flex items-center gap-4">
        <StatusBarUpdate />
        <StatusBarTasks />
        <StatusBarSessionInfo />
        <StatusBarTokens />
        <StatusBarConnection />
        <StatusBarAlerts />
      </div>
    </div>
  )
}
