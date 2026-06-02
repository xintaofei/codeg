"use client"

/**
 * Viewer for a delegated sub-agent's full conversation.
 *
 * Opens from `DelegatedSubThread`'s header and renders the same
 * `MessageListView` used by the main conversation panel, but without
 * the input bar, send signal, or reload/new-session handlers — so the
 * user can scroll the transcript without driving the child's turns. The
 * one interaction it hosts is the child's permission prompt: when the
 * child (running at the user's configured permission level) requests
 * approval, the dialog is surfaced here to allow/deny, since the parent
 * card itself is non-interactive (it only badges "awaiting approval").
 *
 * Streaming: while the sheet is open, the child connection's live
 * message and status (from `acp-connections-context`) are mirrored
 * into the runtime session for the child `conversationId` so the
 * `MessageListView` shows real-time deltas. The bridge runs only
 * while the sheet is mounted; once it closes, no further mirroring
 * happens. Persistence of completed turns comes from the broker's
 * own DB writes, surfaced via `useConversationDetail`.
 */

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { MessageListView } from "@/components/message/message-list-view"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import {
  useAcpActions,
  useConnectionStore,
  type ConnectionState,
} from "@/contexts/acp-connections-context"
import { PermissionDialog } from "@/components/chat/permission-dialog"
import { AGENT_LABELS, type AgentType } from "@/lib/types"

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  childConversationId: number
  childConnectionId: string | null
  agentType: AgentType | null
}

function useChildConnectionState(
  connectionId: string | null
): ConnectionState | undefined {
  const store = useConnectionStore()
  const subscribe = useCallback(
    (cb: () => void) => {
      if (!connectionId) return () => {}
      return store.subscribeKey(connectionId, cb)
    },
    [store, connectionId]
  )
  const getSnapshot = useCallback(
    () => (connectionId ? store.getConnection(connectionId) : undefined),
    [store, connectionId]
  )
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

/**
 * Bridge the child connection's `liveMessage` and status transitions into
 * the runtime session for `childConversationId`, so the read-only
 * `MessageListView` sees streaming turns and turn completions while the
 * sheet is open.
 *
 * Mirrors the effects in `conversation-detail-panel.tsx` with two extra
 * concerns specific to this read-only sheet:
 *
 *  1. **Detail-fetch race.** Opening the sheet triggers `fetchDetail`. Its
 *     `FETCH_DETAIL_SUCCESS` reducer wipes any bridged `liveMessage`
 *     because no `awaiting_persist` syncState is in place (no user just
 *     sent a prompt — the broker did). To avoid losing the in-flight turn
 *     when detail loading completes mid-stream, a re-bridge effect fires
 *     on the `detailLoading: true → false` edge and re-dispatches
 *     `setLiveMessage` from the current `conn.liveMessage`. Using
 *     `isLive` = (connStatus === "prompting") preserves the
 *     reconnect-replay guard at SET_LIVE_MESSAGE: an actively prompting
 *     stream bypasses it, a finished message gets rejected because the
 *     freshly loaded detail.turns already contains it.
 *
 *  2. **Close-mid-stream / reopen-after-complete.** The cleanup of the
 *     mirror-live effect intentionally does not clear `liveMessage` while
 *     still prompting (so it remains promotable for the completeTurn
 *     edge). If the user closes the sheet during that window and the
 *     child later finishes, no bridge is running to dispatch
 *     `completeTurn`, leaving stale `liveMessage` in runtime state. On
 *     reopen, `fetchDetail`'s active-data guard would skip the refetch
 *     and the user would see a stale partial transcript. We solve this
 *     by calling `removeConversation` on the sheet body's full unmount —
 *     the runtime session is owned by this sheet alone, so dropping it
 *     forces the next open to fetch the persisted detail from scratch.
 */
function useChildLiveBridge(
  childConversationId: number,
  childConnState: ConnectionState | undefined,
  detailLoading: boolean
) {
  const { setLiveMessage, completeTurn, removeConversation } =
    useConversationRuntime()

  const connStatus = childConnState?.status ?? null
  const liveMessage = childConnState?.liveMessage ?? null

  const connStatusRef = useRef(connStatus)
  useEffect(() => {
    connStatusRef.current = connStatus
  }, [connStatus])

  // Effect ORDER matters: completeTurn must be declared BEFORE mirror-live
  // so React runs its setup before mirror-live's cleanup. When connStatus
  // transitions away from "prompting", completeTurn snapshots and promotes
  // liveMessage first; then mirror-live's cleanup can safely clear it.
  const prevStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevStatusRef.current === "prompting"
    prevStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return
    completeTurn(childConversationId, liveMessage)
  }, [connStatus, liveMessage, childConversationId, completeTurn])

  useEffect(() => {
    if (liveMessage != null) {
      setLiveMessage(
        childConversationId,
        liveMessage,
        connStatus === "prompting"
      )
    }
    return () => {
      if (connStatusRef.current !== "prompting") {
        setLiveMessage(childConversationId, null)
      }
    }
  }, [liveMessage, connStatus, childConversationId, setLiveMessage])

  // Re-bridge after detail-loading transitions true → false. The
  // FETCH_DETAIL_SUCCESS reducer cleared our liveMessage; restore it from
  // the current connection state so an actively streaming turn doesn't
  // vanish from the sheet right after detail finishes loading.
  const prevDetailLoadingRef = useRef(detailLoading)
  useEffect(() => {
    const wasLoading = prevDetailLoadingRef.current
    prevDetailLoadingRef.current = detailLoading
    if (!wasLoading || detailLoading) return
    if (liveMessage == null) return
    setLiveMessage(childConversationId, liveMessage, connStatus === "prompting")
  }, [
    detailLoading,
    liveMessage,
    connStatus,
    childConversationId,
    setLiveMessage,
  ])

  // Full teardown on sheet close: drop the runtime session so the next
  // open starts from a fresh `fetchDetail` instead of stale bridged state.
  useEffect(() => {
    return () => {
      removeConversation(childConversationId)
    }
  }, [childConversationId, removeConversation])
}

