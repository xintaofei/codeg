"use client"

import { memo, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { CollapsedOverlayChip } from "@/components/chat/collapsed-overlay-chip"
import { PlanEntryRow } from "@/components/chat/plan-entry-row"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { PlanEntryInfo } from "@/lib/types"
import { ChevronDownIcon, ListTodoIcon } from "lucide-react"

interface AgentPlanOverlayProps {
  message?: LiveMessage | null
  entries?: PlanEntryInfo[] | null
  planKey?: string | null
  visible?: boolean
  defaultExpanded?: boolean
  isStreaming?: boolean
}

function getLatestPlanEntries(message: LiveMessage | null): PlanEntryInfo[] {
  if (!message) return []

  for (let i = message.content.length - 1; i >= 0; i -= 1) {
    const block = message.content[i]
    if (block.type === "plan") {
      return block.entries
    }
  }

  return []
}

export const AgentPlanOverlay = memo(function AgentPlanOverlay({
  message,
  entries,
  planKey,
  visible = true,
  defaultExpanded = false,
  isStreaming = false,
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

  // Detect the streaming "plan just created" transition and latch a one-time
  // auto-expand. Done with the adjust-state-during-render pattern (guarded
  // setState in the render body, not an effect) so it converges before paint —
  // no collapsed→expanded flash and no cascading-render lint warnings.
  //
  // The overlay remounts per live message (parent keys it on the message id),
  // so `prevLiveHadPlan === null` means this mount's first render. A plan that
  // is already present then (opening a mid-stream session) initializes the
  // tracker without expanding; only a later false→true flip while streaming —
  // i.e. the agent creating the plan as we watch — triggers the auto-expand.
  const liveHasPlan = liveEntries.length > 0
  const [prevLiveHadPlan, setPrevLiveHadPlan] = useState<boolean | null>(null)
  const [autoExpanded, setAutoExpanded] = useState(false)
  if (prevLiveHadPlan !== liveHasPlan) {
    const planCreatedLive =
      prevLiveHadPlan === false &&
      liveHasPlan &&
      isStreaming &&
      hasIncompleteEntries
    setPrevLiveHadPlan(liveHasPlan)
    if (planCreatedLive) {
      setAutoExpanded(true)
    }
  }

  const userCollapsed = collapsedByPlanKey[currentPlanStateKey]
  const isExpanded =
    userCollapsed !== undefined
      ? !userCollapsed
      : autoExpanded || resolvedDefaultExpanded

  if (!hasPlan) {
    return null
  }

  if (!isExpanded) {
    // Positioning (absolute start-0 top-4 z-20) is owned by the shared
    // overlay-stack container in MessageListView so this panel stacks with the
    // sub-agent overlay; the chip only declares layout + pointer behavior.
    return (
      <CollapsedOverlayChip
        icon={<ListTodoIcon className="size-3" />}
        summary={t("collapsedSummary", {
          completed: completedCount,
          total: resolvedEntries.length,
        })}
        onClick={() =>
          setCollapsedByPlanKey((prev) => ({
            ...prev,
            [currentPlanStateKey]: false,
          }))
        }
      />
    )
  }

  return (
    <div
      className="pointer-events-none flex max-w-[min(22rem,calc(100%-2rem))]"
      data-plan-key={currentPlanKey ?? undefined}
    >
      <div className="pointer-events-auto w-72 max-w-full rounded-xl border bg-card/60 hover:bg-card/95 shadow-lg backdrop-blur transition-colors supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
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

        {/* Height-capped: the panel rides a PINNED absolute overlay stack
            inside a fixed-height pane (message-list-view), so an unbounded
            body would push long plans past the viewport with no scroll
            context anywhere — the trailing entries become unreachable. */}
        <div className="max-h-96 overflow-y-auto p-3 space-y-2">
          {resolvedEntries.map((entry, index) => (
            <PlanEntryRow
              key={`${entry.content}-${index}`}
              entry={entry}
              isStreaming={isStreaming}
            />
          ))}
        </div>
      </div>
    </div>
  )
})
