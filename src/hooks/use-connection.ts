"use client"

import { useCallback, useMemo, useRef, useSyncExternalStore } from "react"
import {
  useAcpActions,
  useConnectionStore,
  getCachedSelectors,
  type ClaudeApiRetryState,
  type ConnectionState,
  type LiveMessage,
  type PendingPermission,
  type PendingQuestion,
} from "@/contexts/acp-connections-context"
import type {
  AgentType,
  AvailableCommandInfo,
  ConnectionStatus,
  PromptCapabilitiesInfo,
  SessionConfigOptionInfo,
  SessionModeStateInfo,
  PromptInputBlock,
} from "@/lib/types"

const DEFAULT_PROMPT_CAPABILITIES: PromptCapabilitiesInfo = {
  image: false,
  audio: false,
  embedded_context: false,
}

export interface UseConnectionReturn {
  connectionId: string | null
  status: ConnectionStatus | null
  promptCapabilities: PromptCapabilitiesInfo
  supportsFork: boolean
  selectorsReady: boolean
  hasCachedSelectors: boolean
  sessionId: string | null
  modes: SessionModeStateInfo | null
  configOptions: SessionConfigOptionInfo[] | null
  availableCommands: AvailableCommandInfo[] | null
  liveMessage: LiveMessage | null
  pendingPermission: PendingPermission | null
  pendingQuestion: PendingQuestion | null
  claudeApiRetry: ClaudeApiRetryState | null
  /** True while the agent is compacting context history mid-turn. */
  compacting: boolean
  error: string | null
  connect: (
    agentType: AgentType,
    workingDir?: string,
    sessionId?: string
  ) => Promise<void>
  disconnect: () => Promise<void>
  sendPrompt: (blocks: PromptInputBlock[]) => Promise<void>
  setMode: (modeId: string) => Promise<void>
  setConfigOption: (configId: string, valueId: string) => Promise<void>
  cancel: () => Promise<void>
  respondPermission: (requestId: string, optionId: string) => Promise<void>
}

function derive(conn: ConnectionState | undefined) {
  if (!conn) return null
  return conn
}

/**
 * Subscribe to a single derived slice of a connection. Returns a stable
 * reference whenever the slice value compares equal (Object.is) to the
 * previous one, so consumers re-render only when their slice actually
 * changes — instead of on every connection-state mutation.
 *
 * This is the field-scoped primitive that backs the granular hooks below.
 * Use it directly when you need a custom slice not covered by them.
 */
