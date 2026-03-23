"use client"

import {
  memo,
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { Plus, RefreshCw, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { disposeTauriListener } from "@/lib/tauri-listener"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useFolderContext } from "@/contexts/folder-context"
import { useTabContext } from "@/contexts/tab-context"
import { useSessionStats } from "@/contexts/session-stats-context"
import { cn } from "@/lib/utils"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { useMessageQueue, type QueuedMessage } from "@/hooks/use-message-queue"
import { MessageListView } from "@/components/message/message-list-view"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { AgentSelector } from "@/components/chat/agent-selector"
import { ChatInput } from "@/components/chat/chat-input"
import {
  acpFork,
  createConversation,
  openSettingsWindow,
  updateConversationExternalId,
  updateConversationStatus,
  updateConversationTitle,
} from "@/lib/tauri"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  extractUserImagesFromDraft,
  extractUserResourcesFromDraft,
  getPromptDraftDisplayText,
} from "@/lib/prompt-draft"
import type {
  AcpEvent,
  AgentType,
  ContentBlock,
  MessageTurn,
  PromptDraft,
} from "@/lib/types"
import {
  buildConversationDraftStorageKey,
  buildNewConversationDraftStorageKey,
  moveMessageInputDraft,
} from "@/lib/message-input-draft"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"

interface ConversationTabViewProps {
  tabId: string
  conversationId: number | null
  agentType: AgentType
  workingDir?: string
  isActive: boolean
  reloadSignal: number
}

function buildOptimisticUserTurnFromDraft(
  draft: PromptDraft,
  attachedResourcesFallback: string
): MessageTurn {
  const displayText = getPromptDraftDisplayText(
    draft,
    attachedResourcesFallback
  )
  const resources = extractUserResourcesFromDraft(draft)
  const resourceLines = resources.map((resource) => {
    const label = resource.uri.toLowerCase().startsWith("file://")
      ? resource.name
      : `@${resource.name}`
    return `[${label}](${resource.uri})`
  })
  const text = [displayText, ...resourceLines].join("\n").trim()

  const blocks: ContentBlock[] = []
  for (const image of extractUserImagesFromDraft(draft)) {
    blocks.push({
      type: "image",
      data: image.data,
      mime_type: image.mime_type,
      uri: image.uri ?? null,
    })
  }
  blocks.push({ type: "text", text })

  return {
    id: `optimistic-${crypto.randomUUID()}`,
    role: "user",
    blocks,
    timestamp: new Date().toISOString(),
  }
}

function normalizeErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

function isExpectedAutoLinkError(error: unknown): boolean {
  if (!error || typeof error !== "object") return false
  return (error as { alerted?: unknown }).alerted === true
}

function buildVirtualConversationId(seed: string): number {
  let hash = 0
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) | 0
  }
  const normalized = Math.abs(hash) + 1
  return -normalized
}

