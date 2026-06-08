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
      <div className="h-7 shrink-0 border-t border-border bg-muted px-3 flex items-center justify-between text-xs text-muted-foreground">
        <StatusBarConnection />
        <div className="flex items-center gap-3">
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
