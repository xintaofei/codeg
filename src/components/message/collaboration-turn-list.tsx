"use client"

/**
 * Read-only list of collaboration turns (rework rounds) for one delegation
 * child — rendered inside `SubAgentSessionDialog` above the live transcript.
 *
 * Data comes exclusively from `get_collaboration_session` (the shared
 * read-only core); this component NEVER talks to the agent. While the dialog
 * is open it polls at most once per second (v2 design §6) and stops the
 * moment it closes — the interval lives in an effect keyed on `open`, and the
 * in-flight/seq gates (same pattern as `subagent-session-dialog.tsx`) drop
 * stale responses. Version ratcheting keeps an older snapshot from
 * overwriting a newer one, per turn.
 */

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ChevronDown, ChevronUp, Loader2 } from "lucide-react"

import {
  ACTIVE_TURN_STATES,
  COLLAB_SNAPSHOT_DEFAULT_LIMIT,
  getCollaborationSession,
  type CollaborationSnapshot,
  type TurnReport,
  type TurnState,
} from "@/lib/collaboration"

/** Poll cadence while the drawer is open — never faster than 1 Hz. */
const POLL_INTERVAL_MS = 1000

interface Props {
  open: boolean
  /** The child conversation whose drawer this list renders in. */
  childConversationId: number
  /** The parent conversation that owns the source task (scoping). */
  parentConversationId: number
  /** The frozen delegation task this collaboration continues. */
  sourceTaskId: string
}

/** Badge variant + emphasis per turn state — presentation only. */
function stateBadgeTone(state: TurnState): {
  className: string
  done: boolean
} {
  if (ACTIVE_TURN_STATES.includes(state)) {
    return {
      className:
        "border-blue-500/40 bg-blue-500/10 text-blue-600 dark:text-blue-400",
      done: false,
    }
  }
  switch (state) {
    case "completed":
      return {
        className:
          "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
        done: true,
      }
    case "failed":
    case "canceled":
    case "interrupted":
    case "outcome_unknown":
      return {
        className:
          "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400",
        done: true,
      }
  }
  // Exhaustive for terminal states; active states returned above.
  return { className: "border-border text-muted-foreground", done: true }
}

