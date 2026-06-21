"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { automationList } from "@/lib/api"
import { subscribe } from "@/lib/platform"
import type { Automation } from "@/lib/types"

const AUTOMATION_CHANGED_EVENT = "automation://changed"

interface AutomationsViewContextValue {
  automations: Automation[]
  /** Sum of unseen failed runs — drives the sidebar badge. */
  unseenFailures: number
  refetch: () => Promise<void>
}

const AutomationsViewContext =
  createContext<AutomationsViewContextValue | null>(null)

/**
 * Data layer for the Automations feature: the automation list + a realtime
 * subscription, kept always-mounted so the sidebar's failure badge stays live.
 * It is the single source for both the badge and the Automations route page.
 *
 * Navigation lives in WorkbenchRouteProvider (the sidebar sets the route, the
 * content region swaps), so this provider holds data only — no open-state, no
 * rendered overlay.
 */
export function useAutomationsView() {
  const ctx = useContext(AutomationsViewContext)
  if (!ctx) {
    throw new Error(
      "useAutomationsView must be used within AutomationsViewProvider"
    )
  }
  return ctx
}

export function AutomationsViewProvider({ children }: { children: ReactNode }) {
  const [automations, setAutomations] = useState<Automation[]>([])
  const reqRef = useRef(0)

  const refetch = useCallback(async () => {
    const id = ++reqRef.current
    try {
      const list = await automationList()
      // Drop stale responses; keep the previous list on transient error rather
      // than blanking the view (matches CONVERSATION_CHANGED_EVENT consumers).
      if (id === reqRef.current) setAutomations(list)
    } catch {
      // ignore — a later event/refetch recovers
    }
  }, [])

  useEffect(() => {
    // Initial fetch + subscribe for backend-pushed updates. `refetch` only
    // setStates after an await / inside the subscribe callback (the canonical
    // "subscribe to an external system, setState in the callback" effect), and
    // its deps are stable, so this can't cascade. Same block-disable idiom as
    // workspace-context.tsx.
    /* eslint-disable react-hooks/set-state-in-effect */
    void refetch()
    let unsub: (() => void) | undefined
    let cancelled = false
    void subscribe(AUTOMATION_CHANGED_EVENT, () => {
      void refetch()
    }).then((u: () => void) => {
      if (cancelled) u()
      else unsub = u
    })
    return () => {
      cancelled = true
      unsub?.()
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [refetch])

  const unseenFailures = useMemo(
    () => automations.reduce((sum, a) => sum + (a.unseen_failures || 0), 0),
    [automations]
  )

  const value = useMemo<AutomationsViewContextValue>(
    () => ({ automations, unseenFailures, refetch }),
    [automations, unseenFailures, refetch]
  )

  return (
    <AutomationsViewContext.Provider value={value}>
      {children}
    </AutomationsViewContext.Provider>
  )
}
