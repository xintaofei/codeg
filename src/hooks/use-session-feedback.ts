"use client"

/**
 * State + actions for the live-feedback ("steering") side channel, lifted out of
 * the old always-on composer bar. The user opens a dialog (from the composer
 * "+" menu) and sends a short note to the running agent; it is delivered the next
 * time the agent calls the `check_user_feedback` MCP tool. Sent notes render as
 * read-only rows above the composer, flipping from "waiting" to "received" once
 * the agent reads them.
 *
 * Cooperative by design: the agent must call the tool to see a note, so this is
 * a side channel, not a hard interrupt. If the turn ends between opening the
 * dialog and sending, the note is rerouted through the message queue
 * (`onResendAsPrompt`) so it is never silently dropped.
 *
 * State is hydrated from the session snapshot on mount / connection change (so a
 * refresh or a second mid-turn viewer recovers pending notes) and then kept live
 * via the `feedback_submitted` / `feedback_consumed` event stream. Consumed-id
 * tombstones reconcile a consume event that races ahead of hydration.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { useAcpEvent } from "@/contexts/acp-connections-context"
import { acpGetSessionSnapshot, submitSessionFeedback } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { isNoActiveTurnRejection } from "@/lib/turn-busy"
import type { ConnectionStatus, FeedbackItem } from "@/lib/types"

/** Merge snapshot-hydrated notes with live ones, keyed by id; live entries win
 *  (they carry the most recent status). Snapshot order first, live-only after. */
function mergeNotes(
  base: FeedbackItem[],
  live: FeedbackItem[]
): FeedbackItem[] {
  const byId = new Map<string, FeedbackItem>()
  for (const n of base) byId.set(n.id, n)
  for (const n of live) byId.set(n.id, n)
  return [...byId.values()]
}

export interface UseSessionFeedbackArgs {
  connectionId: string | null
  connStatus: ConnectionStatus | null
  /** Whether the live-feedback feature is enabled (global setting). */
  enabled: boolean
  /** Reroute a note as an ordinary prompt when the turn ended before it could be
   *  submitted (turn-end race). */
  onResendAsPrompt?: (text: string) => void
}

export interface UseSessionFeedback {
  /** All notes for the current turn (pending + delivered). */
  notes: FeedbackItem[]
  /** Global feature flag — gates whether the "+" menu entry is shown at all. */
  featureEnabled: boolean
  /** Whether a note can be sent right now (entry is enabled vs. greyed out). */
  canSubmit: boolean
  /** Whether to render the read-only notes list above the composer. */
  showList: boolean
  /** Whether a submit is in flight (disables the dialog send button). */
  submitting: boolean
  dialogOpen: boolean
  openDialog: () => void
  closeDialog: () => void
  /** Send a note. Closes the dialog on success / turn-end reroute. */
  submit: (text: string) => Promise<void>
}