const ConversationTabView = memo(function ConversationTabView({
  tabId,
  conversationId,
  agentType,
  workingDir,
  isActive,
  reloadSignal,
}: ConversationTabViewProps) {
  const t = useTranslations("Folder.conversation")
  const tWelcome = useTranslations("Folder.chat.welcomeInputPanel")
  const sharedT = useTranslations("Folder.chat.shared")
  const { folder, folderId, refreshConversations } = useFolderContext()
  const { tabs, bindConversationTab, setTabRuntimeConversationId } =
    useTabContext()
  const { setSessionStats } = useSessionStats()
  const {
    appendOptimisticTurn,
    completeTurn,
    getSession,
    refetchDetail,
    syncTurnMetadata,
    removeConversation,
    setExternalId,
    setLiveMessage,
    setPendingCleanup,
    setSyncState,
  } = useConversationRuntime()

  // Stable runtime session key — set once at mount, never changes.
  // For new conversations this is a virtual (negative) ID; for existing
  // conversations opened from the sidebar it equals the real DB ID.
  const [effectiveConversationId] = useState(
    () => conversationId ?? buildVirtualConversationId(`draft-${tabId}`)
  )
  const [createdConversationId, setCreatedConversationId] = useState<
    number | null
  >(null)
  const dbConversationId = conversationId ?? createdConversationId
  const [draftAgentType, setDraftAgentType] = useState<AgentType>(agentType)
  const selectedAgent = conversationId != null ? agentType : draftAgentType
  const [modeId, setModeId] = useState<string | null>(null)
  const [sendSignal, setSendSignal] = useState(0)
  const [agentsLoaded, setAgentsLoaded] = useState(false)
  const [usableAgentCount, setUsableAgentCount] = useState(0)
  const [agentConnectError, setAgentConnectError] = useState<string | null>(
    null
  )
  const [hasSentMessage, setHasSentMessage] = useState(false)

  const hasPersistedConversation = dbConversationId != null
  const canAutoConnect =
    hasPersistedConversation || (agentsLoaded && usableAgentCount > 0)

  // Expose the runtime session key to the tab so the aux panel (Diff sidebar)
  // can look up live turns even before the DB conversation is created.
  useEffect(() => {
    if (effectiveConversationId !== conversationId) {
      setTabRuntimeConversationId(tabId, effectiveConversationId)
    }
  }, [
    tabId,
    effectiveConversationId,
    conversationId,
    setTabRuntimeConversationId,
  ])

  // Clear pendingCleanup when tab is (re)opened
  useEffect(() => {
    setPendingCleanup(effectiveConversationId, false)
  }, [effectiveConversationId, setPendingCleanup])

  const latestReloadSignal = useRef(reloadSignal)
  const pendingReloadState = useRef<{
    signal: number
    sawLoading: boolean
  } | null>(null)
  const dbConvIdRef = useRef<number | null>(conversationId)
  const mountedRef = useRef(true)
  const statusUpdatedRef = useRef(false)
  const selectedAgentRef = useRef(selectedAgent)
  const createConversationPendingRef = useRef(false)
  // When the turn finishes (cancel / complete) before createConversation
  // resolves, we can't update the DB status yet.  This ref records the
  // desired status so the createConversation callback can apply it.
  const deferredStatusRef = useRef<string | null>(null)
  const externalIdSavedRef = useRef(false)
  const sessionIdRef = useRef<string | null>(null)
  const syncCancelRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    dbConvIdRef.current = dbConversationId
  }, [dbConversationId])

  useEffect(() => {
    selectedAgentRef.current = selectedAgent
  }, [selectedAgent])

  const {
    detail,
    loading: detailLoading,
    error: detailError,
  } = useConversationDetail(effectiveConversationId)

  const runtimeSession = getSession(effectiveConversationId)
  const effectiveSessionStats = runtimeSession?.sessionStats ?? null

  useEffect(() => {
    if (!isActive) return
    setSessionStats(effectiveSessionStats)
  }, [effectiveSessionStats, isActive, setSessionStats])

  const externalId = detail?.summary.external_id ?? undefined
  const draftStorageKey = useMemo(() => {
    if (dbConversationId != null) {
      return buildConversationDraftStorageKey(selectedAgent, dbConversationId)
    }
    return buildNewConversationDraftStorageKey({ folderId })
  }, [dbConversationId, folderId, selectedAgent])
  const workingDirForConnection = useMemo(() => {
    if (dbConversationId != null) {
      return detailLoading ? undefined : folder?.path
    }
    return workingDir ?? folder?.path
  }, [dbConversationId, detailLoading, folder?.path, workingDir])

  const {
    conn,
    modeLoading,
    configOptionsLoading,
    autoConnectError,
    handleFocus,
    handleSend: lifecycleSend,
    handleSetConfigOption,
    handleCancel,
    handleRespondPermission,
  } = useConnectionLifecycle({
    contextKey: tabId,
    agentType: selectedAgent,
    isActive: isActive && canAutoConnect,
    workingDir: workingDirForConnection,
    sessionId: dbConversationId != null ? externalId : undefined,
  })
  const {
    status: connStatus,
    connect: connConnect,
    disconnect: connDisconnect,
    sessionId: connSessionId,
  } = conn
  const messageQueue = useMessageQueue()
  const {
    queue: msgQueue,
    enqueue: mqEnqueue,
    dequeue: mqDequeue,
    remove: mqRemove,
    reorder: mqReorder,
    updateItem: mqUpdateItem,
    editingItemId: mqEditingItemId,
    startEditing: mqStartEditing,
    cancelEditing: mqCancelEditing,
  } = messageQueue
  const connStatusRef = useRef(connStatus)
  useEffect(() => {
    connStatusRef.current = connStatus
  }, [connStatus])
  const isConnecting =
    connStatus === "connecting" || connStatus === "downloading"
  const connectionModes = useMemo(
    () => conn.modes?.available_modes ?? [],
    [conn.modes?.available_modes]
  )
  const connectionConfigOptions = useMemo(
    () => conn.configOptions ?? [],
    [conn.configOptions]
  )
  const connectionCommands = useMemo(
    () => conn.availableCommands ?? [],
    [conn.availableCommands]
  )
  const selectedModeId = useMemo(() => {
    if (connectionModes.length === 0) return null
    if (modeId && connectionModes.some((mode) => mode.id === modeId)) {
      return modeId
    }
    return conn.modes?.current_mode_id ?? connectionModes[0]?.id ?? null
  }, [conn.modes?.current_mode_id, connectionModes, modeId])

  useEffect(() => {
    if (connSessionId) {
      sessionIdRef.current = connSessionId
    }
  }, [connSessionId])

  // completeTurn MUST be declared BEFORE setLiveMessage so that React runs
  // its cleanup/setup before setLiveMessage's cleanup. When connStatus
  // transitions away from "prompting", completeTurn snapshots and promotes
  // the liveMessage first, then setLiveMessage's cleanup safely clears it.
  const prevConnStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevConnStatusRef.current === "prompting"
    prevConnStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return

    // Turn completed — promote liveMessage + optimisticTurns to localTurns
    completeTurn(effectiveConversationId)

    // Cancel previous metadata sync (handles rapid consecutive turns)
    syncCancelRef.current?.()
    syncCancelRef.current = null

    const targetStatus =
      connStatus === "disconnected" || connStatus === "error"
        ? null
        : "pending_review"

    const persistedId = dbConvIdRef.current
    if (!persistedId) {
      // Conversation hasn't been persisted yet (createConversation still
      // in flight).  Record the desired status so the create callback
      // can apply it once the DB row exists.
      if (targetStatus) {
        deferredStatusRef.current = targetStatus
      }
      return
    }

    // Async patch metadata (usage, duration_ms, model, session_stats)
    if (persistedId > 0) {
      syncCancelRef.current = syncTurnMetadata(
        persistedId,
        effectiveConversationId
      )
    }

    if (targetStatus) {
      updateConversationStatus(persistedId, targetStatus)
        .then(() => refreshConversations())
        .catch((e: unknown) =>
          console.error("[ConversationTabView] update status:", e)
        )
    }
  }, [
    completeTurn,
    connStatus,
    effectiveConversationId,
    refreshConversations,
    syncTurnMetadata,
  ])

  // Auto-send queued messages when agent finishes responding.
  // Refs are synced via useEffect; the auto-send effect is declared
  // AFTER completeTurn so React runs it second.
  const autoSendQueueRef = useRef<() => QueuedMessage | undefined>(mqDequeue)
  useEffect(() => {
    autoSendQueueRef.current = mqDequeue
  }, [mqDequeue])
  const handleSendRef = useRef<
    (draft: PromptDraft, modeId?: string | null) => void
  >(() => {})

  const prevAutoSendStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevAutoSendStatusRef.current === "prompting"
    prevAutoSendStatusRef.current = connStatus
    if (!wasPrompting || connStatus !== "connected") return

    // Use queueMicrotask to ensure completeTurn effect has fully committed
    queueMicrotask(() => {
      const next = autoSendQueueRef.current()
      if (next) {
        handleSendRef.current(next.draft, next.modeId)
      }
    })
  }, [connStatus])

  useEffect(() => {
    // Only sync non-null liveMessage updates to state. When conn.liveMessage
    // goes null (agent finished streaming), don't clear state.liveMessage —
    // COMPLETE_TURN needs to snapshot it when connStatus transitions.
    // Clearing is handled by COMPLETE_TURN (sets liveMessage = null) and
    // by this effect's cleanup (when not prompting).
    if (conn.liveMessage != null) {
      setLiveMessage(effectiveConversationId, conn.liveMessage)
    }
    return () => {
      // Don't clear liveMessage if agent is still responding — the session
      // is kept via pendingCleanup, and clearing here would cause the
      // SET_LIVE_MESSAGE guard to block the reconnect liveMessage on reopen.
      if (connStatusRef.current !== "prompting") {
        setLiveMessage(effectiveConversationId, null)
      }
    }
  }, [conn.liveMessage, effectiveConversationId, setLiveMessage])

  useEffect(() => {
    if (effectiveConversationId <= 0) return
    setExternalId(effectiveConversationId, detail?.summary.external_id ?? null)
  }, [effectiveConversationId, detail?.summary.external_id, setExternalId])

  useEffect(() => {
    if (!connSessionId) return
    setExternalId(effectiveConversationId, connSessionId)
  }, [connSessionId, effectiveConversationId, setExternalId])

  const trySaveExternalId = useCallback(() => {
    if (
      externalIdSavedRef.current ||
      !dbConvIdRef.current ||
      !sessionIdRef.current
    ) {
      return
    }
    externalIdSavedRef.current = true
    updateConversationExternalId(
      dbConvIdRef.current,
      sessionIdRef.current
    ).catch((e: unknown) =>
      console.error("[ConversationTabView] update external_id:", e)
    )
  }, [])

  useEffect(() => {
    if (connSessionId) {
      trySaveExternalId()
    }
  }, [connSessionId, trySaveExternalId])

  useEffect(() => {
    if (connStatus === "connected" || connStatus === "prompting") {
      statusUpdatedRef.current = false
      return
    }
    if (statusUpdatedRef.current) return
    const persistedId = dbConvIdRef.current
    if (!persistedId) return
    if (connStatus === "disconnected") {
      statusUpdatedRef.current = true
      updateConversationStatus(persistedId, "completed")
        .then(() => refreshConversations())
        .catch((e) => console.error("[ConversationTabView] update status:", e))
    } else if (connStatus === "error") {
      statusUpdatedRef.current = true
      updateConversationStatus(persistedId, "cancelled")
        .then(() => refreshConversations())
        .catch((e) => console.error("[ConversationTabView] update status:", e))
    }
  }, [connStatus, refreshConversations])

  useEffect(() => {
    if (dbConversationId == null) return
    if (reloadSignal === latestReloadSignal.current) return
    latestReloadSignal.current = reloadSignal
    pendingReloadState.current = {
      signal: reloadSignal,
      sawLoading: false,
    }
    refetchDetail(dbConversationId)
  }, [dbConversationId, reloadSignal, refetchDetail])

  useEffect(() => {
    const pending = pendingReloadState.current
    if (!pending) return

    if (detailLoading) {
      pending.sawLoading = true
      return
    }

    if (!pending.sawLoading) return

    pendingReloadState.current = null

    if (detailError) {
      toast.error(t("reloadFailed", { message: detailError }))
      return
    }

    toast.success(t("reloaded"))
  }, [detailLoading, detailError, t])

  // Cleanup runtime data on unmount (tab close)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      syncCancelRef.current?.()
      if (connStatusRef.current === "prompting") {
        // Agent still responding — mark for deferred cleanup
        setPendingCleanup(effectiveConversationId, true)
      } else {
        removeConversation(effectiveConversationId)
      }
    }
  }, [effectiveConversationId, removeConversation, setPendingCleanup])

  const handleSend = useCallback(
    (draft: PromptDraft, selectedModeIdArg?: string | null) => {
      if (!hasPersistedConversation && !canAutoConnect) {
        setAgentConnectError(tWelcome("enableAgentFirstPlaceholder"))
        return
      }
      if (connStatus !== "connected") return

      const optimisticTurn = buildOptimisticUserTurnFromDraft(
        draft,
        sharedT("attachedResources")
      )
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((prev) => prev + 1)
      setSyncState(effectiveConversationId, "awaiting_persist")
      setHasSentMessage(true)
      lifecycleSend(draft, selectedModeIdArg)

      const persistedId = dbConvIdRef.current
      if (persistedId) {
        updateConversationStatus(persistedId, "in_progress")
          .then(() => refreshConversations())
          .catch((e: unknown) =>
            console.error("[ConversationTabView] update status:", e)
          )
        statusUpdatedRef.current = false
        return
      }

      if (createConversationPendingRef.current) return
      createConversationPendingRef.current = true
      const title = getPromptDraftDisplayText(
        draft,
        sharedT("attachedResources")
      ).slice(0, 80)
      createConversation(folderId, selectedAgent, title)
        .then((newConversationId) => {
          dbConvIdRef.current = newConversationId
          // Set external ID on the stable virtual session (no migration needed —
          // effectiveConversationId never changes, so the session stays in place)
          setExternalId(effectiveConversationId, sessionIdRef.current ?? null)
          trySaveExternalId()

          if (!mountedRef.current) {
            // Component unmounted while creating — mark for deferred cleanup
            // so the background turn_complete handler can clean up later.
            setPendingCleanup(effectiveConversationId, true)
            refreshConversations()
            return
          }

          setCreatedConversationId(newConversationId)
          bindConversationTab(
            tabId,
            newConversationId,
            selectedAgent,
            title,
            effectiveConversationId
          )
          moveMessageInputDraft(
            buildNewConversationDraftStorageKey({ folderId }),
            buildConversationDraftStorageKey(selectedAgent, newConversationId)
          )
          statusUpdatedRef.current = false
          // If the turn already finished while we were creating the
          // conversation, apply the deferred status directly instead
          // of setting "in_progress" (which would never be updated).
          const initialStatus = deferredStatusRef.current ?? "in_progress"
          deferredStatusRef.current = null
          updateConversationStatus(newConversationId, initialStatus)
            .then(() => refreshConversations())
            .catch((e: unknown) =>
              console.error("[ConversationTabView] update status:", e)
            )
        })
        .catch((e: unknown) =>
          console.error("[ConversationTabView] create conversation:", e)
        )
        .finally(() => {
          createConversationPendingRef.current = false
        })
    },
    [
      appendOptimisticTurn,
      bindConversationTab,
      canAutoConnect,
      connStatus,
      effectiveConversationId,
      folderId,
      hasPersistedConversation,
      lifecycleSend,
      refreshConversations,
      selectedAgent,
      setExternalId,
      setPendingCleanup,
      setSyncState,
      sharedT,
      tWelcome,
      tabId,
      trySaveExternalId,
    ]
  )

  // Sync handleSend ref for auto-send effect (declared before handleSend)
  useEffect(() => {
    handleSendRef.current = handleSend
  }, [handleSend])

  // Resolve the current conversation title from tab context (most up-to-date)
  // or fall back to the DB detail summary.
  const conversationTitle = useMemo(() => {
    const tabTitle = tabs.find((tab) => tab.id === tabId)?.title
    return tabTitle || detail?.summary.title || null
  }, [tabs, tabId, detail?.summary.title])

  const handleForkSend = useCallback(
    async (draft: PromptDraft, selectedModeIdArg?: string | null) => {
      const connectionId = conn.connectionId
      if (!connectionId || connStatus !== "connected") return
      try {
        const { forkedSessionId, originalSessionId } =
          await acpFork(connectionId)
        const persistedId = dbConvIdRef.current
        if (persistedId != null) {
          const baseTitle = conversationTitle ?? t("newConversation")
          // Strip existing [Fork] prefix to avoid stacking
          const cleanTitle = baseTitle.replace(/^\[Fork]\s*/g, "")
          // Point current conversation at S2 (forked) and add fork tag
          await updateConversationExternalId(persistedId, forkedSessionId)
          await updateConversationTitle(persistedId, `[Fork] ${cleanTitle}`)
          // Save original S1 as a separate conversation with original title
          const s1ConvId = await createConversation(
            folderId,
            selectedAgent,
            cleanTitle
          )
          await updateConversationExternalId(s1ConvId, originalSessionId)
          await updateConversationStatus(s1ConvId, "pending_review")
        }
        // Update runtime session id to S2
        sessionIdRef.current = forkedSessionId
        setExternalId(effectiveConversationId, forkedSessionId)

        await refreshConversations()
        // Send the message on the forked session (S2)
        handleSend(draft, selectedModeIdArg)
      } catch (err) {
        toast.error(
          t("forkSessionFailed", {
            error:
              err instanceof Error
                ? err.message
                : typeof err === "object" && err !== null
                  ? JSON.stringify(err)
                  : String(err),
          })
        )
      }
    },
    [
      conn.connectionId,
      connStatus,
      conversationTitle,
      effectiveConversationId,
      folderId,
      handleSend,
      refreshConversations,
      selectedAgent,
      setExternalId,
      t,
    ]
  )

  const handleOpenAgentsSettings = useCallback(() => {
    openSettingsWindow("agents", { agentType: selectedAgent }).catch((err) => {
      console.error(
        "[ConversationTabView] failed to open settings window:",
        err
      )
    })
  }, [selectedAgent])

  const handleAgentSelect = useCallback(
    (nextAgentType: AgentType) => {
      if (nextAgentType === selectedAgentRef.current) return
      if (dbConvIdRef.current) return

      setDraftAgentType(nextAgentType)
      setModeId(null)
      setAgentConnectError(null)

      const s = connStatusRef.current
      const doConnect = () => {
        if (!workingDirForConnection) return
        connConnect(nextAgentType, workingDirForConnection, undefined, {
          source: "auto_link",
        })
          .then(() => {
            setAgentConnectError(null)
          })
          .catch((e) => {
            setAgentConnectError(normalizeErrorMessage(e))
            if (!isExpectedAutoLinkError(e)) {
              console.error("[ConversationTabView] switch agent:", e)
            }
          })
      }

      // If not yet connected, directly attempt to connect with the new agent.
      if (!s || s === "disconnected" || s === "error") {
        doConnect()
        return
      }

      connDisconnect()
        .catch((e) =>
          console.error("[ConversationTabView] disconnect old agent:", e)
        )
        .finally(doConnect)
    },
    [connConnect, connDisconnect, workingDirForConnection]
  )

  const handleAnswerQuestion = useCallback(
    (answer: string) => {
      if (connStatus !== "connected") return
      const optimisticTurn: MessageTurn = {
        id: `optimistic-${crypto.randomUUID()}`,
        role: "user",
        blocks: [{ type: "text", text: answer }],
        timestamp: new Date().toISOString(),
      }
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((prev) => prev + 1)
      setSyncState(effectiveConversationId, "awaiting_persist")
      lifecycleSend(
        { blocks: [{ type: "text", text: answer }], displayText: answer },
        null
      )
    },
    [
      appendOptimisticTurn,
      connStatus,
      effectiveConversationId,
      lifecycleSend,
      setSyncState,
    ]
  )

  // Queue edit flow: derive editing draft text from queue state
  const editingQueueDraftText = useMemo(() => {
    if (!mqEditingItemId) return null
    const item = msgQueue.find((m) => m.id === mqEditingItemId)
    return item?.draft.displayText ?? null
  }, [mqEditingItemId, msgQueue])

  const handleQueueEdit = useCallback(
    (id: string) => {
      mqStartEditing(id)
    },
    [mqStartEditing]
  )

  const handleQueueCancelEdit = useCallback(() => {
    mqCancelEditing()
  }, [mqCancelEditing])

  const handleSaveQueueEdit = useCallback(
    (draft: PromptDraft) => {
      if (mqEditingItemId) {
        mqUpdateItem(mqEditingItemId, draft)
      }
    },
    [mqEditingItemId, mqUpdateItem]
  )

  const showDraftHeader = !hasPersistedConversation && !hasSentMessage
  const isWelcomeMode = showDraftHeader

  const messageListNode = (
    <MessageListView
      conversationId={effectiveConversationId}
      agentType={selectedAgent}
      connStatus={connStatus}
      isActive={isActive}
      sendSignal={sendSignal}
      sessionStats={effectiveSessionStats}
      detailLoading={detailLoading}
      detailError={detailError}
      hideEmptyState={!hasPersistedConversation || hasSentMessage}
    />
  )

  return (
    <ConversationShell
      status={connStatus}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={workingDirForConnection}
      error={conn.error}
      pendingPermission={conn.pendingPermission}
      pendingQuestion={conn.pendingQuestion}
      onFocus={handleFocus}
      onSend={handleSend}
      onCancel={handleCancel}
      onRespondPermission={handleRespondPermission}
      onAnswerQuestion={handleAnswerQuestion}
      modes={connectionModes}
      configOptions={connectionConfigOptions}
      modeLoading={modeLoading}
      configOptionsLoading={configOptionsLoading}
      selectedModeId={selectedModeId}
      onModeChange={setModeId}
      onConfigOptionChange={handleSetConfigOption}
      availableCommands={connectionCommands}
      attachmentTabId={tabId}
      draftStorageKey={draftStorageKey}
      hideInput={isWelcomeMode}
      isActive={isActive}
      queue={msgQueue}
      onEnqueue={mqEnqueue}
      onQueueReorder={mqReorder}
      onQueueEdit={handleQueueEdit}
      onQueueDelete={mqRemove}
      editingItemId={mqEditingItemId}
      editingDraftText={editingQueueDraftText}
      isEditingQueueItem={mqEditingItemId != null}
      onSaveQueueEdit={handleSaveQueueEdit}
      onCancelQueueEdit={handleQueueCancelEdit}
      onForkSend={
        connStatus === "connected" &&
        hasPersistedConversation &&
        conn.supportsFork
          ? handleForkSend
          : undefined
      }
    >
      {isWelcomeMode ? (
        <div className="flex h-full min-h-0 flex-col items-center justify-center">
          <div className="flex w-full max-w-2xl flex-col gap-4 px-4">
            <AgentSelector
              defaultAgentType={
                conversationId != null ? selectedAgent : undefined
              }
              onSelect={handleAgentSelect}
              onAgentsLoaded={(agents) => {
                setAgentsLoaded(true)
                setUsableAgentCount(
                  agents.filter((agent) => agent.enabled && agent.available)
                    .length
                )
              }}
              onOpenAgentsSettings={handleOpenAgentsSettings}
              disabled={isConnecting || dbConversationId != null}
            />
            {autoConnectError || agentConnectError ? (
              <button
                type="button"
                onClick={handleOpenAgentsSettings}
                className="w-full cursor-pointer rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-center text-xs text-destructive transition-colors hover:bg-destructive/10"
              >
                <div
                  className="overflow-hidden text-ellipsis whitespace-nowrap text-center"
                  title={autoConnectError ?? agentConnectError ?? ""}
                >
                  {autoConnectError ?? agentConnectError}
                </div>
              </button>
            ) : null}
            <ChatInput
              status={connStatus}
              promptCapabilities={conn.promptCapabilities}
              defaultPath={workingDirForConnection}
              onFocus={handleFocus}
              onSend={handleSend}
              onCancel={handleCancel}
              modes={connectionModes}
              configOptions={connectionConfigOptions}
              modeLoading={modeLoading}
              configOptionsLoading={configOptionsLoading}
              selectedModeId={selectedModeId}
              onModeChange={setModeId}
              onConfigOptionChange={handleSetConfigOption}
              availableCommands={connectionCommands}
              attachmentTabId={tabId}
              draftStorageKey={draftStorageKey}
              isActive={isActive}
            />
          </div>
        </div>
      ) : showDraftHeader ? (
        <div className="flex h-full min-h-0 flex-col">
          <div className="px-4 pt-3 pb-2">
            <AgentSelector
              defaultAgentType={
                conversationId != null ? selectedAgent : undefined
              }
              onSelect={handleAgentSelect}
              onAgentsLoaded={(agents) => {
                setAgentsLoaded(true)
                setUsableAgentCount(
                  agents.filter((agent) => agent.enabled && agent.available)
                    .length
                )
              }}
              onOpenAgentsSettings={handleOpenAgentsSettings}
              disabled={isConnecting || dbConversationId != null}
            />
            {autoConnectError || agentConnectError ? (
              <button
                type="button"
                onClick={handleOpenAgentsSettings}
                className="mt-2 w-full cursor-pointer rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-center text-xs text-destructive transition-colors hover:bg-destructive/10"
              >
                <div
                  className="overflow-hidden text-ellipsis whitespace-nowrap text-center"
                  title={autoConnectError ?? agentConnectError ?? ""}
                >
                  {autoConnectError ?? agentConnectError}
                </div>
              </button>
            ) : null}
          </div>
          <div className="min-h-0 flex-1">{messageListNode}</div>
        </div>
      ) : (
        messageListNode
      )}
    </ConversationShell>
  )
})

