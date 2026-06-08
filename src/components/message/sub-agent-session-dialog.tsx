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
 * Streaming: while the dialog is open, the child connection's live
 * message and status (from `acp-connections-context`) are mirrored
 * into the runtime session for the child `conversationId` so the
 * `MessageListView` shows real-time deltas. The bridge runs only
 * while the dialog is mounted; once it closes, no further mirroring
 * happens. Persistence of completed turns comes from the broker's
 * own DB writes, surfaced via `useConversationDetail`.
 */

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { MessageListView } from "@/components/message/message-list-view"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
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
  /**
   * The parent's `delegate_to_agent` task text — the child's kickoff prompt,
   * known synchronously in the card. Surfaced so the kickoff user turn can be
   * shown immediately while the child's persisted transcript still lags the
   * live stream (the agent CLI writes its JSONL asynchronously).
   */
  kickoffTask?: string | null
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
 * dialog is open.
 *
 * Mirrors the effects in `conversation-detail-panel.tsx`, with one concern
 * specific to this read-only dialog:
 *
 *  **Close-mid-stream / reopen-after-complete.** The cleanup of the
 *  mirror-live effect intentionally does not clear `liveMessage` while
 *  still prompting (so it remains promotable for the completeTurn edge).
 *  If the user closes the dialog during that window and the child later
 *  finishes, no bridge is running to dispatch `completeTurn`, leaving stale
 *  `liveMessage` in runtime state. On reopen, `fetchDetail`'s active-data
 *  guard would skip the refetch and the user would see a stale partial
 *  transcript. We solve this by calling `removeConversation` on the dialog
 *  body's full unmount — the runtime session is owned by this dialog alone,
 *  so dropping it forces the next open to fetch the persisted detail from
 *  scratch.
 *
 * The detail-fetch no longer races the streaming bridge: the dialog's mount
 * fetch uses `preserveLive: true`, so `FETCH_DETAIL_SUCCESS` keeps the bridged
 * `liveMessage` instead of wiping it — no re-bridge effect is needed.
 *
 * One more case is handled explicitly: **reopen-after-completion.** If the
 * dialog mounts onto a child that already finished but whose connection still
 * holds its final `liveMessage` (kept for a short grace period after
 * completion), the streaming→settled `completeTurn` edge never fires and the
 * non-live mirror is rejected while the detail loads — so the
 * adopt-settled-reply effect promotes that retained reply directly, covering
 * the window before the persisted transcript catches up.
 */
function useChildLiveBridge(
  childConversationId: number,
  childConnState: ConnectionState | undefined
) {
  const { setLiveMessage, completeTurn, syncTurnMetadata, removeConversation } =
    useConversationRuntime()

  const connStatus = childConnState?.status ?? null
  const liveMessage = childConnState?.liveMessage ?? null

  // Backfill token usage / duration / model into the promoted reply once the
  // child's persisted transcript catches up. `completeTurn` lands the streamed
  // reply WITHOUT those fields — `buildStreamingTurnsFromLiveMessage` carries no
  // usage data; it comes from the DB parser — so without this the child's
  // post-stream stats row stays blank. Mirrors `conversation-detail-panel.tsx`:
  // a delayed, self-retrying DB roundtrip that PATCHes metadata onto the
  // existing `localTurns` (it never replaces them, so the kept live reply is not
  // blanked, unlike a `refetchDetail`). Cancel the previous sync before starting
  // a new one, and on dialog close, via the ref.
  const syncCancelRef = useRef<(() => void) | null>(null)
  const startMetadataSync = useCallback(() => {
    if (childConversationId <= 0) return
    syncCancelRef.current?.()
    syncCancelRef.current = syncTurnMetadata(childConversationId)
  }, [childConversationId, syncTurnMetadata])

  const connStatusRef = useRef(connStatus)
  useEffect(() => {
    connStatusRef.current = connStatus
  }, [connStatus])

  // When connStatus transitions away from "prompting", completeTurn snapshots
  // and promotes the live reply. This stays correct across the transition
  // because the mirror-live effect's cleanup gates on `connStatusRef` (which
  // still reads "prompting" at cleanup time, since React updates it only in a
  // later setup pass) rather than on effect declaration order. We also latch
  // whether we ever observed streaming this mount, so the adopt-settled-reply
  // effect below can tell a fresh "reopened after the child already finished"
  // mount from a normal streaming→settled handoff.
  const prevStatusRef = useRef(connStatus)
  const everPromptingRef = useRef(connStatus === "prompting")
  useEffect(() => {
    const wasPrompting = prevStatusRef.current === "prompting"
    prevStatusRef.current = connStatus
    if (connStatus === "prompting") everPromptingRef.current = true
    if (!wasPrompting || connStatus === "prompting") return
    completeTurn(childConversationId, liveMessage)
    startMetadataSync()
  }, [
    connStatus,
    liveMessage,
    childConversationId,
    completeTurn,
    startMetadataSync,
  ])

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

  // Adopt-settled-reply: handle reopening the dialog onto a child that ALREADY
  // finished but whose connection still carries its final liveMessage (kept for
  // CHILD_DETACH_GRACE_MS after completion to bridge DB lag). For such a mount
  // the streaming→settled completeTurn edge never fires (we never saw
  // "prompting"), and the non-live mirror above is rejected by the
  // SET_LIVE_MESSAGE guard while the mount fetch is loading — so without this
  // the final reply would vanish whenever the persisted transcript still lags
  // (empty / user-only / partial detail). Adopt the retained reply directly:
  // bridge it as live (a one-shot child's liveMessage is unambiguously its own
  // reply, never a stale reconnect replay) then promote it to a COMPLETED local
  // turn (no streaming affordance), where the `liveOwnsActiveTurn` projection
  // keeps it and dedupes the persisted copy once the DB catches up. Runs at most
  // once, and never when streaming was observed (that path promotes via the
  // settled edge).
  const adoptedRef = useRef(false)
  useEffect(() => {
    if (adoptedRef.current || everPromptingRef.current) return
    if (connStatus == null || connStatus === "prompting") return
    if (liveMessage == null) return
    adoptedRef.current = true
    setLiveMessage(childConversationId, liveMessage, true)
    completeTurn(childConversationId, liveMessage)
    startMetadataSync()
  }, [
    connStatus,
    liveMessage,
    childConversationId,
    setLiveMessage,
    completeTurn,
    startMetadataSync,
  ])

  // Full teardown on dialog close: cancel any in-flight metadata sync, then
  // drop the runtime session so the next open starts from a fresh `fetchDetail`
  // instead of stale bridged state.
  useEffect(() => {
    return () => {
      syncCancelRef.current?.()
      syncCancelRef.current = null
      removeConversation(childConversationId)
    }
  }, [childConversationId, removeConversation])
}