export function SubAgentSessionSheet({
  open,
  onOpenChange,
  childConversationId,
  childConnectionId,
  agentType,
}: Props) {
  const t = useTranslations("Folder.chat.delegation")

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="w-full sm:max-w-2xl lg:max-w-3xl [&_[data-slot=sheet-close]]:top-2"
      >
        <SheetTitle className="sr-only">{t("detailTitle")}</SheetTitle>
        <SheetDescription className="sr-only">
          {t("detailDescription")}
        </SheetDescription>
        {open ? (
          <SubAgentSessionBody
            childConversationId={childConversationId}
            childConnectionId={childConnectionId}
            agentType={agentType}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}

function SubAgentSessionBody({
  childConversationId,
  childConnectionId,
  agentType,
}: {
  childConversationId: number
  childConnectionId: string | null
  agentType: AgentType | null
}) {
  const t = useTranslations("Folder.chat.delegation")

  // Force a fresh fetch on every open. Necessary because the previous
  // open's `useConversationDetail` auto-`fetchDetail` could still be
  // in-flight when the user closes the sheet — its later resolution
  // resurrects a stale runtime session that survives the unmount's
  // `removeConversation`. The auto-fetch in `useConversationDetail`
  // would then skip on reopen (active-data guard), surfacing a stale
  // pre-completion transcript. `refetchDetail` bypasses that guard so
  // the latest DB state always wins.
  const { refetchDetail } = useConversationRuntime()
  useEffect(() => {
    refetchDetail(childConversationId)
  }, [childConversationId, refetchDetail])

  const { loading, error, acpLoadError } =
    useConversationDetail(childConversationId)

  const childConn = useChildConnectionState(childConnectionId)
  useChildLiveBridge(childConversationId, childConn, loading)

  const connStatus = childConn?.status ?? null

  // The child runs with the user's configured permission level, so it may
  // raise a permission request. The parent card no longer answers it inline
  // (it only badges "awaiting approval"); this sheet is where the user
  // resolves it. Route the response through the CHILD connection id.
  const { respondPermission } = useAcpActions()
  const childPendingPermission = childConn?.pendingPermission ?? null
  const onRespondPermission = useCallback(
    (requestId: string, optionId: string) => {
      if (!childConnectionId) return
      void respondPermission(childConnectionId, requestId, optionId)
    },
    [childConnectionId, respondPermission]
  )

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 px-5 py-2.5 border-b border-border pr-12">
        <span className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-border bg-background text-foreground">
          {agentType ? (
            <AgentIcon agentType={agentType} className="h-4 w-4" />
          ) : (
            <span className="h-2 w-2 rounded-sm bg-muted-foreground/60" />
          )}
        </span>
        <span className="min-w-0 flex-1 truncate text-sm font-semibold text-foreground">
          {agentType ? AGENT_LABELS[agentType] : t("unknownAgent")}
        </span>
      </div>
      {childPendingPermission && (
        <div className="border-b border-border px-4 py-3">
          <PermissionDialog
            permission={childPendingPermission}
            onRespond={onRespondPermission}
          />
        </div>
      )}
      <div className="flex-1 min-h-0 px-4 py-3">
        <MessageListView
          conversationId={childConversationId}
          agentType={agentType ?? "claude_code"}
          connStatus={connStatus}
          isActive={false}
          detailLoading={loading}
          detailError={error}
          acpLoadError={acpLoadError}
          hideEmptyState={false}
        />
      </div>
    </div>
  )
}
