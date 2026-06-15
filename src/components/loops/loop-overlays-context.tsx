"use client"

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react"

import type { AgentType } from "@/lib/types"
import { IterationDialog } from "@/components/loops/iteration-dialog"

interface OpenIterationArgs {
  conversationId: number
  agentType?: AgentType | null
}

interface LoopOverlaysValue {
  openIteration: (args: OpenIterationArgs) => void
}

const Ctx = createContext<LoopOverlaysValue | null>(null)

/** Holds the single IterationDialog instance for the whole loop UI. Any surface
 *  opens it by dispatch — no more duplicate per-surface mounts. */
export function LoopOverlaysProvider({ children }: { children: ReactNode }) {
  const [iteration, setIteration] = useState<OpenIterationArgs | null>(null)
  const openIteration = useCallback(
    (args: OpenIterationArgs) => setIteration(args),
    []
  )
  const value = useMemo<LoopOverlaysValue>(
    () => ({ openIteration }),
    [openIteration]
  )
  return (
    <Ctx.Provider value={value}>
      {children}
      <IterationDialog
        open={iteration != null}
        onOpenChange={(o) => {
          if (!o) setIteration(null)
        }}
        conversationId={iteration?.conversationId ?? 0}
        agentType={iteration?.agentType ?? null}
      />
    </Ctx.Provider>
  )
}

export function useLoopOverlays(): LoopOverlaysValue {
  const ctx = useContext(Ctx)
  if (!ctx)
    throw new Error("useLoopOverlays must be used within LoopOverlaysProvider")
  return ctx
}
