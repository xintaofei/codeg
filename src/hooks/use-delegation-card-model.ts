"use client"

/**
 * Resolves a unified "delegation card model" — agent type, task, status,
 * child ids — from a `delegate_to_agent` tool call, in priority order:
 *   live `DelegationContext` binding → persisted `meta["codeg.delegation"]`
 *   → parsed tool input/output. The same model drives both the inline
 *   `DelegatedSubThread` card and the top-right `SubAgentOverlay`, so the two
 *   never disagree on what a sub-agent is doing.
 *
 * Pure parsing lives in `@/lib/delegation-card`; this hook adds the two
 * React-state reads it can't do on its own: the live binding
 * (`useDelegatedSubSession`) and the child connection's pending-permission
 * status (so the card can badge "waiting").
 */

import { useCallback, useMemo, useSyncExternalStore } from "react"

import { type AgentType } from "@/lib/types"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import {
  useConnectionStore,
  type ConnectionState,
} from "@/contexts/acp-connections-context"
import { useDelegatedSubSession } from "@/hooks/use-delegated-sub-session"
import {
  parseDelegateTaskId,
  parseDelegationMeta,
  parseInput,
  parseToolOutput,
  resolveDelegationStatus,
  type DelegationCardStatus,
  type ParsedToolOutput,
} from "@/lib/delegation-card"

/** The raw inputs a `delegate_to_agent` tool call carries — the props
 *  `DelegatedSubThread` already receives, and the shape `SubAgentOverlay`
 *  extracts from the last assistant turn's tool-call parts. */
export interface DelegationCardSource {
  parentToolUseId: string
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
  meta?: Record<string, unknown> | null
}

export interface DelegationCardModel {
  agentType: AgentType | null
  task: string | null
  taskId: string | null
  status: DelegationCardStatus
  errorCode: string | undefined
  childConversationId: number | null
  childConnectionId: string | null
  /** False when there's no live binding and the input parsed to neither an
   *  agent type nor a task — nothing useful to draw. Callers render null. */
  hasModel: boolean
}

/**
 * Subscribe to the child connection's `ConnectionState` (live message,
 * pending permission, etc.) from the shared connections store. Returns
 * `undefined` while no synthetic entry exists yet. Re-renders on every state
 * change via `useSyncExternalStore`.
 */
function useDelegationChildLive(
  childConnectionId: string | null
): ConnectionState | undefined {
  const store = useConnectionStore()
  const subscribe = useCallback(
    (cb: () => void) => {
      if (!childConnectionId) return () => {}
      return store.subscribeKey(childConnectionId, cb)
    },
    [store, childConnectionId]
  )
  const getSnapshot = useCallback(
    () =>
      childConnectionId ? store.getConnection(childConnectionId) : undefined,
    [store, childConnectionId]
  )
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

export function useDelegationCardModel(
  source: DelegationCardSource
): DelegationCardModel {
  const { parentToolUseId, input, output, errorText, state, meta } = source

  const parsed = useMemo(() => parseInput(input), [input])
  const parsedMeta = useMemo(() => parseDelegationMeta(meta), [meta])
  const taskId = useMemo(
    () => parseDelegateTaskId(output, errorText),
    [output, errorText]
  )

  // `enabled: false` — the model never fetches the child's persisted detail; it
  // only needs the live `binding` (agent type, status, child ids). The child's
  // output is viewed via "查看会话" (SubAgentSessionDialog).
  const { binding } = useDelegatedSubSession(parentToolUseId, {
    enabled: false,
  })

  // Parse the parent `delegate_to_agent` tool output once. Under async this is
  // a running *ack* (kind:"ack") while the child runs; a terminal kind:"outcome"
  // only for a fast-complete or a legacy synchronous result. Used purely to
  // derive the status badge and the child id for synthetic-id cards.
  const toolOutput = useMemo<ParsedToolOutput | null>(() => {
    if (errorText) {
      const parsedErr = parseToolOutput(errorText, true)
      if (parsedErr) return parsedErr
    }
    return parseToolOutput(output)
  }, [output, errorText])

  // Resolution order: live binding → persisted snapshot meta → the broker's
  // ack output (the synthetic-id path that emits no binding/meta).
  const childConnectionId =
    binding?.childConnectionId ?? parsedMeta?.childConnectionId ?? null
  const childConversationId =
    binding?.childConversationId ??
    parsedMeta?.childConversationId ??
    toolOutput?.childConversationId ??
    null

  const childLive = useDelegationChildLive(childConnectionId)
  const childAwaitingPermission = childLive?.pendingPermission != null

  const agentType: AgentType | null = binding?.agentType ?? parsed.agentType
  const status = resolveDelegationStatus({
    binding,
    parsedMeta,
    toolOutput,
    state,
    errorText,
    childAwaitingPermission,
  })
  const errorCode = binding?.errorCode ?? parsedMeta?.errorCode ?? undefined

  return {
    agentType,
    task: parsed.task,
    taskId,
    status,
    errorCode,
    childConversationId,
    childConnectionId,
    hasModel: Boolean(binding || parsed.agentType || parsed.task),
  }
}
