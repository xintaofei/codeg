"use client"

import { useCallback, useMemo, useSyncExternalStore } from "react"
import { Coins } from "lucide-react"
import { useTranslations } from "next-intl"
import { useSessionStats } from "@/contexts/session-stats-context"
import { useConnectionStore } from "@/contexts/acp-connections-context"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import { formatTokenCount } from "@/lib/token-format"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"

const ICON_RADIUS = 6
const ICON_CENTER = 8
const ICON_VIEWBOX = 16
const ICON_CIRCUMFERENCE = 2 * Math.PI * ICON_RADIUS

function formatPercent(percent: number | null): string {
  if (percent == null) return "--"
  return `${percent.toFixed(1)}%`
}

function parseClaudeAutoCompactWindow(
  value: string | undefined
): number | null {
  const raw = value?.trim()
  if (!raw || !/^\+?\d+$/.test(raw)) return null
  const parsed = Number(raw)
  if (parsed < 100_000 || parsed > 1_000_000) return null
  return parsed
}

export function StatusBarTokens() {
  const t = useTranslations("Folder.statusBar.tokens")
  const store = useConnectionStore()
  const { sessionStats } = useSessionStats()
  const { agents } = useAcpAgents()
  const usage = sessionStats?.total_usage

  const subscribeActiveKey = useCallback(
    (cb: () => void) => store.subscribeActiveKey(cb),
    [store]
  )
  const getActiveKey = useCallback(() => store.getActiveKey(), [store])
  const activeKey = useSyncExternalStore(
    subscribeActiveKey,
    getActiveKey,
    getActiveKey
  )

  const subscribeConn = useCallback(
    (cb: () => void) => {
      if (!activeKey) return () => {}
      return store.subscribeKey(activeKey, cb)
    },
    [store, activeKey]
  )
  const getConnSnapshot = useCallback(
    () => (activeKey ? store.getConnection(activeKey) : undefined),
    [store, activeKey]
  )
  const activeConn = useSyncExternalStore(
    subscribeConn,
    getConnSnapshot,
    getConnSnapshot
  )

  const activeAgent = useMemo(
    () => agents.find((agent) => agent.agent_type === activeConn?.agentType),
    [agents, activeConn?.agentType]
  )
  const configuredAutoCompactWindow =
    activeConn?.agentType === "claude_code"
      ? parseClaudeAutoCompactWindow(
          activeAgent?.env.CLAUDE_CODE_AUTO_COMPACT_WINDOW
        )
      : null
  const rawLiveUsed = activeConn?.usage?.used ?? null
  const rawLiveSize = activeConn?.usage?.size ?? null
  // Treat live used=0 as "no data" so we fall back to sessionStats —
  // Claude Code sends used=0 for synthetic local commands (/context etc.)
  const liveContextUsed =
    rawLiveUsed != null && rawLiveUsed > 0 ? rawLiveUsed : null
  const liveContextMax =
    rawLiveSize != null && rawLiveSize > 0 ? rawLiveSize : null

  const contextUsed =
    liveContextUsed ?? sessionStats?.context_window_used_tokens ?? null
  const contextMax =
    configuredAutoCompactWindow ??
    liveContextMax ??
    sessionStats?.context_window_max_tokens ??
    null
  const contextPercentRaw =
    (contextUsed != null && contextMax != null && contextMax > 0
      ? (contextUsed / contextMax) * 100
      : configuredAutoCompactWindow == null
        ? sessionStats?.context_window_usage_percent
        : null) ?? null
  const contextPercent =
    contextPercentRaw == null
      ? null
      : Math.max(0, Math.min(100, contextPercentRaw))
  const hasContext = contextPercent != null
  const hasUsage = usage != null
  const fallbackTotal = hasUsage
    ? usage.input_tokens +
      usage.output_tokens +
      usage.cache_creation_input_tokens +
      usage.cache_read_input_tokens
    : null
  const total = sessionStats?.total_tokens ?? fallbackTotal

  const dashOffset = ICON_CIRCUMFERENCE * (1 - (contextPercent ?? 0) / 100)

  const rows: {
    key: "input" | "output" | "cacheRead" | "cacheWrite" | "total"
    value: number
  }[] = []
  if (hasUsage) {
    rows.push(
      { key: "input", value: usage.input_tokens },
      { key: "output", value: usage.output_tokens },
      { key: "cacheRead", value: usage.cache_read_input_tokens },
      { key: "cacheWrite", value: usage.cache_creation_input_tokens }
    )
  }
  if (total != null) {
    rows.push({ key: "total", value: total })
  }

  const hasTokenSection = rows.length > 0

  if (!hasContext && !hasTokenSection) return null

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button className="flex items-center gap-1 hover:text-foreground transition-colors">
          {hasContext ? (
            <>
              <svg
                aria-label={t("contextWindowUsageAria")}
                className="size-3.5"
                viewBox={`0 0 ${ICON_VIEWBOX} ${ICON_VIEWBOX}`}
              >
                <circle
                  cx={ICON_CENTER}
                  cy={ICON_CENTER}
                  r={ICON_RADIUS}
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  opacity="0.25"
                />
                <circle
                  cx={ICON_CENTER}
                  cy={ICON_CENTER}
                  r={ICON_RADIUS}
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                  strokeDasharray={`${ICON_CIRCUMFERENCE} ${ICON_CIRCUMFERENCE}`}
                  strokeDashoffset={dashOffset}
                  style={{
                    transformOrigin: "center",
                    transform: "rotate(-90deg)",
                  }}
                  opacity="0.75"
                />
              </svg>
              <span>{formatPercent(contextPercent)}</span>
            </>
          ) : (
            <>
              <Coins className="size-3.5" />
              <span>{formatTokenCount(total ?? 0)}</span>
            </>
          )}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="end" className="w-56 gap-2 p-3 text-xs">
        {hasContext ? (
          <div
            className={`space-y-1 ${
              hasUsage ? "mb-0.5 border-b border-border pb-0.5" : ""
            }`}
          >
            <div className="flex items-center justify-between gap-2 text-xs font-medium whitespace-nowrap">
              <span>{t("contextWindow")}</span>
              <span className="tabular-nums shrink-0">
                {formatPercent(contextPercent)}
              </span>
            </div>
            <div className="relative h-1.5 overflow-hidden rounded-full bg-muted">
              <div
                className="absolute inset-y-0 left-0 bg-foreground/70"
                style={{ width: `${contextPercent ?? 0}%` }}
              />
            </div>
            <div className="flex items-center justify-between text-xs leading-none text-muted-foreground">
              <span>{t("usedMax")}</span>
              <span className="tabular-nums">
                {contextUsed == null || contextMax == null
                  ? "--"
                  : `${formatTokenCount(contextUsed)} / ${formatTokenCount(contextMax)}`}
              </span>
            </div>
          </div>
        ) : null}
        {hasTokenSection ? (
          <>
            <div className="mb-0 mt-0.5 text-xs leading-none font-medium">
              {t("tokenUsage")}
            </div>
            <div className="space-y-0">
              {rows.map((row) => (
                <div
                  key={row.key}
                  className={`flex items-center justify-between py-0.5 text-xs leading-none ${
                    row.key === "total"
                      ? "mt-0.5 border-t border-border pt-0.5 font-medium"
                      : "text-muted-foreground"
                  }`}
                >
                  <span>{t(row.key)}</span>
                  <span className="tabular-nums">
                    {formatTokenCount(row.value)}
                  </span>
                </div>
              ))}
            </div>
          </>
        ) : null}
      </PopoverContent>
    </Popover>
  )
}
