import { useCallback, useEffect, useRef, useState } from "react"
import { subscribe } from "@/lib/platform"
import { appendInstallLogLine } from "@/lib/install-stream"
import type { AgentInstallEvent, AgentInstallEventKind } from "@/lib/types"

const AGENT_INSTALL_EVENT = "app://agent-install"

export type AgentInstallStatus = "idle" | "running" | "success" | "failed"

interface AgentInstallStreamState {
  status: AgentInstallStatus
  logs: string[]
  error: string | null
}

export function useAgentInstallStream() {
  const [state, setState] = useState<AgentInstallStreamState>({
    status: "idle",
    logs: [],
    error: null,
  })
  const unsubRef = useRef<(() => void) | null>(null)
  // Flipped by reset()/unmount. Guards the gap between awaiting subscribe() and
  // storing its unsubscribe fn: if the panel tore down meanwhile, we unsubscribe
  // immediately instead of leaking the listener.
  const cancelledRef = useRef(false)

  const start = useCallback(async (taskId: string) => {
    cancelledRef.current = false
    setState({ status: "running", logs: [], error: null })

    unsubRef.current?.()

    const unsub = await subscribe<AgentInstallEvent>(
      AGENT_INSTALL_EVENT,
      (event) => {
        if (event.task_id !== taskId) return

        switch (event.kind as AgentInstallEventKind) {
          case "started":
            setState((prev) => ({ ...prev, status: "running" }))
            break
          case "log":
            setState((prev) => ({
              ...prev,
              logs: appendInstallLogLine(prev.logs, event.payload),
            }))
            break
          case "completed":
            setState((prev) => ({
              ...prev,
              status: "success",
              logs: appendInstallLogLine(prev.logs, event.payload),
            }))
            unsubRef.current?.()
            break
          case "failed":
            setState((prev) => ({
              ...prev,
              status: "failed",
              error: event.payload,
              logs: appendInstallLogLine(prev.logs, `ERROR: ${event.payload}`),
            }))
            unsubRef.current?.()
            break
        }
      }
    )

    if (cancelledRef.current) {
      // reset()/unmount ran while subscribe() was resolving — don't leak.
      unsub()
      return
    }
    unsubRef.current = unsub
  }, [])

  const reset = useCallback(() => {
    cancelledRef.current = true
    unsubRef.current?.()
    unsubRef.current = null
    setState({ status: "idle", logs: [], error: null })
  }, [])

  // Unsubscribe on unmount: a panel closed mid-install must not leak the
  // global event subscription (or setState after unmount).
  useEffect(() => {
    return () => {
      cancelledRef.current = true
      unsubRef.current?.()
      unsubRef.current = null
    }
  }, [])

  return { ...state, start, reset }
}