export function useSessionFeedback({
  connectionId,
  connStatus,
  enabled,
  onResendAsPrompt,
}: UseSessionFeedbackArgs): UseSessionFeedback {
  const t = useTranslations("LiveFeedback")
  const [notes, setNotes] = useState<FeedbackItem[]>([])
  const [submitting, setSubmitting] = useState(false)
  const [dialogOpen, setDialogOpen] = useState(false)
  // Whether THIS agent actually has the `check_user_feedback` tool (from the
  // snapshot). The authoritative gate — enabling the feature mid-session can't
  // retrofit the tool onto an already-running agent. Starts false until the
  // snapshot confirms.
  const [toolAvailable, setToolAvailable] = useState(false)
  // Tombstones for notes consumed via `feedback_consumed` whose
  // `feedback_submitted` we never held (a consume event that lands BEFORE the
  // matching submit — e.g. before snapshot hydration resolves, or out-of-order
  // broadcast). Applied so a stale snapshot or a late submit can't resurrect a
  // note as `pending` after the agent already read it.
  const consumedRef = useRef<Map<string, string>>(new Map())
  // Bumped on every new turn (`user_message`). A snapshot fetch captures the
  // generation it started in; if a new turn lands before it resolves, its
  // (previous-turn) notes are discarded — feedback is turn-scoped and the new
  // turn already cleared them, so applying the stale snapshot would resurrect
  // them.
  const turnGenRef = useRef(0)

  const isPrompting = connStatus === "prompting"

  // Reset on connection change, then hydrate from the snapshot: recover pending
  // notes (a refresh / second mid-turn viewer won't get the one-shot
  // `feedback_submitted` events) AND read the agent's real feedback-tool
  // capability. Live events arriving before the fetch resolves are preserved
  // (live wins in the merge); consumed-id tombstones override stale `pending`.
  useEffect(() => {
    setNotes([])
    setToolAvailable(false)
    consumedRef.current = new Map()
    if (!enabled || !connectionId) return
    let cancelled = false
    const startGen = turnGenRef.current
    void acpGetSessionSnapshot(connectionId)
      .then((snap) => {
        if (cancelled || !snap) return
        // Tool availability is fixed at launch and only ever upgrades to true.
        // Never overwrite a confirmed `true` with a stale `false` from a read
        // that raced the spawn — the synchronous reset above is the only place
        // it goes back to false (on connection / feature-flag change).
        if (snap.feedback_tool_available) setToolAvailable(true)
        // A new turn started while the fetch was in flight — the snapshot holds
        // the previous turn's (already-cleared) notes; drop them.
        if (turnGenRef.current !== startGen) return
        const hydrated = snap.feedback ?? []
        if (hydrated.length === 0) return
        const reconciled = hydrated.map((n) => {
          const at = consumedRef.current.get(n.id)
          return at
            ? { ...n, status: "delivered" as const, delivered_at: at }
            : n
        })
        setNotes((prev) => mergeNotes(reconciled, prev))
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [connectionId, enabled])

  // Self-heal tool availability. The hydrate above is keyed on `connectionId`,
  // which appears the moment a NEW conversation's connection is created — while
  // it's still "connecting", BEFORE the backend sets `feedback_tool_available`
  // at spawn. That first read returns false and would never refresh, leaving the
  // "+" entry permanently disabled. Once the connection is actually live, re-read
  // it (only while still unknown — a `false` no-ops so this can't loop, and it
  // stops the moment it flips true).
  useEffect(() => {
    if (!enabled || !connectionId || toolAvailable) return
    if (connStatus !== "connected" && connStatus !== "prompting") return
    let cancelled = false
    void acpGetSessionSnapshot(connectionId)
      .then((snap) => {
        // Monotonic upgrade only (see hydrate effect) — no downgrade, no flicker.
        if (!cancelled && snap?.feedback_tool_available) {
          setToolAvailable(true)
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [enabled, connectionId, connStatus, toolAvailable])

  // Build the note list from the live event stream, scoped to this connection.
  useAcpEvent(
    useCallback(
      (envelope) => {
        if (envelope.connection_id !== connectionId) return
        switch (envelope.type) {
          case "feedback_submitted": {
            // If a consume already arrived for this id (out-of-order broadcast),
            // honor the tombstone so it never shows as pending.
            const at = consumedRef.current.get(envelope.item.id)
            const item: FeedbackItem = at
              ? { ...envelope.item, status: "delivered", delivered_at: at }
              : envelope.item
            setNotes((prev) =>
              prev.some((n) => n.id === item.id) ? prev : [...prev, item]
            )
            break
          }
          case "feedback_consumed": {
            const ids = new Set(envelope.ids)
            const at = envelope.delivered_at
            for (const id of envelope.ids) consumedRef.current.set(id, at)
            setNotes((prev) =>
              prev.map((n) =>
                ids.has(n.id)
                  ? { ...n, status: "delivered", delivered_at: at }
                  : n
              )
            )
            break
          }
          case "user_message": {
            // A new turn started — notes are turn-scoped, mirror the backend
            // clear so a fresh turn begins empty. Bump the generation so an
            // in-flight snapshot fetch from the previous turn can't re-add them.
            turnGenRef.current += 1
            setNotes([])
            consumedRef.current = new Map()
            break
          }
        }
      },
      [connectionId]
    )
  )

  const submit = useCallback(
    async (rawText: string) => {
      const text = rawText.trim()
      if (!text || submitting || !connectionId) return
      // Eligibility can drop while the dialog is open (e.g. the feature is
      // toggled off in another window). Don't send into a disabled / unsupported
      // session — close the dialog instead. NOTE: a merely-ended turn keeps
      // `enabled`/`toolAvailable` true, so it still flows to the submit below and
      // gets rerouted via the no-active-turn fallback (draft preserved).
      if (!enabled || !toolAvailable) {
        setDialogOpen(false)
        return
      }
      setSubmitting(true)
      try {
        const item = await submitSessionFeedback(connectionId, text)
        // Optimistically add; the broadcast event dedups against this by id.
        setNotes((prev) =>
          prev.some((n) => n.id === item.id) ? prev : [...prev, item]
        )
        setDialogOpen(false)
      } catch (err: unknown) {
        if (isNoActiveTurnRejection(err) && onResendAsPrompt) {
          // The turn ended between opening the dialog and sending. Fall back to
          // a normal prompt so the user's intent isn't lost.
          onResendAsPrompt(text)
          setDialogOpen(false)
          toast.info(t("turnEndedResent"))
        } else {
          toast.error(t("submitFailed"), { description: toErrorMessage(err) })
        }
      } finally {
        setSubmitting(false)
      }
    },
    [submitting, connectionId, enabled, toolAvailable, onResendAsPrompt, t]
  )

  const openDialog = useCallback(() => setDialogOpen(true), [])
  const closeDialog = useCallback(() => setDialogOpen(false), [])

  const canSubmit =
    enabled && Boolean(connectionId) && toolAvailable && isPrompting
  const showList = notes.length > 0 && isPrompting

  return useMemo(
    () => ({
      notes,
      featureEnabled: enabled,
      canSubmit,
      showList,
      submitting,
      dialogOpen,
      openDialog,
      closeDialog,
      submit,
    }),
    [
      notes,
      enabled,
      canSubmit,
      showList,
      submitting,
      dialogOpen,
      openDialog,
      closeDialog,
      submit,
    ]
  )
}
