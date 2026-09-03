"use client"

import { useCallback, useEffect, useState } from "react"

import { toErrorMessage } from "@/lib/app-error"
import { getDelegationSettings } from "@/lib/api"
import type { AgentDelegationDefaults, AgentType } from "@/lib/types"

export interface DelegationBaselineState {
  /** Per-agent global delegation defaults; `null` while loading. */
  baseline: Partial<Record<AgentType, AgentDelegationDefaults>> | null
  loading: boolean
  /** Human-readable fetch failure; never treated as an empty baseline. */
  error: string | null
  /** Retry the current open's baseline fetch. */
  retry: () => void
}

const CLOSED: DelegationBaselineState = {
  baseline: null,
  loading: false,
  error: null,
  retry: () => {},
}

interface Settled {
  /** Which open this result belongs to; only the CURRENT open's fetch may
   *  land — anything older is stale by definition. */
  epoch: number
  baseline: Partial<Record<AgentType, AgentDelegationDefaults>> | null
  error: string | null
}

/**
 * The global delegation defaults the per-mention popover displays as its
 * baseline. `active` tracks whether the popover is open.
 *
 * Every activation re-fetches AND clears the baseline first: the previous
 * values must never survive a close, or an edit made right after the user
 * changed global settings would be seeded against stale values — and since a
 * per-call value REPLACES the global default wholesale (no merge), a stale
 * baseline silently drops the settings the user just saved. The clear is
 * DERIVED during render from the open epoch (React's "adjust state when a
 * prop changes" pattern), so the moment the popover reopens `loading` is true
 * and `baseline` is null — no effect round-trip, no stale window. The fetch
 * is a cheap local IPC; a superseded response never lands (cancelled flag +
 * epoch check).
 */
export function useDelegationGlobalBaseline(
  active: boolean
): DelegationBaselineState {
  const [settled, setSettled] = useState<Settled | null>(null)
  const [retryToken, setRetryToken] = useState(0)
  // Bumps each time the popover transitions closed → open.
  const [openEpoch, setOpenEpoch] = useState(0)
  const [wasActive, setWasActive] = useState(false)
  if (active && !wasActive) {
    setWasActive(true)
    setOpenEpoch((epoch) => epoch + 1)
  } else if (!active && wasActive) {
    setWasActive(false)
  }

  const retry = useCallback(() => {
    setSettled(null)
    setRetryToken((token) => token + 1)
  }, [])

  useEffect(() => {
    if (!active || openEpoch === 0) return
    let cancelled = false
    getDelegationSettings()
      .then((settings) => {
        if (!cancelled)
          setSettled({
            epoch: openEpoch,
            baseline: settings.agent_defaults ?? {},
            error: null,
          })
      })
      .catch((err: unknown) => {
        // Never turn an unreadable baseline into an empty successful one. The
        // popover emits a whole per-call replacement, so editing against an
        // unknown baseline can silently drop unrelated global pins.
        if (!cancelled) {
          const detail = toErrorMessage(err)
          setSettled({
            epoch: openEpoch,
            baseline: null,
            error: detail.trim() || "Unknown error",
          })
        }
      })
    return () => {
      cancelled = true
    }
  }, [active, openEpoch, retryToken])

  if (!active) return { ...CLOSED, retry }
  const current = settled?.epoch === openEpoch ? settled : null
  return {
    baseline: current?.baseline ?? null,
    loading: current === null,
    error: current?.error ?? null,
    retry,
  }
}
