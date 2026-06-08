"use client"

import { useCallback, useMemo, useSyncExternalStore } from "react"
import {
  useAcpActions,
  useConnectionStore,
  getCachedSelectors,
  type ClaudeApiRetryState,
  type ConnectionState,
  type LiveMessage,
  type PendingPermission,
  type PendingUserMessage,
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
  /**
   * True when this context attached to a connection another client owns
   * (cross-client viewing). Viewers detach but never `acpDisconnect`, so the
   * unmount cleanup must tear them down even mid-turn (the owner's agent is
   * unaffected) — otherwise the attach subscription leaks past tab close.
   */
  isViewer: boolean
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
  pendingUserMessage: PendingUserMessage | null
  pendingQuestion: PendingQuestion | null
  claudeApiRetry: ClaudeApiRetryState | null
  error: string | null
  loadError: string | null
  connect: (
    agentType: AgentType,
    workingDir?: string,
    sessionId?: string,
    conversationId?: number
  ) => Promise<void>
  disconnect: () => Promise<void>
  sendPrompt: (
    blocks: PromptInputBlock[],
    opts?: {
      folderId?: number | null
      conversationId?: number | null
      clientMessageId?: string | null
    }
  ) => Promise<void>
  setMode: (modeId: string) => Promise<void>
  setConfigOption: (configId: string, valueId: string) => Promise<void>
  cancel: () => Promise<void>
  respondPermission: (requestId: string, optionId: string) => Promise<void>
}

function derive(conn: ConnectionState | undefined) {
  if (!conn) return null
  return conn
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
  const isViewer = connection?.isViewer ?? false
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
  const pendingUserMessage = connection?.pendingUserMessage ?? null
  const pendingQuestion = connection?.pendingQuestion ?? null
  const claudeApiRetry = connection?.claudeApiRetry ?? null
  const error = connection?.error ?? null
  const loadError = connection?.loadError ?? null

  const connect = useCallback(
    (
      agentType: AgentType,
      workingDir?: string,
      sessionId?: string,
      conversationId?: number
    ) =>
      actions.connect(
        contextKey,
        agentType,
        workingDir,
        sessionId,
        conversationId
      ),
    [actions, contextKey]
  )

  const disconnect = useCallback(
    () => actions.disconnect(contextKey),
    [actions, contextKey]
  )

  const sendPrompt = useCallback(
    (
      blocks: PromptInputBlock[],
      opts?: {
        folderId?: number | null
        conversationId?: number | null
        clientMessageId?: string | null
      }
    ) => actions.sendPrompt(contextKey, blocks, opts),
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
      isViewer,
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
      pendingUserMessage,
      pendingQuestion,
      claudeApiRetry,
      error,
      loadError,
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
      isViewer,
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
      pendingUserMessage,
      pendingQuestion,
      claudeApiRetry,
      error,
      loadError,
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
