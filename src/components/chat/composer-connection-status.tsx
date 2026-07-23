"use client"

import { useCallback, useSyncExternalStore } from "react"
import {
  HeartHandshake,
  HeartPulse,
  HeartCrack,
  HeartOff,
  type LucideIcon,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { useConnectionStore } from "@/contexts/acp-connections-context"
import { AGENT_LABELS } from "@/lib/types"
import { cn } from "@/lib/utils"

// Connection-only states. The session "prompting" state is intentionally
// collapsed into "connected" (the connection stays up while the agent responds),
// and anything unknown falls back to "disconnected".
type ConnStatusKey = "connected" | "connecting" | "error" | "disconnected"

// Colour-coded heart icons per connection state (HeartCrack stands in for the
// requested HeartX, which lucide does not ship).
const STATUS_ICON: Record<
  ConnStatusKey,
  { Icon: LucideIcon; className: string }
> = {
  connected: { Icon: HeartHandshake, className: "text-emerald-500" },
  connecting: { Icon: HeartPulse, className: "text-amber-500 animate-pulse" },
  error: { Icon: HeartCrack, className: "text-red-500" },
  disconnected: { Icon: HeartOff, className: "text-muted-foreground/60" },
}

function toConnStatus(status: string | null): ConnStatusKey {
  switch (status) {
    case "connected":
    case "prompting":
      return "connected"
    case "connecting":
      return "connecting"
    case "error":
      return "error"
    default:
      return "disconnected"
  }
}

/**
 * Connection-status icon shown in the row below the composer. Scoped to its own
 * conversation via `tabId` (the connection `contextKey`) so tiled/multi-open
 * composers each reflect their own connection. Shows connection state only — the
 * session "prompting" state is treated as connected — and a colour-coded heart
 * icon is the whole inline signal (no agent icon or model label); a native
 * `title` carries the detail on hover.
 */
export function ComposerConnectionStatus({ tabId }: { tabId: string | null }) {
  const t = useTranslations("Folder.statusBar.connection")
  const store = useConnectionStore()

  const subscribeConn = useCallback(
    (cb: () => void) => {
      if (!tabId) return () => {}
      return store.subscribeKey(tabId, cb)
    },
    [store, tabId]
  )
  const getConnSnapshot = useCallback(
    () => (tabId ? store.getConnection(tabId) : undefined),
    [store, tabId]
  )
  const conn = useSyncExternalStore(
    subscribeConn,
    getConnSnapshot,
    getConnSnapshot
  )

  const statusKey = toConnStatus(conn?.status ?? null)
  const statusLabel = t(statusKey)
  const agentType = conn?.agentType ?? null
  const agentLabel = agentType ? AGENT_LABELS[agentType] : null
  const titleText = !agentLabel
    ? statusLabel
    : statusKey === "error" && conn?.error
      ? t("tooltipError", { agent: agentLabel, error: conn.error })
      : t("tooltip", { agent: agentLabel, status: statusLabel })

  const { Icon, className } = STATUS_ICON[statusKey]

  // The native `title` (hover tooltip) lives on the wrapping span: React's SVG
  // types don't declare a `title` attribute, and the span guarantees the native
  // tooltip. The icon is left decorative (lucide auto-adds aria-hidden).
  return (
    <span
      role="img"
      aria-label={statusLabel}
      title={titleText}
      className="inline-flex shrink-0"
    >
      <Icon className={cn("size-3.5", className)} />
    </span>
  )
}
