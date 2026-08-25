"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { useShallow } from "zustand/react/shallow"
import type { MessageTurn } from "@/lib/types"
import { useTabStore } from "@/contexts/tab-context"
import { useConversationRuntimeStore } from "@/stores/conversation-runtime-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { resolveActiveSessionDetails } from "@/components/conversations/active-session-details"
import { SessionDetailsContent } from "@/components/conversations/session-details-content"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"

// Stable empty-turns reference so the `useShallow` slice below stays
// reference-equal when there's no active session — otherwise a fresh `[]` each
// render would defeat the shallow compare and re-render on every unrelated
// streaming batch.
const EMPTY_TURNS: MessageTurn[] = []

/**
 * The aux-panel "Session Details" tab. Shows the active conversation's metadata
 * and token usage (via the shared `SessionDetailsContent`). The branch selector
 * + command launcher that used to sit atop this tab now live in the bottom
 * status bar on both platforms, so the tab shows the details alone.
 *
 * Details are resolved from live runtime state exactly the way the conversation
 * detail panel does it (`resolveActiveSessionDetails`), so no network fetch is
 * needed for the focused session.
 */
export function SessionDetailsTab() {
  const t = useTranslations("Folder.sessionDetails")
  const { isOpen, activeTab } = useAuxPanelContext()

  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const activeConversationTab = useMemo(
    () =>
      tabs.find(
        (tab) => tab.id === activeTabId && tab.conversationId != null
      ) ?? null,
    [tabs, activeTabId]
  )

  // A brand-new conversation streams under its virtual `runtimeConversationId`
  // until it reconciles; key the live-session lookup on it first (mirrors the
  // detail panel, exercised by active-session-details.test.ts).
  const activeRuntimeId =
    activeConversationTab?.runtimeConversationId ??
    activeConversationTab?.conversationId ??
    null
  // Subscribe to ONLY the detail-related fields, not the whole session object.
  // The live-message sink replaces the session object on every streaming batch
  // (~60/s via SET_LIVE_MESSAGE); a whole-session selector would re-render this
  // tab — and its non-memoized details subtree — on each token. These fields
  // change only at turn boundaries, so `useShallow` keeps the slice
  // reference-stable across batches (mirrors use-conversation-detail.ts).
  const runtimeSlice = useConversationRuntimeStore(
    useShallow((s) => {
      const session =
        activeRuntimeId != null
          ? s.byConversationId.get(activeRuntimeId)
          : undefined
      return {
        detail: session?.detail ?? null,
        sessionStats: session?.sessionStats ?? null,
        localTurns: session?.localTurns ?? EMPTY_TURNS,
      }
    })
  )
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const { summary, stats, model } = resolveActiveSessionDetails(
    activeConversationTab,
    (id) => (id === activeRuntimeId ? runtimeSlice : null),
    conversations
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {summary ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="p-3">
            <SessionDetailsContent
              summary={summary}
              stats={stats}
              model={model}
              active={isOpen && activeTab === "session_details"}
            />
          </div>
        </ScrollArea>
      ) : (
        <div className="flex min-h-0 flex-1 items-center justify-center p-6 text-center text-sm text-muted-foreground">
          {t("noActiveSession")}
        </div>
      )}
    </div>
  )
}
