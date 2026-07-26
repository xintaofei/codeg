"use client"

/**
 * Viewer for a delegated sub-agent's full conversation.
 *
 * Opens from `DelegatedSubThread`'s header and renders the same
 * `MessageListView` used by the main conversation panel, but without
 * the input bar, send signal, or reload/new-session handlers — so the
 * user can scroll the transcript without driving the child's turns. The
 * interactions it hosts are the child's blocking prompts that resolve
 * WITHOUT driving a new turn: the permission request (the child runs at
 * the user's configured permission level), and the codeg-mcp
 * `ask_user_question` multiple-choice card. Both are answered through the
 * CHILD connection id; the backend routes the response to the child's
 * parked tool call. The parent card itself stays non-interactive (it only
 * badges "awaiting approval"). The legacy free-text `pendingQuestion` path
 * is intentionally NOT hosted here — it is answered by sending a prompt,
 * which this read-only viewer deliberately cannot do.
 *
 * Streaming: while the dialog is open, the child connection's live
 * message and status (from `acp-connections-context`) are mirrored
 * into the runtime session for the child `conversationId` so the
 * `MessageListView` shows real-time deltas. The bridge runs only
 * while the dialog is mounted; once it closes, no further mirroring
 * happens. Persistence of completed turns comes from the broker's
 * own DB writes, surfaced via `useConversationDetail`.
 */

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react"
import { ExternalLink, Info, SendHorizontal } from "lucide-react"
import { useTranslations } from "next-intl"

import {
  continueDelegation,
  getContinuationAvailability,
  type ContinuationAvailability,
} from "@/lib/api"

