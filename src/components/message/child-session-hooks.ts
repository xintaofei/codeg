/**
 * Shared hooks for viewing an externally-owned ACP session read-only — a
 * delegation sub-agent (`SubAgentSessionDialog`) or a loop iteration
 * (`IterationDialog`). Both mirror a connection another producer owns (the
 * delegation broker, or the loop engine) into the runtime session so a
 * `MessageListView` can stream it live without driving the owner's turns.
 *
 * Extracted verbatim from `sub-agent-session-dialog.tsx` so the loop iteration
 * viewer reuses exactly the same streaming/teardown semantics.
 */

import { useCallback, useEffect, useRef, useSyncExternalStore } from "react"

import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import {
  useConnectionStore,
  type ConnectionState,
} from "@/contexts/acp-connections-context"

export function useChildConnectionState(
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
export function useChildLiveBridge(
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
