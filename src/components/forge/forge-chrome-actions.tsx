"use client"

import { useTranslations } from "next-intl"
import { RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useForgeRefreshStore } from "@/stores/forge-refresh-store"
import { cn } from "@/lib/utils"
import type { WorkbenchChromeActionsProps } from "@/components/workbench/workbench-content"

/**
 * The forge route's entry in the window's top-right chrome cluster, drawn
 * immediately left of the (window-level) settings gear — see
 * `WorkbenchRouteChromeActions`.
 *
 * Reloading acts on the WHOLE list rather than narrowing it, which is what
 * separates it from everything in the page's own toolbar; up here it also
 * stops competing for width with the filters. The handler and the busy flag
 * come from the page through `useForgeRefreshStore`, because this component
 * and the page it reloads have no common ancestor below the window shell.
 */
export function ForgeChromeActions({
  buttonClassName,
  iconClassName,
}: WorkbenchChromeActionsProps) {
  const t = useTranslations("Forge")
  const refresh = useForgeRefreshStore((s) => s.refresh)
  const busy = useForgeRefreshStore((s) => s.busy)

  return (
    <Button
      variant="ghost"
      size="icon"
      className={buttonClassName}
      // Nothing to reload until a page has published a repository — before
      // that the click would be a no-op the button gave no sign of.
      disabled={refresh == null || busy}
      onClick={() => refresh?.()}
      title={t("refresh")}
      aria-label={t("refresh")}
    >
      <RefreshCw className={cn(iconClassName, busy && "animate-spin")} />
    </Button>
  )
}
