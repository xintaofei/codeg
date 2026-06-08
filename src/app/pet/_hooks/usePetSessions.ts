"use client"

import { useEffect, useState } from "react"
import { listActivePetSessions } from "@/lib/pet/api"
import { getTransport } from "@/lib/transport"
import type { PetSessionsPayload } from "@/lib/pet/types"

const PET_SESSIONS_EVENT = "pet://sessions"

const EMPTY: PetSessionsPayload = {
  runningCount: 0,
  waitingCount: 0,
  errorCount: 0,
  sessions: [],
}

// Retry backoff for the freshly-opened-window case. The panel window is
// recreated on every open and makes its first Tauri IPC calls during window
// spin-up, where the very first call can reject or stall once before the bridge
// is ready. Capped so a genuinely-unavailable backend is polled at a sane rate;
// retries stop on success, on the first live event, or on unmount.
const RETRY_BASE_MS = 150
const RETRY_MAX_MS = 2000
const backoffMs = (attempt: number) =>
  Math.min(RETRY_BASE_MS * 2 ** attempt, RETRY_MAX_MS)

/**
 * Subscribe to the backend-owned `pet://sessions` stream and recover the
 * current snapshot. Returns the full payload so both the sprite badge (counts
 * only) and the panel (full list) share one subscription path.
 *
 * Resilient acquisition: the snapshot fetch and the live subscription are
 * INDEPENDENT, and each retries on failure. This matters because the panel is a
 * fresh window on every open — if its first `subscribe()` IPC rejects or stalls
 * even once, the snapshot must still run (it used to be nested after the
 * subscription, so one hiccup left the panel permanently empty), and a transient
 * failure must self-heal rather than give up. The badge's long-lived pet window
 * benefits from the same resilience.
 *
 * A live event still wins over the (possibly stale) initial snapshot: once one
 * arrives, snapshot retries stop and never clobber it.
 */
export function usePetSessions(): PetSessionsPayload {
  const [payload, setPayload] = useState<PetSessionsPayload>(EMPTY)

  useEffect(() => {
    let cancelled = false
    let liveEventSeen = false
    let snapshotLoaded = false
    let unlisten: (() => void) | null = null
    let warnedSnapshot = false
    let warnedSubscribe = false
    const timers = new Set<ReturnType<typeof setTimeout>>()

    const retry = (fn: () => void, attempt: number) => {
      if (cancelled) return
      const id = setTimeout(() => {
        timers.delete(id)
        fn()
      }, backoffMs(attempt))
      timers.add(id)
    }

    const applyLive = (next: PetSessionsPayload | null) => {
      if (cancelled || !next) return
      liveEventSeen = true
      setPayload(next)
    }

    // Pull the current snapshot, independent of the subscription. A successful
    // (even empty) payload is the answer and does not retry; only a rejection
    // retries, until a live event arrives or we unmount.
    const fetchSnapshot = (attempt = 0) => {
      if (cancelled || liveEventSeen) return
      void listActivePetSessions()
        .then((snapshot) => {
          // A reachable backend answered — latch so a pending retry from an
          // earlier rejection (on the other chain) doesn't keep firing. The
          // explicit post-subscribe catch-up fetch is still allowed to run.
          snapshotLoaded = true
          if (!cancelled && !liveEventSeen) setPayload(snapshot)
        })
        .catch((err) => {
          if (cancelled || liveEventSeen || snapshotLoaded) return
          if (!warnedSnapshot) {
            warnedSnapshot = true
            console.warn("[Pet] sessions snapshot failed (retrying):", err)
          }
          retry(() => fetchSnapshot(attempt + 1), attempt)
        })
    }

    // Arm the live subscription, independent of the snapshot. On each successful
    // (re)subscribe, re-pull the snapshot to close the gap between snapshot time
    // and the listener going live.
    const subscribe = (attempt = 0) => {
      if (cancelled) return
      void getTransport()
        .subscribe<PetSessionsPayload>(PET_SESSIONS_EVENT, (raw) =>
          applyLive(normalize(raw))
        )
        .then((off) => {
          if (cancelled) {
            off()
            return
          }
          unlisten = off
          fetchSnapshot()
        })
        .catch((err) => {
          if (cancelled) return
          if (!warnedSubscribe) {
            warnedSubscribe = true
            console.warn("[Pet] sessions subscription failed (retrying):", err)
          }
          retry(() => subscribe(attempt + 1), attempt)
        })
    }

    // Kick both off immediately and independently so the panel populates from
    // the snapshot without waiting on (or being blocked by) the subscription.
    fetchSnapshot()
    subscribe()

    return () => {
      cancelled = true
      for (const id of timers) clearTimeout(id)
      timers.clear()
      if (unlisten) unlisten()
    }
  }, [])

  return payload
}

/** Unwrap a possible `{ payload: {...} }` transport envelope and validate the
 *  shape so callers can't be handed a non-payload value. */
function normalize(raw: unknown): PetSessionsPayload | null {
  let obj = raw
  if (
    obj &&
    typeof obj === "object" &&
    "payload" in obj &&
    !("sessions" in obj)
  ) {
    obj = (obj as { payload: unknown }).payload
  }
  if (
    obj &&
    typeof obj === "object" &&
    Array.isArray((obj as PetSessionsPayload).sessions)
  ) {
    return obj as PetSessionsPayload
  }
  return null
}