function useConnectionSlice<T>(
  contextKey: string,
  selector: (conn: ConnectionState | undefined) => T
): T {
  const store = useConnectionStore()
  const subscribe = useCallback(
    (cb: () => void) => store.subscribeKey(contextKey, cb),
    [store, contextKey]
  )

  // Cache the last selected value to keep the snapshot identity stable
  // when the underlying connection mutates but the slice is equal.
  const lastValueRef = useRef<{ value: T; computed: boolean }>({
    value: undefined as unknown as T,
    computed: false,
  })

  const getSnapshot = useCallback(() => {
    const next = selector(store.getConnection(contextKey))
    if (
      lastValueRef.current.computed &&
      Object.is(lastValueRef.current.value, next)
    ) {
      return lastValueRef.current.value
    }
    lastValueRef.current = { value: next, computed: true }
    return next
  }, [store, contextKey, selector])

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

const selectStatus = (c: ConnectionState | undefined) => c?.status ?? null
const selectLiveMessage = (c: ConnectionState | undefined) =>
  c?.liveMessage ?? null
const selectPendingPermission = (c: ConnectionState | undefined) =>
  c?.pendingPermission ?? null
const selectPendingQuestion = (c: ConnectionState | undefined) =>
  c?.pendingQuestion ?? null
const selectError = (c: ConnectionState | undefined) => c?.error ?? null
const selectCompacting = (c: ConnectionState | undefined) =>
  c?.compacting ?? false
const selectClaudeApiRetry = (c: ConnectionState | undefined) =>
  c?.claudeApiRetry ?? null
const selectConnectionId = (c: ConnectionState | undefined) =>
  c?.connectionId ?? null
const selectSessionId = (c: ConnectionState | undefined) => c?.sessionId ?? null

/** Subscribe only to connection status. */
export function useConnectionStatus(contextKey: string) {
  return useConnectionSlice(contextKey, selectStatus)
}

/** Subscribe only to the live (streaming) message. High-frequency: prefer
 * this over `useConnection` in components that render the streaming bubble. */
export function useConnectionLiveMessage(contextKey: string) {
  return useConnectionSlice(contextKey, selectLiveMessage)
}

/** Subscribe only to pending permission/question prompts. */
export function useConnectionPendingPermission(contextKey: string) {
  return useConnectionSlice(contextKey, selectPendingPermission)
}
export function useConnectionPendingQuestion(contextKey: string) {
  return useConnectionSlice(contextKey, selectPendingQuestion)
}

/** Subscribe only to error / retry / compacting indicators. */
export function useConnectionError(contextKey: string) {
  return useConnectionSlice(contextKey, selectError)
}
export function useConnectionCompacting(contextKey: string) {
  return useConnectionSlice(contextKey, selectCompacting)
}
export function useConnectionClaudeApiRetry(contextKey: string) {
  return useConnectionSlice(contextKey, selectClaudeApiRetry)
}

/** Subscribe only to identifiers. */
export function useConnectionId(contextKey: string) {
  return useConnectionSlice(contextKey, selectConnectionId)
}
export function useConnectionSessionId(contextKey: string) {
  return useConnectionSlice(contextKey, selectSessionId)
}

export function useConnection(contextKey: string): UseConnectionReturn {
  const store = useConnectionStore()
  const actions = useAcpActions()

  const subscribe = useCallback(
    (cb: () => void) => store.subscribeKey(contextKey, cb),
    [store, contextKey]
  )
  const getSnapshot = useCallback(
    () => derive(store.getConnection(contextKey)),
    [store, contextKey]
  )
  const connection = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)

  const connectionId = connection?.connectionId ?? null
  const status = connection?.status ?? null
  const promptCapabilities =
    connection?.promptCapabilities ?? DEFAULT_PROMPT_CAPABILITIES
  const supportsFork = connection?.supportsFork ?? false
  const selectorsReady = connection?.selectorsReady ?? false
  const sessionId = connection?.sessionId ?? null
  const cached = connection?.agentType
    ? getCachedSelectors(connection.agentType)
    : null
  const hasCachedSelectors = cached !== null
  const modes = connection?.modes ?? cached?.modes ?? null
  const configOptions =
    connection?.configOptions ?? cached?.configOptions ?? null
  const availableCommands = connection?.availableCommands ?? null
  const liveMessage = connection?.liveMessage ?? null
  const pendingPermission = connection?.pendingPermission ?? null
  const pendingQuestion = connection?.pendingQuestion ?? null
  const claudeApiRetry = connection?.claudeApiRetry ?? null
  const compacting = connection?.compacting ?? false
  const error = connection?.error ?? null

  const connect = useCallback(
    (agentType: AgentType, workingDir?: string, sessionId?: string) =>
      actions.connect(contextKey, agentType, workingDir, sessionId),
    [actions, contextKey]
  )

  const disconnect = useCallback(
    () => actions.disconnect(contextKey),
    [actions, contextKey]
  )

  const sendPrompt = useCallback(
    (blocks: PromptInputBlock[]) => actions.sendPrompt(contextKey, blocks),
    [actions, contextKey]
  )

  const setMode = useCallback(
    (modeId: string) => actions.setMode(contextKey, modeId),
    [actions, contextKey]
  )

  const setConfigOption = useCallback(
    (configId: string, valueId: string) =>
      actions.setConfigOption(contextKey, configId, valueId),
    [actions, contextKey]
  )

  const cancel = useCallback(
    () => actions.cancel(contextKey),
    [actions, contextKey]
  )

  const respondPermission = useCallback(
    (requestId: string, optionId: string) =>
      actions.respondPermission(contextKey, requestId, optionId),
    [actions, contextKey]
  )

  return useMemo(
    () => ({
      connectionId,
      status,
      promptCapabilities,
      supportsFork,
      selectorsReady,
      hasCachedSelectors,
      sessionId,
      modes,
      configOptions,
      availableCommands,
      liveMessage,
      pendingPermission,
      pendingQuestion,
      claudeApiRetry,
      compacting,
      error,
      connect,
      disconnect,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      respondPermission,
    }),
    [
      connectionId,
      status,
      promptCapabilities,
      supportsFork,
      selectorsReady,
      hasCachedSelectors,
      sessionId,
      modes,
      configOptions,
      availableCommands,
      liveMessage,
      pendingPermission,
      pendingQuestion,
      claudeApiRetry,
      compacting,
      error,
      connect,
      disconnect,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      respondPermission,
    ]
  )
}
