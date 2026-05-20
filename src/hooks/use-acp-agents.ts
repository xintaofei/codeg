"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { acpListAgents } from "@/lib/api"
import { onTransportReconnect, subscribe } from "@/lib/platform"
import type { UnsubscribeFn } from "@/lib/transport/types"
import type { AcpAgentInfo } from "@/lib/types"

const ACP_AGENTS_UPDATED_EVENT = "app://acp-agents-updated"

export interface UseAcpAgentsResult {
  /** Agents sorted by `sort_order` then `name`. No filtering applied. */
  agents: AcpAgentInfo[]
  /**
   * Whether at least one successful reload has completed this session.
   * Monotonic: once true, stays true. Consumers use this to decide
   * when "best-guess" defaults can be replaced with the real list.
   */
  fresh: boolean
  /** Manual refresh — useful for explicit user-driven retries. */
  refresh: () => Promise<void>
}

/**
 * Subscribe to the ACP agent registry. Centralizes the
 * "fetch + window focus + `app://acp-agents-updated` event" pattern that
 * was previously copy-pasted across AgentSelector, SidebarConversationList,
 * and TabProvider — all three were also directly importing the Tauri
 * event API, bypassing the platform layer. This hook uses the platform-
 * agnostic `subscribe()` so the event path works in both desktop and web
 * modes.
 *
 * Behavior on error: the agents list is **not cleared** — keeping the
 * last good cache prevents a transient API blip from silently degrading
 * downstream defaults.
 */
export function useAcpAgents(): UseAcpAgentsResult {
  const [agents, setAgents] = useState<AcpAgentInfo[]>([])
  const [fresh, setFresh] = useState(false)

  const cancelledRef = useRef(false)
  // Tracks the most recently issued reload. `latestSuccessIdRef` tracks
  // the most recent reload that actually wrote state. Splitting these
  // matters when reloads race: if #1 starts then #2 starts then #1
  // succeeds, the old single-counter scheme would discard #1's valid
  // data because `requestId(1) !== latestRequestIdRef(2)`. If #2 then
  // failed (or was still pending), `fresh` would stay false forever
  // despite #1 having returned a usable list. The success-id counter
  // only bumps on actual writes, so #1's success can latch `fresh`,
  // and a later #2 success can still overwrite #1's data (monotonic).
  const latestRequestIdRef = useRef(0)
  const latestSuccessIdRef = useRef(0)

  const reload = useCallback(async () => {
    const requestId = latestRequestIdRef.current + 1
    latestRequestIdRef.current = requestId
    try {
      const list = await acpListAgents()
      if (cancelledRef.current) return
      // Only bail if a strictly later success has already committed
      // state — older successes are still useful when newer requests
      // are pending or failed.
      if (requestId <= latestSuccessIdRef.current) return
      latestSuccessIdRef.current = requestId
      const sorted = [...list].sort(
        (a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)
      )
      setAgents(sorted)
      setFresh(true)
    } catch {
      // Keep the previous list — clearing on transient failure would
      // silently regress downstream defaults to AGENT_DISPLAY_ORDER[0].
    }
  }, [])

  useEffect(() => {
    cancelledRef.current = false

    // Defer initial reload to the next microtask so the set-state-in-effect
    // lint rule doesn't flag `void reload()` — the setState calls inside
    // reload all fire post-await anyway, but the analyzer can't see through
    // useCallback boundaries to verify that. Microtask deferral has no
    // user-visible cost (still resolves before paint).
    queueMicrotask(() => {
      void reload()
    })

    const onFocus = () => {
      void reload()
    }
    window.addEventListener("focus", onFocus)

    let unsubscribe: UnsubscribeFn | null = null
    let unsubscribed = false
    void subscribe<unknown>(ACP_AGENTS_UPDATED_EVENT, () => {
      void reload()
    })
      .then((dispose) => {
        if (cancelledRef.current || unsubscribed) {
          dispose()
          return
        }
        unsubscribe = dispose
      })
      .catch(() => {
        // Transport doesn't support subscribe (shouldn't happen) — fall
        // back to mount + focus triggers.
      })

    // Web/remote transports lose events emitted during a WS disconnect
    // window (the broadcaster drops them while `receiver_count == 0`).
    // Re-fetching on reconnect is the recovery path; no-op on Tauri IPC.
    const offReconnect = onTransportReconnect(() => {
      void reload()
    })

    return () => {
      cancelledRef.current = true
      window.removeEventListener("focus", onFocus)
      unsubscribed = true
      if (unsubscribe) {
        try {
          unsubscribe()
        } catch {
          // Ignore — disposing twice or transport gone is harmless.
        }
      }
      if (offReconnect) {
        try {
          offReconnect()
        } catch {
          // Ignore.
        }
      }
    }
  }, [reload])

  return { agents, fresh, refresh: reload }
}