export function ConversationDetailPanel() {
  const t = useTranslations("Folder.conversation")
  const {
    completeTurn: runtimeCompleteTurn,
    getConversationIdByExternalId,
    getSession,
    removeConversation: runtimeRemoveConversation,
  } = useConversationRuntime()
  const { folder, newConversation, conversations, refreshConversations } =
    useFolderContext()
  const {
    tabs,
    activeTabId,
    isTileMode,
    openNewConversationTab,
    closeTab,
    switchTab,
    onPreviewTabReplaced,
  } = useTabContext()
  const { disconnect: disconnectByKey } = useAcpActions()
  const [reloadByTabId, setReloadByTabId] = useState<Record<string, number>>({})
  const tabsRef = useRef(tabs)
  const conversationsRef = useRef(conversations)

  useEffect(() => {
    tabsRef.current = tabs
  }, [tabs])

  useEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])

  // Disconnect the old connection immediately when a preview tab is replaced
  useEffect(() => {
    return onPreviewTabReplaced((replacedTabId) => {
      disconnectByKey(replacedTabId).catch(() => {})
    })
  }, [onPreviewTabReplaced, disconnectByKey])

  // Refs for background turn_complete handler so the listener
  // can be registered once and always read the latest values.
  const getConversationIdByExternalIdRef = useRef(getConversationIdByExternalId)
  const getSessionRef = useRef(getSession)
  const runtimeCompleteTurnRef = useRef(runtimeCompleteTurn)
  const runtimeRemoveConversationRef = useRef(runtimeRemoveConversation)
  const refreshConversationsRef = useRef(refreshConversations)
  useEffect(() => {
    getConversationIdByExternalIdRef.current = getConversationIdByExternalId
  }, [getConversationIdByExternalId])
  useEffect(() => {
    getSessionRef.current = getSession
  }, [getSession])
  useEffect(() => {
    runtimeCompleteTurnRef.current = runtimeCompleteTurn
  }, [runtimeCompleteTurn])
  useEffect(() => {
    runtimeRemoveConversationRef.current = runtimeRemoveConversation
  }, [runtimeRemoveConversation])
  useEffect(() => {
    refreshConversationsRef.current = refreshConversations
  }, [refreshConversations])

  // Background turn_complete handler: for conversations not open in tabs.
  // Registered once — uses refs to avoid re-creating the listener on every
  // state change, which would cause "Couldn't find callback id" warnings
  // due to the async gap between unlisten and the new listen().
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void | Promise<void>) | null = null

    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<AcpEvent>("acp://event", (event) => {
          const payload = event.payload
          if (payload.type !== "turn_complete") return

          const runtimeConversationId =
            getConversationIdByExternalIdRef.current(payload.session_id)
          const summary = conversationsRef.current.find(
            (item) => item.external_id === payload.session_id
          )
          const matchedConversationId =
            runtimeConversationId ?? summary?.id ?? null
          if (!matchedConversationId) return

          // Check both virtual (runtime) ID and real DB ID — after
          // bindConversationTab the tab stores the real DB ID while the
          // runtime session may still be keyed by the virtual ID.
          const dbId2 = summary?.id
          const isOpenInTabs = tabsRef.current.some(
            (tab) =>
              tab.conversationId === matchedConversationId ||
              (dbId2 != null && tab.conversationId === dbId2)
          )
          if (isOpenInTabs) return

          // Promote liveMessage + optimisticTurns to localTurns immediately
          runtimeCompleteTurnRef.current(matchedConversationId)

          // If tab was closed while agent was responding, clean up now
          const session = getSessionRef.current(matchedConversationId)
          if (session?.pendingCleanup) {
            runtimeRemoveConversationRef.current(matchedConversationId)
          }

          // Update conversation status — use the DB summary (found by
          // external_id above) since matchedConversationId may be a virtual
          // (negative) ID that won't match any DB record.
          const dbId =
            summary?.id ??
            (matchedConversationId > 0 ? matchedConversationId : null)
          if (dbId && (!summary || summary.status === "in_progress")) {
            updateConversationStatus(dbId, "pending_review")
              .then(() => refreshConversationsRef.current())
              .catch((error: unknown) =>
                console.error(
                  "[ConversationDetailPanel] background update status:",
                  error
                )
              )
          }
        })
      )
      .then((dispose) => {
        if (cancelled) {
          disposeTauriListener(
            dispose,
            "ConversationDetailPanel.backgroundRefresh"
          )
          return
        }
        unlisten = dispose
      })
      .catch(() => {
        // Ignore when non-tauri runtime.
      })

    return () => {
      cancelled = true
      disposeTauriListener(
        unlisten,
        "ConversationDetailPanel.backgroundRefresh"
      )
    }
  }, [])

  const hasNoTabs = tabs.length === 0 && !activeTabId
  const activeConversationTab = useMemo(
    () =>
      tabs.find(
        (tab) => tab.id === activeTabId && tab.conversationId != null
      ) ?? null,
    [tabs, activeTabId]
  )
  const canReloadActiveConversation = activeConversationTab != null
  const handleReloadActiveConversation = useCallback(() => {
    if (!activeConversationTab) return
    setReloadByTabId((prev) => ({
      ...prev,
      [activeConversationTab.id]: (prev[activeConversationTab.id] ?? 0) + 1,
    }))
  }, [activeConversationTab])

  const handleNewConversation = useCallback(() => {
    if (!folder) return
    openNewConversationTab(folder.path)
  }, [folder, openNewConversationTab])

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTabId) return
    closeTab(activeTabId)
  }, [activeTabId, closeTab])

  // Ensure no-tab state is immediately bridged to a real new-conversation tab.
  useEffect(() => {
    if (!folder) return

    if (hasNoTabs) {
      openNewConversationTab(newConversation?.workingDir ?? folder.path)
    }
  }, [folder, hasNoTabs, newConversation?.workingDir, openNewConversationTab])

  const canTile = isTileMode && tabs.length > 1

  // Empty state: no tabs at all — show full-screen welcome
  if (hasNoTabs) {
    return null
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="relative h-full min-h-0 overflow-hidden">
          {canTile ? (
            <ResizablePanelGroup direction="horizontal">
              {tabs.map((tab, index) => {
                const active = tab.id === activeTabId
                return (
                  <Fragment key={tab.id}>
                    {index > 0 && <ResizableHandle withHandle />}
                    <ResizablePanel
                      id={`tile-${tab.id}`}
                      order={index}
                      minSize={15}
                    >
                      <div
                        className={cn(
                          "h-full",
                          active
                            ? "bg-gradient-to-b from-muted/50 to-transparent"
                            : ""
                        )}
                        onPointerDownCapture={() => {
                          if (!active) switchTab(tab.id)
                        }}
                      >
                        <ConversationTabView
                          tabId={tab.id}
                          conversationId={tab.conversationId}
                          agentType={tab.agentType}
                          workingDir={tab.workingDir ?? folder?.path}
                          isActive={active}
                          reloadSignal={reloadByTabId[tab.id] ?? 0}
                        />
                      </div>
                    </ResizablePanel>
                  </Fragment>
                )
              })}
            </ResizablePanelGroup>
          ) : (
            tabs.map((tab) => {
              const active = tab.id === activeTabId
              return (
                <div
                  key={tab.id}
                  className={
                    active
                      ? "h-full"
                      : "absolute inset-0 invisible pointer-events-none"
                  }
                >
                  <ConversationTabView
                    tabId={tab.id}
                    conversationId={tab.conversationId}
                    agentType={tab.agentType}
                    workingDir={tab.workingDir ?? folder?.path}
                    isActive={active}
                    reloadSignal={reloadByTabId[tab.id] ?? 0}
                  />
                </div>
              )
            })
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem
          disabled={!canReloadActiveConversation}
          onSelect={handleReloadActiveConversation}
        >
          <RefreshCw className="h-4 w-4" />
          {t("reload")}
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!folder?.path}
          onSelect={handleNewConversation}
        >
          <Plus className="h-4 w-4" />
          {t("newConversation")}
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          disabled={!activeTabId}
          onSelect={handleCloseActiveTab}
        >
          <X className="h-4 w-4" />
          {t("closeConversation")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
