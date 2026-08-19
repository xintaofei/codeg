/**
 * AIR typed session-failure projection helpers.
 *
 * The wire (`session_failure` events / the snapshot's `session_failures`
 * table) carries UPSERTS ONLY — see `SessionFailureRecord` in `lib/types`.
 * These helpers implement the client half of the contract, shared by the
 * connections reducer (live events + snapshot hydrate) so it matches the
 * backend's `SessionState::apply_event` byte for byte:
 *
 * - monotonic per-id merge: a record is accepted only when its revision is
 *   STRICTLY greater than the stored one (equal = verbatim replay, e.g.
 *   claude re-publishing still-active failures on session/load). Accepted
 *   upserts re-arm `resolved` — id reuse with a bumped revision is how codex
 *   escalates a retry warning into the turn's terminal error, and how a
 *   settled incident legitimately recurs. The ONE place this is looser than
 *   `apply_event` is an equal-revision record carrying `resolved: true`: that
 *   bit is client-inferred rather than adapter-published, so a snapshot must
 *   be able to hand it to a client that missed the events it came from. The
 *   backend needs no counterpart — nothing upstream ever sends IT a
 *   resolution.
 * - inferred resolution: adapters never publish resolve/tombstone, so the
 *   client settles records itself — RETRY INCIDENTS as soon as the turn makes
 *   progress again, any remaining severity-"warning" record at the turn
 *   boundary (mirroring the retry banner's clear-at-turn-end), and EVERYTHING
 *   when the user starts a new prompt (acting past an error acknowledges it;
 *   a still-real failure comes back with a higher revision).
 *
 * Entries are retained after resolution — each doubles as its id's revision
 * watermark, so a delayed stale upsert can never resurrect a settled record.
 * Kept dependency-free for unit testing without the context harness.
 */

import type {
  ContentBlock,
  MessageTurn,
  SessionFailureRecord,
} from "@/lib/types"

/** Merge one incoming upsert; returns the SAME array reference when rejected. */
export function upsertSessionFailure(
  current: SessionFailureRecord[],
  record: SessionFailureRecord
): SessionFailureRecord[] {
  return mergeSessionFailures(current, [record])
}

/**
 * Merge a batch of upserts (snapshot hydrate) into the current table by the
 * monotonic per-id rule. Returns the same array reference when nothing
 * changed, so reducer consumers can cheaply detect no-ops.
 */
export function mergeSessionFailures(
  current: SessionFailureRecord[],
  incoming: SessionFailureRecord[] | null | undefined
): SessionFailureRecord[] {
  if (!incoming || incoming.length === 0) return current
  let next: SessionFailureRecord[] | null = null
  for (const record of incoming) {
    if (!record.id || !(record.revision >= 1)) continue
    const target = next ?? current
    const index = target.findIndex((f) => f.id === record.id)
    const stored = index >= 0 ? target[index] : null
    if (stored && record.revision < stored.revision) continue
    if (stored && record.revision === stored.revision) {
      // Equal revision is a verbatim replay (claude re-publishes still-active
      // failures on session/load) — with ONE exception: `resolved` is inferred
      // per client, so a snapshot that already settled this record has to be
      // able to hand that resolution to a client which missed the progress /
      // turn-end events it was inferred from (a mid-turn reattach, where
      // hydrate is the only channel). Resolution moves false → true only; a
      // genuine recurrence still re-arms through a HIGHER revision, and a
      // replay carrying `false` can never un-settle — which is also what keeps
      // a local `dismissed` alive against a backend snapshot that never saw it.
      if (!record.resolved || stored.resolved) continue
      next ??= [...current]
      const at = next.findIndex((f) => f.id === record.id)
      // Spread the STORED record: same id+revision means same content, and
      // this preserves the client-local `dismissed` bit.
      next[at] = { ...next[at], resolved: true }
      continue
    }
    next ??= [...current]
    const accepted: SessionFailureRecord = {
      ...record,
      resolved: record.resolved ?? false,
    }
    const nextIndex = next.findIndex((f) => f.id === record.id)
    if (nextIndex >= 0) next[nextIndex] = accepted
    else next.push(accepted)
  }
  return next ?? current
}