export function CollaborationTurnList({
  open,
  parentConversationId,
  sourceTaskId,
}: Props) {
  const t = useTranslations("Folder.chat.delegation")
  const [snapshot, setSnapshot] = useState<CollaborationSnapshot | null>(null)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const [loadingMore, setLoadingMore] = useState(false)

  // Poll plumbing: at most one in-flight request, and a monotonically
  // increasing sequence so a late response can never overwrite a newer one.
  const inFlightRef = useRef(false)
  const seqRef = useRef(0)
  // Per-turn version high-water marks: apply a response only when EVERY
  // turn it carries is at least as new as what we already rendered.
  const versionWatermarkRef = useRef<Map<string, number>>(new Map())
  // The turns currently rendered (mirrored from every snapshot write). React
  // defers `setSnapshot` updaters, so any decision that must be made in the
  // same tick as the response (the R9 follow-up below) reads this instead of
  // waiting for the updater to run.
  const turnsRef = useRef<TurnReport[]>([])

  // The ONE merge used by every response kind (poll, follow-up, pagination):
  // version-ratcheted per turn ID. An incoming turn older than the rendered
  // watermark keeps the rendered one; an incoming turn the caller has never
  // seen replaces/inserts; turns the response omits are kept. Sorting by
  // ordinal keeps the list stable. Sharing this is what stops a history page
  // that ALSO carries the projected active round from duplicating it
  // (reacceptance R10) and keeps pagination responses version-checked.
  const applyTurns = useCallback(
    (incoming: TurnReport[], prev: TurnReport[] | undefined): TurnReport[] => {
      const watermark = versionWatermarkRef.current
      const accept = (turn: TurnReport): TurnReport => {
        const known = watermark.get(turn.turn_id)
        if (known !== undefined && turn.version < known) {
          const older = prev?.find((t) => t.turn_id === turn.turn_id)
          return older ?? turn
        }
        watermark.set(turn.turn_id, turn.version)
        return turn
      }
      const accepted = incoming.map(accept)
      if (!prev) {
        return [...accepted].sort((a, b) => a.ordinal - b.ordinal)
      }
      const incomingIds = new Set(accepted.map((t) => t.turn_id))
      const kept = prev.filter((t) => !incomingIds.has(t.turn_id)).map(accept)
      return [...accepted, ...kept].sort((a, b) => a.ordinal - b.ordinal)
    },
    []
  )

  const load = useCallback(
    async (initial: boolean) => {
      if (!open) return
      if (!initial && inFlightRef.current) return
      const seq = ++seqRef.current
      inFlightRef.current = true
      try {
        const fresh = await getCollaborationSession({
          parentConversationId,
          sourceTaskId,
        })
        if (seq !== seqRef.current) return // a newer load superseded this one
        // Known NON-TERMINAL turns the fresh page did NOT include (reacceptance
        // R9): a projected active round that reached a terminal beyond the
        // first page's window disappears from every later first-page response,
        // so without a follow-up fetch it would render as running forever.
        // Computed from the turns ALREADY rendered (the ref) — the snapshot
        // updater below runs later and cannot feed this decision.
        const freshIds = new Set(fresh.turns.map((t) => t.turn_id))
        const unsettledOrdinals = turnsRef.current
          .filter(
            (turn) =>
              ACTIVE_TURN_STATES.includes(turn.state) &&
              !freshIds.has(turn.turn_id)
          )
          .map((turn) => turn.ordinal)
        setSnapshot((prev) => {
          // Version ratchet per turn — maintained on EVERY load including the
          // first (acceptance F11: an uninitialized watermark let the first
          // poll roll a completed v2 back to a stale running v1) — and a
          // turn-ID merge that PRESERVES loaded history pages (acceptance
          // F12: the poll only re-reads the first page; a page 2 the user
          // loaded must not vanish on the next tick).
          const merged = applyTurns(fresh.turns, prev?.turns)
          turnsRef.current = merged
          // The deepest cursor wins: pagination may have walked further back
          // than the poll's first page.
          const nextAfter =
            !prev || prev.next_after_ordinal === null
              ? fresh.next_after_ordinal
              : fresh.next_after_ordinal === null
                ? prev.next_after_ordinal
                : Math.max(prev.next_after_ordinal, fresh.next_after_ordinal)
          return {
            ...fresh,
            session: fresh.session ?? prev?.session ?? null,
            turns: merged,
            next_after_ordinal: nextAfter,
          }
        })
        // Follow-up fetch for the missing active rounds: query from just
        // below the lowest unsettled ordinal so the response covers it (the
        // backend also appends any still-active round to the page).
        if (unsettledOrdinals.length > 0) {
          const afterOrdinal = Math.max(0, Math.min(...unsettledOrdinals) - 1)
          const followUp = await getCollaborationSession({
            parentConversationId,
            sourceTaskId,
            afterOrdinal,
            limit: COLLAB_SNAPSHOT_DEFAULT_LIMIT,
          })
          if (seq !== seqRef.current) return
          setSnapshot((prev) => {
            if (!prev) return prev
            const merged = applyTurns(followUp.turns, prev.turns)
            turnsRef.current = merged
            return { ...prev, turns: merged }
          })
        }
      } catch {
        // Read-only surface: a failed poll keeps the last snapshot. The next
        // tick retries; the drawer never blocks on this.
      } finally {
        if (seq === seqRef.current) inFlightRef.current = false
      }
    },
    [open, parentConversationId, sourceTaskId, applyTurns]
  )

  // Initial load on open, 1 Hz polling while open, teardown on close.
  useEffect(() => {
    if (!open) {
      // Closing stops polling AND drops the snapshot so reopening rebuilds
      // from the DB (the freshness source) instead of stale memory.
      setSnapshot(null)
      turnsRef.current = []
      versionWatermarkRef.current.clear()
      return
    }
    void load(true)
    const timer = window.setInterval(() => void load(false), POLL_INTERVAL_MS)
    return () => window.clearInterval(timer)
  }, [open, load])

  async function loadOlder() {
    if (!snapshot?.next_after_ordinal || loadingMore) return
    setLoadingMore(true)
    try {
      const older = await getCollaborationSession({
        parentConversationId,
        sourceTaskId,
        afterOrdinal: snapshot.next_after_ordinal,
        limit: COLLAB_SNAPSHOT_DEFAULT_LIMIT,
      })
      setSnapshot((prev) => {
        if (!prev) return prev
        // Merge through the SAME version-ratcheted turn-ID merge as the poll
        // (reacceptance R10): a history page that also carries the projected
        // active round must not render it twice (and must not let an older
        // version roll a rendered turn back).
        const merged = applyTurns(older.turns, prev.turns)
        turnsRef.current = merged
        return {
          ...prev,
          turns: merged,
          next_after_ordinal: older.next_after_ordinal,
        }
      })
    } catch {
      // Same read-only contract as the poll: keep what we have.
    } finally {
      setLoadingMore(false)
    }
  }

  const session = snapshot?.session ?? null
  const turns = snapshot?.turns ?? []

  // Nothing to show for sources with no collaboration relationship — the
  // section renders nothing rather than an empty box.
  if (!session || turns.length === 0) {
    return null
  }

  return (
    <div className="border-b border-border px-4 py-3">
      <div className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        {t("collabTitle")}
      </div>
      {session.state === "blocked" ? (
        <div className="mb-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-2.5 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          {t("collabBlockedNotice")}
        </div>
      ) : null}
      <ol className="flex flex-col gap-2">
        {turns.map((turn) => (
          <TurnRow
            key={turn.turn_id}
            turn={turn}
            expanded={expanded[turn.turn_id] ?? false}
            onToggle={() =>
              setExpanded((prev) => ({
                ...prev,
                [turn.turn_id]: !prev[turn.turn_id],
              }))
            }
          />
        ))}
      </ol>
      {snapshot?.next_after_ordinal ? (
        <Button
          variant="ghost"
          size="sm"
          className="mt-2 w-full"
          onClick={() => void loadOlder()}
          disabled={loadingMore}
        >
          {loadingMore ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <ChevronUp className="h-3.5 w-3.5" />
          )}
          {t("collabLoadOlder")}
        </Button>
      ) : null}
    </div>
  )
}

