"use client"

/**
 * Inline-start overlay listing the sub-agents delegated in the LAST agent reply.
 *
 * Mirrors `AgentPlanOverlay` (the "计划任务" panel): collapses to a bullet chip,
 * expands to a card, remembers collapse state per `overlayKey`, and renders
 * nothing when there's nothing to show. Positioning (absolute inline-start/top) is
 * owned by the shared overlay-stack container in `MessageListView`, which
 * places this panel BELOW the plan panel when both are present.
 *
 * Each row resolves its agent type / task / status / child ids from the same
 * `useDelegationCardModel` the inline `DelegatedSubThread` card uses, so the
 * overlay and the message-stream card never disagree. Clicking a row opens the
 * child's full conversation via `SubAgentSessionDialog` ("查看会话").
 */

import { memo, useState } from "react"
import { useTranslations } from "next-intl"
import { BotIcon, ChevronDownIcon } from "lucide-react"

import { CollapsedOverlayChip } from "@/components/chat/collapsed-overlay-chip"
import { DelegationRow } from "@/components/chat/delegation-row"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import type { DelegationCardSource } from "@/hooks/use-delegation-card-model"

interface SubAgentOverlayProps {
  /** The `delegate_to_agent` tool calls in the last assistant reply. */
  delegations: DelegationCardSource[]
  /** Stable key for the current "last assistant reply": collapse/expand state
   *  is remembered per key (and the parent also remounts via `key` on change,
   *  resetting state across conversations/messages). */
  overlayKey?: string | null
  /** Collapsed by default, matching the plan overlay. */
  defaultExpanded?: boolean
}

export const SubAgentOverlay = memo(function SubAgentOverlay({
  delegations,
  overlayKey,
  defaultExpanded = false,
}: SubAgentOverlayProps) {
  const t = useTranslations("Folder.chat.subAgentOverlay")
  const stateKey = overlayKey ?? "__subagents__default__"
  const [collapsedByKey, setCollapsedByKey] = useState<Record<string, boolean>>(
    {}
  )

  const count = delegations.length
  if (count === 0) {
    return null
  }

  const userCollapsed = collapsedByKey[stateKey]
  const isExpanded =
    userCollapsed !== undefined ? !userCollapsed : defaultExpanded

  if (!isExpanded) {
    return (
      <CollapsedOverlayChip
        icon={<BotIcon className="size-3" />}
        summary={t("collapsedSummary", { count })}
        onClick={() =>
          setCollapsedByKey((prev) => ({ ...prev, [stateKey]: false }))
        }
      />
    )
  }

  return (
    <div className="pointer-events-none flex max-w-[min(22rem,calc(100%-2rem))]">
      <div className="pointer-events-auto w-72 max-w-full rounded-xl border bg-card/60 hover:bg-card/95 shadow-lg backdrop-blur transition-colors supports-[backdrop-filter]:bg-card/50 supports-[backdrop-filter]:hover:bg-card/85">
        <div className="flex items-center justify-between border-b px-3 py-2">
          <div className="flex items-center gap-2 min-w-0">
            <BotIcon className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm font-medium truncate">{t("title")}</span>
            <Badge variant="secondary" className="h-5">
              {count}
            </Badge>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label={t("collapseAria")}
            onClick={() =>
              setCollapsedByKey((prev) => ({ ...prev, [stateKey]: true }))
            }
          >
            <ChevronDownIcon className="h-4 w-4" />
          </Button>
        </div>

        <div className="max-h-96 overflow-y-auto p-2 space-y-1.5">
          {delegations.map((source) => (
            <DelegationRow key={source.parentToolUseId} source={source} />
          ))}
        </div>
      </div>
    </div>
  )
})