/**
 * Lifecycle boundary a settle pass runs at:
 *
 * - `"retry_incidents"` — the turn produced content again, so every in-flight
 *   retry incident recovered (codex's own `completeRetryIncidentOnTurnProgress`).
 * - `"warnings"` — a CLEAN turn end; sweeps whatever incidents/notices are left.
 * - `"all"` — the user started a new prompt, acknowledging even terminal errors.
 */
export type SessionFailureSettleScope = "retry_incidents" | "warnings" | "all"

/**
 * A warning that represents an IN-FLIGHT RETRY INCIDENT — the adapter lost the
 * upstream and is reconnecting, so the next byte of turn output proves it
 * recovered.
 *
 * Category "unknown" is deliberately excluded: it is where BOTH adapters route
 * their non-incident informational records (codex config/skill-budget notices,
 * claude `model_refusal_fallback` advisories). Those are not incidents, nothing
 * "recovers" them, and settling them on the next token would flash them away
 * before they can be read — they wait for the turn boundary like before.
 */
function isRetryIncident(f: SessionFailureRecord): boolean {
  return f.severity === "warning" && f.category !== "unknown"
}

/** Whether a progress settle would change anything — cheap per-chunk guard. */
export function hasSettleableRetryIncident(
  failures: SessionFailureRecord[]
): boolean {
  return failures.some((f) => !f.resolved && isRetryIncident(f))
}

/**
 * Settle records in place at a lifecycle boundary (see
 * [`SessionFailureSettleScope`]); errors survive everything but `"all"`, since
 * codex deliberately keeps terminal records active. Returns the same array
 * reference when nothing needed settling.
 */
export function settleSessionFailures(
  failures: SessionFailureRecord[],
  scope: SessionFailureSettleScope
): SessionFailureRecord[] {
  const settles = (f: SessionFailureRecord) => {
    if (f.resolved) return false
    if (scope === "all") return true
    if (scope === "warnings") return f.severity === "warning"
    return isRetryIncident(f)
  }
  if (!failures.some(settles)) return failures
  return failures.map((f) => (settles(f) ? { ...f, resolved: true } : f))
}

/**
 * Resolve records because the user closed their strip (client-local, like
 * `dismissConfigStale`). Takes a LIST because one collapsed warning strip
 * stands for every active warning behind it — closing a bar labelled "+2 more"
 * has to close all three, not peel them off one click at a time.
 *
 * Dismissal is marked distinctly from recovery: `dismissed` records are
 * excluded from the "recovered" line (see [`mostRecentRecoveredWarning`]),
 * because a silenced incident is not a fixed one. The entry stays as its id's
 * revision watermark, so this silences only what was actually on screen — a
 * failure that is still real re-arms via a higher revision on the same id.
 *
 * Applies to ALREADY-RESOLVED records too, not just active ones: the muted
 * "recovered" line is by definition resolved, and closing (or auto-expiring)
 * it still has to mark it `dismissed` so it stops rendering. Gating this on
 * `!resolved` made both of that strip's exits silent no-ops.
 *
 * Returns the same array reference when nothing needed dismissing.
 */
export function dismissSessionFailures(
  failures: SessionFailureRecord[],
  ids: string[]
): SessionFailureRecord[] {
  const targets = new Set(ids)
  const changes = (f: SessionFailureRecord) =>
    targets.has(f.id) && !(f.resolved && f.dismissed)
  if (!failures.some(changes)) return failures
  return failures.map((f) =>
    changes(f) ? { ...f, resolved: true, dismissed: true } : f
  )
}

/** Unresolved records, for the banner's active section. */
export function activeSessionFailures(
  failures: SessionFailureRecord[]
): SessionFailureRecord[] {
  return failures.filter((f) => !f.resolved)
}