export function SubAgentSessionDialog({
  open,
  onOpenChange,
  childConversationId,
  childConnectionId,
  agentType,
  kickoffTask,
}: Props) {
  const t = useTranslations("Folder.chat.delegation")

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        closeButtonClassName="top-2 right-2"
        className="flex h-[85vh] w-full max-w-3xl flex-col gap-0 overflow-hidden rounded-2xl p-0 lg:max-w-4xl"
      >
        <DialogTitle className="sr-only">{t("detailTitle")}</DialogTitle>
        <DialogDescription className="sr-only">
          {t("detailDescription")}
        </DialogDescription>
        {open ? (
          <SubAgentSessionBody
            childConversationId={childConversationId}
            childConnectionId={childConnectionId}
            agentType={agentType}
            kickoffTask={kickoffTask}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  )
}

function SubAgentSessionBody({
  childConversationId,
  childConnectionId,
  agentType,
  kickoffTask,
}: {
  childConversationId: number
  childConnectionId: string | null
  agentType: AgentType | null
  kickoffTask?: string | null
}) {
  const t = useTranslations("Folder.chat.delegation")

  const childConn = useChildConnectionState(childConnectionId)
  const connStatus = childConn?.status ?? null
  const isChildStreaming = connStatus === "prompting"

  const { refetchDetail, setLiveOwnsActiveTurn } = useConversationRuntime()

  // Enter delegation-child viewer mode: mark the session live-owned and record
  // the known kickoff task. `getTimelineTurns` then (a) synthesizes the kickoff
  // user turn from this text while the persisted transcript still lags the live
  // stream, so the user message shows immediately, and (b) strips the persisted
  // copy of the reply while the live/local reply is present, so it never
  // duplicates the stream. Re-applies if `kickoffTask` resolves late (harmless).
  useEffect(() => {
    setLiveOwnsActiveTurn(childConversationId, true, kickoffTask ?? null)
  }, [childConversationId, kickoffTask, setLiveOwnsActiveTurn])

  // Single persisted-detail fetch on mount, always `preserveLive: true` so the
  // bridged/promoted reply is never wiped — the render-time projection above
  // handles dedup against the persisted copy. No settle-time refetch: when the
  // child finishes, `completeTurn` promotes its (complete) live reply into
  // localTurns, which the projection keeps showing; replacing it from the DB
  // would race the still-lagging transcript and could blank the reply.
  useEffect(() => {
    refetchDetail(childConversationId, { preserveLive: true })
  }, [childConversationId, refetchDetail])

  // Reader only — its built-in auto-fetch is disabled; the effect above is
  // the sole fetch path.
  const { loading, error, acpLoadError } = useConversationDetail(
    childConversationId,
    { enabled: false }
  )

  // While streaming, mask loading as false: the live bridge owns the reply and
  // the synthesized kickoff covers the user turn, so we don't want a skeleton
  // over the live stream. Passed to MessageListView only.
  const detailLoading = isChildStreaming ? false : loading

  useChildLiveBridge(childConversationId, childConn)

  // The child runs with the user's configured permission level, so it may
  // raise a permission request. The parent card no longer answers it inline
  // (it only badges "awaiting approval"); this dialog is where the user
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
          detailLoading={detailLoading}
          detailError={error}
          acpLoadError={acpLoadError}
          hideEmptyState={false}
          showMessageNav={false}
        />
      </div>
    </div>
  )
}
