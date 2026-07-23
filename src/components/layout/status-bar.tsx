"use client"

import { StatusBarStats } from "@/components/layout/status-bar-stats"
import { StatusBarTasks } from "@/components/layout/status-bar-tasks"
import { StatusBarAlerts } from "@/components/layout/status-bar-alerts"
import { StatusBarUpdate } from "@/components/layout/status-bar-update"
import { CommandDropdown } from "@/components/layout/command-dropdown"
import { useIsMobile } from "@/hooks/use-mobile"

export function StatusBar() {
  const isMobile = useIsMobile()

  if (isMobile) {
    // Mobile mirrors the desktop bar: workspace stats on the left, the command
    // launcher + alerts on the right. `h-8` (matching desktop) gives the h-6
    // command control room. The branch selector and context-window circle live
    // in the below-composer row, so those are the bar's only two clusters.
    return (
      <div className="h-8 shrink-0 border-t border-border ws-chrome-border ws-surface-muted px-3 flex items-center justify-between text-xs text-muted-foreground">
        <div className="flex items-center gap-3">
          <StatusBarStats />
        </div>
        <div className="flex items-center gap-3">
          <CommandDropdown />
          <StatusBarAlerts />
        </div>
      </div>
    )
  }

  return (
    <div className="h-8 shrink-0 border-t border-border ws-chrome-border ws-surface-muted px-4 flex items-center justify-between text-xs text-muted-foreground">
      {/* The branch selector, context-window circle and agent connection status
          moved to the below-composer folder/branch row; the left side now
          carries just the workspace stats. */}
      <div className="flex items-center gap-3">
        <StatusBarStats />
      </div>
      <div className="flex items-center gap-4">
        <StatusBarUpdate />
        <StatusBarTasks />
        {/* Command launcher (moved from the aux "session details" tab), taking
            the slot the old static branch label (StatusBarSessionInfo) held. */}
        <CommandDropdown />
        <StatusBarAlerts />
      </div>
    </div>
  )
}
