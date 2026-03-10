"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Plus, RefreshCw, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { disposeTauriListener } from "@/lib/tauri-listener"
import { useFolderContext } from "@/contexts/folder-context"
import { useTabContext } from "@/contexts/tab-context"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { MessageListView } from "@/components/message/message-list-view"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { AgentSelector } from "@/components/chat/agent-selector"
import {
  createConversation,
  openSettingsWindow,
  updateConversationExternalId,
  updateConversationStatus,
} from "@/lib/tauri"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import {
  invalidateDetailCache,
  refreshDetailCache,
  useDbMessageDetail,
} from "@/hooks/use-db-message-detail"
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
  const { bindConversationTab } = useTabContext()
  const {
    acknowledgePersistedDetail,
    appendOptimisticTurn,
    migrateConversation,
    setExternalId,
    setLiveMessage,
    setSyncState,
  } = useConversationRuntime()

  const temporaryConversationId = useMemo(
    () => buildVirtualConversationId(`draft-${tabId}`),
    [tabId]
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

  const hasPersistedConversation = dbConversationId != null
  const canAutoConnect =
    hasPersistedConversation || (agentsLoaded && usableAgentCount > 0)
  const effectiveConversationId = dbConversationId ?? temporaryConversationId

  const latestReloadSignal = useRef(reloadSignal)
  const pendingReloadState = useRef<{
    signal: number
    sawLoading: boolean
  } | null>(null)
  const dbConvIdRef = useRef<number | null>(conversationId)
  const statusUpdatedRef = useRef(false)
  const selectedAgentRef = useRef(selectedAgent)
  const createConversationPendingRef = useRef(false)
  const reconcileTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const externalIdSavedRef = useRef(false)
  const sessionIdRef = useRef<string | null>(null)

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
    refetch: refetchConversationDetail,
  } = useDbMessageDetail(effectiveConversationId)
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

  const clearReconcileTimer = useCallback(() => {
    if (!reconcileTimerRef.current) return
    clearTimeout(reconcileTimerRef.current)
    reconcileTimerRef.current = null
  }, [])

  const refreshFromDb = useCallback(
    async (refreshConversationId: number) => {
      try {
        const refreshed = await refreshDetailCache(refreshConversationId)
        acknowledgePersistedDetail(refreshConversationId, refreshed)
      } catch (error) {
        setSyncState(refreshConversationId, "failed")
        console.error(
          "[ConversationTabView] refresh detail cache failed:",
          error
        )
      }
    },
    [acknowledgePersistedDetail, setSyncState]
  )

  useEffect(() => {
    if (connSessionId) {
      sessionIdRef.current = connSessionId
    }
  }, [connSessionId])

  useEffect(() => {
    setLiveMessage(effectiveConversationId, conn.liveMessage ?? null)
    return () => {
      setLiveMessage(effectiveConversationId, null)
    }
  }, [conn.liveMessage, effectiveConversationId, setLiveMessage])

  useEffect(() => {
    if (!dbConversationId) return
    setExternalId(dbConversationId, detail?.summary.external_id ?? null)
  }, [dbConversationId, detail?.summary.external_id, setExternalId])

  useEffect(() => {
    if (!dbConversationId) return
    if (!connSessionId) return
    setExternalId(dbConversationId, connSessionId)
  }, [connSessionId, dbConversationId, setExternalId])

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
    if (!dbConversationId) return
    if (!detail) return
    if (connStatus === "prompting") return
    acknowledgePersistedDetail(dbConversationId, detail)
  }, [acknowledgePersistedDetail, connStatus, dbConversationId, detail])

  const prevStatusRef = useRef(connStatus)
  useEffect(() => {
    const prev = prevStatusRef.current
    prevStatusRef.current = connStatus
    if (prev !== "prompting" || connStatus === "prompting") return

    setSyncState(effectiveConversationId, "reconciling")
    const persistedId = dbConvIdRef.current
    if (!persistedId) return

    invalidateDetailCache(persistedId)
    clearReconcileTimer()
    reconcileTimerRef.current = setTimeout(() => {
      void refreshFromDb(persistedId)
    }, 1200)

    if (connStatus !== "disconnected" && connStatus !== "error") {
      updateConversationStatus(persistedId, "pending_review")
        .then(() => refreshConversations())
        .catch((e: unknown) =>
          console.error("[ConversationTabView] update status:", e)
        )
    }
  }, [
    clearReconcileTimer,
    connStatus,
    effectiveConversationId,
    refreshConversations,
    refreshFromDb,
    setSyncState,
  ])

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
    refetchConversationDetail()
  }, [dbConversationId, reloadSignal, refetchConversationDetail])

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

  useEffect(() => clearReconcileTimer, [clearReconcileTimer])

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
          setCreatedConversationId(newConversationId)
          migrateConversation(temporaryConversationId, newConversationId)
          setExternalId(newConversationId, sessionIdRef.current ?? null)
          bindConversationTab(tabId, newConversationId, selectedAgent, title)
          moveMessageInputDraft(
            buildNewConversationDraftStorageKey({ folderId }),
            buildConversationDraftStorageKey(selectedAgent, newConversationId)
          )
          trySaveExternalId()
          statusUpdatedRef.current = false
          updateConversationStatus(newConversationId, "in_progress")
            .then(() => refreshConversations())
            .catch((e: unknown) =>
              console.error("[ConversationTabView] update status:", e)
            )
          void refreshFromDb(newConversationId)
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
      migrateConversation,
      refreshConversations,
      refreshFromDb,
      selectedAgent,
      setExternalId,
      setSyncState,
      sharedT,
      tWelcome,
      tabId,
      temporaryConversationId,
      trySaveExternalId,
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
      connDisconnect()
        .catch((e) =>
          console.error("[ConversationTabView] disconnect old agent:", e)
        )
        .finally(() => {
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
        })
    },
    [connConnect, connDisconnect, workingDirForConnection]
  )

  const messageListNode = (
    <MessageListView
      conversationId={effectiveConversationId}
      connStatus={connStatus}
      isActive={isActive}
      sendSignal={sendSignal}
    />
  )

  const showDraftHeader = !hasPersistedConversation

  return (
    <ConversationShell
      status={connStatus}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={workingDirForConnection}
      error={conn.error}
      pendingPermission={conn.pendingPermission}
      onFocus={handleFocus}
      onSend={handleSend}
      onCancel={handleCancel}
      onRespondPermission={handleRespondPermission}
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
    >
      {showDraftHeader ? (
        <div className="flex h-full min-h-0 flex-col">
          <div className="px-4 pt-3 pb-2">
            <AgentSelector
              defaultAgentType={selectedAgent}
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
    acknowledgePersistedDetail,
    getConversationIdByExternalId,
    setSyncState,
  } = useConversationRuntime()
  const { folder, newConversation, conversations, refreshConversations } =
    useFolderContext()
  const { tabs, activeTabId, openNewConversationTab, closeTab } =
    useTabContext()
  const [reloadByTabId, setReloadByTabId] = useState<Record<string, number>>({})
  const tabsRef = useRef(tabs)
  const conversationsRef = useRef(conversations)
  const pendingClosedConversationIdsRef = useRef<Set<number>>(new Set())
  const pendingRefreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null
  )

  useEffect(() => {
    tabsRef.current = tabs
  }, [tabs])

  useEffect(() => {
    conversationsRef.current = conversations
  }, [conversations])

  const flushClosedConversationRefresh = useCallback(() => {
    const conversationIds = Array.from(pendingClosedConversationIdsRef.current)
    if (conversationIds.length === 0) return
    pendingClosedConversationIdsRef.current.clear()

    void (async () => {
      await Promise.all(
        conversationIds.map(async (conversationId) => {
          const summary =
            conversationsRef.current.find(
              (item) => item.id === conversationId
            ) ?? null
          if (summary?.status === "in_progress") {
            try {
              await updateConversationStatus(conversationId, "pending_review")
            } catch (error) {
              console.error(
                "[ConversationDetailPanel] background update status failed:",
                error
              )
            }
          }

          try {
            const detail = await refreshDetailCache(conversationId)
            acknowledgePersistedDetail(conversationId, detail)
          } catch (error) {
            setSyncState(conversationId, "failed")
            console.error(
              "[ConversationDetailPanel] background detail cache refresh failed:",
              error
            )
          }
        })
      )

      refreshConversations()
    })()
  }, [acknowledgePersistedDetail, refreshConversations, setSyncState])

  const scheduleClosedConversationRefresh = useCallback(
    (conversationId: number) => {
      pendingClosedConversationIdsRef.current.add(conversationId)
      if (pendingRefreshTimerRef.current) return

      // Delay briefly so local session file writes can settle.
      pendingRefreshTimerRef.current = setTimeout(() => {
        pendingRefreshTimerRef.current = null
        flushClosedConversationRefresh()
      }, 1200)
    },
    [flushClosedConversationRefresh]
  )

  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void | Promise<void>) | null = null
    const pendingClosedConversationIds = pendingClosedConversationIdsRef.current

    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<AcpEvent>("acp://event", (event) => {
          const payload = event.payload
          if (payload.type !== "turn_complete") return

          const runtimeConversationId = getConversationIdByExternalId(
            payload.session_id
          )
          const summary = conversationsRef.current.find(
            (item) => item.external_id === payload.session_id
          )
          const matchedConversationId =
            runtimeConversationId ?? summary?.id ?? null
          if (!matchedConversationId) return

          const isOpenInTabs = tabsRef.current.some(
            (tab) => tab.conversationId === matchedConversationId
          )
          if (isOpenInTabs) return

          invalidateDetailCache(matchedConversationId)
          setSyncState(matchedConversationId, "reconciling")

          scheduleClosedConversationRefresh(matchedConversationId)
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
      if (pendingRefreshTimerRef.current) {
        clearTimeout(pendingRefreshTimerRef.current)
        pendingRefreshTimerRef.current = null
      }
      pendingClosedConversationIds.clear()
      disposeTauriListener(
        unlisten,
        "ConversationDetailPanel.backgroundRefresh"
      )
    }
  }, [
    getConversationIdByExternalId,
    acknowledgePersistedDetail,
    scheduleClosedConversationRefresh,
    setSyncState,
  ])

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
    openNewConversationTab("codex", folder.path)
  }, [folder, openNewConversationTab])

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTabId) return
    closeTab(activeTabId)
  }, [activeTabId, closeTab])

  // Ensure no-tab state is immediately bridged to a real new-conversation tab.
  useEffect(() => {
    if (!folder) return

    if (hasNoTabs) {
      openNewConversationTab(
        newConversation?.agentType ?? "codex",
        newConversation?.workingDir ?? folder.path
      )
    }
  }, [
    folder,
    hasNoTabs,
    newConversation?.agentType,
    newConversation?.workingDir,
    openNewConversationTab,
  ])

  // Empty state: no tabs at all — show full-screen welcome
  if (hasNoTabs) {
    return null
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="relative h-full min-h-0 overflow-hidden">
          {tabs.map((tab) => {
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
          })}
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