/** What the banner actually renders (see [`activeSessionFailureView`]). */
export interface ActiveSessionFailureView {
  /** Terminal records — never collapsed; each needs its own action buttons. */
  errors: SessionFailureRecord[]
  /** The active warning to show, or `null` when there is none. */
  warning: SessionFailureRecord | null
  /** Older active warnings folded behind `warning` (0 when nothing is hidden). */
  hiddenWarnings: number
  /** Every active warning id the collapsed strip stands for, `warning`
   *  included — closing that one bar must close everything it represents. */
  warningIds: string[]
}

/**
 * Project the active records onto the strips to render.
 *
 * Warnings collapse to ONE strip plus a count. They are transient incidents
 * that normally settle on the next chunk, but every extra one costs a
 * permanent row docked under the composer, and a turn that retries N times
 * used to stack N identical strips over the chat (issue #496). Errors are not
 * collapsed: each is terminal and carries its own suggested actions.
 *
 * The strip shows the LAST active warning in table order. That is arrival
 * order for live events, but only a best-effort proxy for recency in general
 * — an in-place upsert keeps its original slot, and a hydrating snapshot
 * arrives in the backend's `BTreeMap` id order. Which of several concurrent
 * incidents shows is cosmetic; the count and the dismiss set cover them all.
 */
export function activeSessionFailureView(
  failures: SessionFailureRecord[]
): ActiveSessionFailureView {
  const active = activeSessionFailures(failures)
  const warnings = active.filter((f) => f.severity === "warning")
  return {
    errors: active.filter((f) => f.severity !== "warning"),
    warning: warnings[warnings.length - 1] ?? null,
    hiddenWarnings: Math.max(0, warnings.length - 1),
    warningIds: warnings.map((f) => f.id),
  }
}

/**
 * The record behind the muted "recovered" line: the most recent warning that
 * settled ON ITS OWN. User-dismissed records are excluded — closing a strip
 * has to REMOVE it, not swap it for a line claiming the incident recovered
 * (which would also be false whenever the connection is still down).
 */
export function mostRecentRecoveredWarning(
  failures: SessionFailureRecord[]
): SessionFailureRecord | null {
  for (let i = failures.length - 1; i >= 0; i--) {
    const f = failures[i]
    if (f.resolved && !f.dismissed && f.severity === "warning") return f
  }
  return null
}

/** Resolved records, for the banner's collapsed "recovered" rows. */
export function resolvedSessionFailures(
  failures: SessionFailureRecord[]
): SessionFailureRecord[] {
  return failures.filter((f) => f.resolved)
}

/**
 * Text of the most recent USER turn — what the failure banner's "retry"
 * action re-submits. Joins the turn's text blocks; image-only or empty user
 * turns are skipped (nothing meaningful to resend). Callers should feed the
 * runtime TIMELINE turns first and fall back to the persisted detail: after a
 * failed turn the prompt may exist only as an optimistic/promoted runtime
 * turn (the persisted parse can lag or miss it), and reading only the detail
 * made the retry click a silent no-op (2026-08-16 field report).
 */
export function lastUserPromptText(
  turns: MessageTurn[] | undefined
): string | null {
  if (!turns) return null
  for (let i = turns.length - 1; i >= 0; i--) {
    const turn = turns[i]
    if (turn.role !== "user") continue
    const text = turn.blocks
      .filter(
        (b): b is Extract<ContentBlock, { type: "text" }> => b.type === "text"
      )
      .map((b) => b.text)
      .join("\n")
      .trim()
    if (text) return text
  }
  return null
}

/** The AIR action vocabulary the banner knows how to wire. */
export const KNOWN_SESSION_FAILURE_ACTIONS = [
  "retry",
  "login",
  "new_session",
] as const

export type SessionFailureAction =
  (typeof KNOWN_SESSION_FAILURE_ACTIONS)[number]

/** The record's suggested actions filtered to the renderable vocabulary. */
export function knownSessionFailureActions(
  record: SessionFailureRecord
): SessionFailureAction[] {
  const actions = record.actions ?? []
  return KNOWN_SESSION_FAILURE_ACTIONS.filter((a) => actions.includes(a))
}