import { AgentIcon } from "@/components/agent-icon"
import { MessageListView } from "@/components/message/message-list-view"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import { useTabActions } from "@/stores/tab-store"
import { useConversationRuntimeActions } from "@/stores/conversation-runtime-store"
import {
  useAcpActions,
  useAcpEvent,
  useConnectionStore,
  type ConnectionState,
} from "@/contexts/acp-connections-context"
import { PermissionDialog } from "@/components/chat/permission-dialog"
import { AskQuestionCard } from "@/components/chat/ask-question-card"
import { PlanApprovalCard } from "@/components/chat/plan-approval-card"
import {
  AGENT_LABELS,
  type AgentType,
  type PlanApprovalAnswer,
  type QuestionAnswer,
} from "@/lib/types"

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
    useConversationRuntimeActions()

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
  // later setup pass) rather than on effect declaration order.
  //
  // Promotion bookkeeping is PER ROUND, keyed by the reply's `liveMessage.id`
  // (minted fresh per prompt cycle), because a delegation child is continuable
  // (Requirement 4.7): a mount-wide latch would make every path below dead
  // after round one. `streamedReplyIds` records the replies we saw stream, so
  // the adopt effect can tell them from a round that was already settled when
  // we first observed it; `promotedReplyIds` records what has been promoted, so
  // the two paths never double-count one round. Both are bounded by the number
  // of rounds taken while the dialog stays open.
  const prevStatusRef = useRef(connStatus)
  const streamedReplyIdsRef = useRef(new Set<string>())
  const promotedReplyIdsRef = useRef(new Set<string>())
  useEffect(() => {
    const wasPrompting = prevStatusRef.current === "prompting"
    prevStatusRef.current = connStatus
    if (connStatus === "prompting") {
      if (liveMessage != null) streamedReplyIdsRef.current.add(liveMessage.id)
      return
    }
    if (!wasPrompting) return
    if (liveMessage != null) promotedReplyIdsRef.current.add(liveMessage.id)
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

  // Adopt-settled-reply: handle observing a child reply that is ALREADY settled
  // but whose connection still carries it (kept for CHILD_DETACH_GRACE_MS after
  // completion to bridge DB lag) — either because the dialog was reopened onto a
  // finished child, or because a continued round's streaming→settled edge never
  // reached us (events coalesced). For such a reply the completeTurn edge above
  // never fires and the non-live mirror is rejected by the SET_LIVE_MESSAGE
  // guard while the mount fetch is loading — so without this the reply would
  // vanish whenever the persisted transcript still lags (empty / user-only /
  // partial detail). Adopt it directly: bridge it as live (the child's retained
  // liveMessage is unambiguously its own latest reply, never a stale reconnect
  // replay) then promote it to a COMPLETED local turn (no streaming affordance),
  // where the `liveOwnsActiveTurn` projection keeps it and dedupes the persisted
  // copy once the DB catches up.
  //
  // Per-round, not per-mount (Requirement 4.7): a reply is adopted at most once
  // (`promotedReplyIds`), and never when we watched it stream (that path
  // promotes via the settled edge), but a LATER round is still eligible.
  useEffect(() => {
    if (connStatus == null || connStatus === "prompting") return
    if (liveMessage == null) return
    if (promotedReplyIdsRef.current.has(liveMessage.id)) return
    if (streamedReplyIdsRef.current.has(liveMessage.id)) return
    promotedReplyIdsRef.current.add(liveMessage.id)
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
            onCloseRequest={() => onOpenChange(false)}
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
  onCloseRequest,
}: {
  childConversationId: number
  childConnectionId: string | null
  agentType: AgentType | null
  kickoffTask?: string | null
  /** Dismiss the dialog once the child has been handed off to a full tab. */
  onCloseRequest: () => void
}) {
  const t = useTranslations("Folder.chat.delegation")

  const childConn = useChildConnectionState(childConnectionId)
  const connStatus = childConn?.status ?? null
  const isChildStreaming = connStatus === "prompting"

  const { refetchDetail, setLiveOwnsActiveTurn } =
    useConversationRuntimeActions()

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
  // handles dedup against the persisted copy. No refetch on the live bridge's
  // own settle edge: `completeTurn` promotes the (complete) live reply into
  // localTurns and a DB refetch would race the still-lagging transcript. The
  // ONE settle-time refetch is the session-update effect below, which covers
  // rounds the bridge never saw — and stays `preserveLive` for the same
  // reason.
  useEffect(() => {
    refetchDetail(childConversationId, { preserveLive: true })
  }, [childConversationId, refetchDetail])

  // Settle-time transcript refresh for CONTINUED rounds: when a continuation
  // of THIS child settles (`delegation_session_update` — the 2.8a replacement
  // for the suppressed second completion), re-fetch the persisted detail.
  // The live bridge only covers rounds whose connection events reach this
  // dialog; a round it missed (user-origin settle, coalesced events) would
  // otherwise stay frozen at the mount-time snapshot until reopen.
  // `preserveLive: true` keeps any bridged reply, same as the mount fetch.
  useAcpEvent(
    useCallback(
      (envelope) => {
        if (
          envelope.type === "delegation_session_update" &&
          envelope.child_conversation_id === childConversationId
        ) {
          refetchDetail(childConversationId, { preserveLive: true })
        }
      },
      [childConversationId, refetchDetail]
    )
  )

  // Reader only — its built-in auto-fetch is disabled; the effect above is
  // the sole fetch path. `detail.summary` is also the only place the child's
  // folder id is available (the card knows the conversation id, not the folder),
  // so "Open in tab" stays disabled until the persisted summary lands.
  const { detail, loading, error, acpLoadError } = useConversationDetail(
    childConversationId,
    { enabled: false }
  )

  const { openTab } = useTabActions()
  const childFolderId = detail?.summary.folder_id ?? null
  const tabAgentType = detail?.summary.agent_type ?? agentType
  const handleOpenInTab = useCallback(() => {
    if (childFolderId == null || tabAgentType == null) return
    // Same path the sidebar uses for a sub-session row (`handleSelect` →
    // `openTab`); openTab itself brings the conversation pane forward via its
    // `activateConversationPane` side effect, so no route call is needed here.
    openTab(childFolderId, childConversationId, tabAgentType)
    onCloseRequest()
  }, [
    childFolderId,
    tabAgentType,
    childConversationId,
    openTab,
    onCloseRequest,
  ])

  // While streaming, mask loading as false: the live bridge owns the reply and
  // the synthesized kickoff covers the user turn, so we don't want a skeleton
  // over the live stream. Passed to MessageListView only.
  const detailLoading = isChildStreaming ? false : loading

  useChildLiveBridge(childConversationId, childConn)

  // The child runs with the user's configured permission level, so it may
  // raise a permission request. The parent card no longer answers it inline
  // (it only badges "awaiting approval"); this dialog is where the user
  // resolves it. Route the response through the CHILD connection id.
  const { respondPermission, answerQuestion, answerPlanApproval } =
    useAcpActions()
  const childPendingPermission = childConn?.pendingPermission ?? null
  const onRespondPermission = useCallback(
    (requestId: string, optionId: string) => {
      if (!childConnectionId) return
      void respondPermission(childConnectionId, requestId, optionId)
    },
    [childConnectionId, respondPermission]
  )

  // The child may also call the codeg-mcp `ask_user_question` tool, raising the
  // interactive multiple-choice card. Mirror the permission path: surface the
  // live `pendingAskQuestion` from the CHILD connection and route the answer
  // back through the same child connection id. `answerQuestion` rejects on
  // failure so AskQuestionCard can show a retryable inline error; it resolves
  // the parked MCP tool without driving a new turn (so it fits this read-only
  // viewer, unlike the prompt-driven free-text question path).
  const childPendingAskQuestion = childConn?.pendingAskQuestion ?? null
  const onAnswerAskQuestion = useCallback(
    (questionId: string, answer: QuestionAnswer) => {
      if (!childConnectionId) return
      return answerQuestion(childConnectionId, questionId, answer)
    },
    [childConnectionId, answerQuestion]
  )

  // A child Grok session may enter plan mode and call `exit_plan_mode`, raising
  // the blocking plan-approval card. Same shape as the ask path: surface the
  // child's live `pendingPlanApproval` and route the decision back through the
  // child connection id so the delegated agent isn't left blocked with no card.
  const childPendingPlanApproval = childConn?.pendingPlanApproval ?? null
  const onAnswerPlanApproval = useCallback(
    (approvalId: string, answer: PlanApprovalAnswer) => {
      if (!childConnectionId) return
      return answerPlanApproval(childConnectionId, approvalId, answer)
    },
    [childConnectionId, answerPlanApproval]
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
        {/* Hand off to the child's own workspace tab, where it is a fully
            interactive panel. Disabled until the persisted summary lands —
            that is the only source of the child's folder id, and openTab keys
            its dedupe on (folderId, conversationId, agentType). */}
        <button
          type="button"
          onClick={handleOpenInTab}
          disabled={childFolderId == null || tabAgentType == null}
          title={t("openInTab")}
          className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs font-medium text-foreground/80 transition-colors hover:bg-muted/60 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
        >
          <ExternalLink className="h-3.5 w-3.5" aria-hidden />
          {t("openInTab")}
        </button>
      </div>
      {/* M1 capability boundary (design.md §M1 · R1-A7): the full tab sends
          straight to the child's ACP connection, bypassing the delegation
          broker — so the parent AI never learns about those turns. State it up
          front instead of letting the user discover it. */}
      <div className="flex items-start gap-2 border-b border-border bg-muted/40 px-5 py-2 text-xs text-muted-foreground">
        <Info className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
        <span>{t("openInTabNotSyncedNotice")}</span>
      </div>
      {childPendingPermission && (
        <div className="border-b border-border px-4 py-3">
          <PermissionDialog
            permission={childPendingPermission}
            onRespond={onRespondPermission}
          />
        </div>
      )}
      {childConnectionId &&
        childPendingAskQuestion &&
        childPendingAskQuestion.questions.length > 0 && (
          <div className="border-b border-border px-4 py-3">
            <AskQuestionCard
              question={childPendingAskQuestion}
              onAnswer={onAnswerAskQuestion}
            />
          </div>
        )}
      {childConnectionId && childPendingPlanApproval && (
        <div className="border-b border-border px-4 py-3">
          <PlanApprovalCard
            key={childPendingPlanApproval.approval_id}
            approval={childPendingPlanApproval}
            onAnswer={onAnswerPlanApproval}
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
      <SubAgentContinuationComposer childConversationId={childConversationId} />
    </div>
  )
}

/**
 * User-side continuation input (Task 5.3 · Requirement 4.1/4.6).
 *
 * Unlike the full tab (whose sends bypass the broker — see the notice bar
 * above), everything submitted here goes through `continueDelegation`, so the
 * turn lands under the same `task_id` the parent AI tracks and shows up in
 * its next `get_delegation_status`.
 *
 * Availability drives the input per design §D4's five tiers; the verdict is
 * re-queried on mount, whenever a delegation event for THIS child arrives
 * (`delegation_completed` / `delegation_session_update`), and after a failed
 * send. The frontend never derives the tier itself (no parallel truth).
 *
 * Idempotency: one `continuation_id` is minted per submission and REUSED when
 * retrying the same text (a broker replay then returns the first report
 * instead of double-dispatching). Editing the text is a new submission and
 * gets a fresh id — reusing the old one would be a `continuation_conflict`.
 */
function SubAgentContinuationComposer({
  childConversationId,
}: {
  childConversationId: number
}) {
  const t = useTranslations("Folder.chat.delegation")
  const [availability, setAvailability] =
    useState<ContinuationAvailability | null>(null)
  const [text, setText] = useState("")
  const [sending, setSending] = useState(false)
  const [errorCode, setErrorCode] = useState<string | null>(null)
  const pendingOpRef = useRef<{ id: string; message: string } | null>(null)

  // Latest-wins guard: a slow older query must not clobber a newer verdict.
  const querySeqRef = useRef(0)
  const refreshAvailability = useCallback(() => {
    const seq = ++querySeqRef.current
    getContinuationAvailability(childConversationId)
      .then((verdict) => {
        if (querySeqRef.current === seq) setAvailability(verdict)
      })
      .catch(() => {
        // Query failure: keep the last known verdict rather than flashing the
        // input off; the next refresh trigger re-tries.
      })
  }, [childConversationId])

  useEffect(() => {
    refreshAvailability()
  }, [refreshAvailability])

  useAcpEvent(
    useCallback(
      (envelope) => {
        if (
          (envelope.type === "delegation_completed" ||
            envelope.type === "delegation_session_update") &&
          envelope.child_conversation_id === childConversationId
        ) {
          refreshAvailability()
        }
      },
      [childConversationId, refreshAvailability]
    )
  )

  const handleSend = useCallback(async () => {
    const message = text.trim()
    if (!message || sending) return
    const pending = pendingOpRef.current
    const continuationId =
      pending && pending.message === message ? pending.id : crypto.randomUUID()
    pendingOpRef.current = { id: continuationId, message }
    setSending(true)
    setErrorCode(null)
    try {
      const report = await continueDelegation(
        childConversationId,
        message,
        continuationId
      )
      if (report.error_code) {
        // Refusal (session_still_running / released / not_continuable / …):
        // surface the stable code, keep the draft + continuation id so a
        // retry of the SAME submission stays idempotent, and re-sync the
        // availability verdict.
        setErrorCode(report.error_code)
        refreshAvailability()
        return
      }
      // Accepted — the turn is running under the parent's task id.
      pendingOpRef.current = null
      setText("")
      setAvailability("running")
    } catch {
      // Transport-level failure (backend unreachable). Keep draft + id for an
      // idempotent retry.
      setErrorCode("transport_error")
      refreshAvailability()
    } finally {
      setSending(false)
    }
  }, [text, sending, childConversationId, refreshAvailability])

  const inputEnabled =
    availability === "continuable_live" || availability === "continuable_resume"
  const notice =
    availability === "running"
      ? t("continueUnavailableRunning")
      : availability === "released"
        ? t("continueUnavailableReleased")
        : availability === "not_continuable"
          ? t("continueUnavailableNotContinuable")
          : availability === "continuable_resume"
            ? t("continueResumeHint")
            : null

  return (
    <div
      data-testid="continuation-composer"
      className="border-t border-border px-4 py-3"
    >
      {notice && <p className="mb-2 text-xs text-muted-foreground">{notice}</p>}
      {errorCode && (
        <p role="alert" className="mb-2 text-xs text-destructive">
          {t("continueRejected", { code: errorCode })}
        </p>
      )}
      <form
        className="flex items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault()
          void handleSend()
        }}
      >
        <textarea
          data-testid="continuation-input"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault()
              void handleSend()
            }
          }}
          placeholder={t("continueInputPlaceholder")}
          disabled={!inputEnabled || sending}
          rows={2}
          className="min-h-0 flex-1 resize-none rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
        />
        <button
          type="submit"
          data-testid="continuation-send"
          disabled={!inputEnabled || sending || text.trim() === ""}
          title={t("continueSend")}
          className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border px-3 py-2 text-xs font-medium text-foreground/80 transition-colors hover:bg-muted/60 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
        >
          <SendHorizontal className="h-3.5 w-3.5" aria-hidden />
          {t("continueSend")}
        </button>
      </form>
    </div>
  )
}