function TurnRow({
  turn,
  expanded,
  onToggle,
}: {
  turn: TurnReport
  expanded: boolean
  onToggle: () => void
}) {
  const t = useTranslations("Folder.chat.delegation")
  const tone = stateBadgeTone(turn.state)
  const isActive = ACTIVE_TURN_STATES.includes(turn.state)
  const hasBody = Boolean(turn.result_text || turn.error_message)

  return (
    <li className="rounded-md border border-border bg-muted/30">
      <div className="flex items-center gap-2 px-2.5 py-1.5">
        <span className="text-xs font-medium text-muted-foreground">
          {t("collabTurnOrdinal", { ordinal: turn.ordinal })}
        </span>
        <span className="text-xs text-muted-foreground">·</span>
        <span className="truncate text-xs text-muted-foreground">
          {t("collabInitiatedByParent")}
        </span>
        <Badge
          variant="outline"
          className={`ml-auto shrink-0 text-[10px] ${tone.className}`}
        >
          {isActive ? (
            <Loader2 className="mr-1 h-2.5 w-2.5 animate-spin" />
          ) : null}
          {t(`collabState_${turn.state}`)}
        </Badge>
      </div>
      {/* The original rework requirement — always visible. */}
      <p className="whitespace-pre-wrap px-2.5 pb-1.5 text-xs text-foreground">
        <span className="font-medium text-muted-foreground">
          {t("collabReworkLabel")}:{" "}
        </span>
        {turn.message}
      </p>
      {hasBody ? (
        <>
          <button
            type="button"
            onClick={onToggle}
            className="flex w-full items-center gap-1 border-t border-border/60 px-2.5 py-1 text-[11px] text-muted-foreground hover:text-foreground"
          >
            {expanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronUp className="h-3 w-3" />
            )}
            {expanded ? t("collabHideResult") : t("collabShowResult")}
          </button>
          {expanded ? (
            <div className="border-t border-border/60 px-2.5 py-1.5">
              {turn.result_text ? (
                <p className="whitespace-pre-wrap text-xs text-foreground">
                  {turn.result_text}
                  {turn.text_truncated ? (
                    <span className="ml-1 text-muted-foreground">
                      ({t("collabResultTruncated")})
                    </span>
                  ) : null}
                </p>
              ) : null}
              {turn.error_message ? (
                <p className="text-xs text-amber-600 dark:text-amber-400">
                  {turn.error_code ? `${turn.error_code}: ` : ""}
                  {turn.error_message}
                </p>
              ) : null}
            </div>
          ) : null}
        </>
      ) : null}
      {turn.state === "outcome_unknown" ? (
        <p className="border-t border-border/60 px-2.5 py-1.5 text-[11px] text-amber-600 dark:text-amber-400">
          {t("collabOutcomeUnknownNotice")}
        </p>
      ) : null}
    </li>
  )
}
