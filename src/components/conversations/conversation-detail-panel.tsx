"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Plus, RefreshCw, X } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useFolderContext } from "@/contexts/folder-context"
import { useTabContext } from "@/contexts/tab-context"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { MessageListView } from "@/components/message/message-list-view"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { WelcomeInputPanel } from "@/components/chat/welcome-input-panel"
import { updateConversationStatus } from "@/lib/tauri"
import { useDbMessageDetail } from "@/hooks/use-db-message-detail"
import type { AgentType, PromptDraft } from "@/lib/types"
import type { AdaptedMessage } from "@/lib/adapters/ai-elements-adapter"
import {
  buildUserMessageTextPartsFromDraft,
  extractUserImagesFromDraft,
  extractUserResourcesFromDraft,
} from "@/lib/prompt-draft"
import { buildConversationDraftStorageKey } from "@/lib/message-input-draft"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"

interface ExistingConversationViewProps {
  tabId: string
  conversationId: number
  agentType: AgentType
  isActive: boolean
  reloadSignal: number
}

const ExistingConversationView = memo(function ExistingConversationView({
  tabId,
  conversationId,
  agentType,
  isActive,
  reloadSignal,
}: ExistingConversationViewProps) {
  const t = useTranslations("Folder.conversation")
  const sharedT = useTranslations("Folder.chat.shared")
  const { refreshConversations, folder } = useFolderContext()
  const contextKey = `conv-${agentType}-${conversationId}`

  // Get external_id to resume existing agent session via LoadSessionRequest.
  // Gate workingDir on loading so auto-connect waits for sessionId to resolve.
  const {
    detail,
    loading: detailLoading,
    error: detailError,
    refetch: refetchConversationDetail,
  } = useDbMessageDetail(conversationId)
  const externalId = detail?.summary.external_id ?? undefined
  const latestReloadSignal = useRef(reloadSignal)
  const pendingReloadState = useRef<{
    signal: number
    sawLoading: boolean
  } | null>(null)

  const {
    conn,
    modeLoading,
    configOptionsLoading,
    handleFocus,
    handleSend,
    handleSetConfigOption,
    handleCancel,
    handleRespondPermission,
  } = useConnectionLifecycle({
    contextKey,
    agentType,
    isActive,
    workingDir: detailLoading ? undefined : folder?.path,
    sessionId: externalId,
  })

  const [pendingMessages, setPendingMessages] = useState<AdaptedMessage[]>([])
  const [modeId, setModeId] = useState<string | null>(null)
  const clearPending = useCallback(() => setPendingMessages([]), [])

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

  // Track status transitions for updating conversation metadata
  const prevStatusRef = useRef(conn.status)
  const statusUpdatedRef = useRef(false)

  // Wrap handleSend to update status
  const handleSendWithPersist = useCallback(
    (draft: PromptDraft, selectedModeId?: string | null) => {
      setPendingMessages([
        {
          id: `pending-${Date.now()}`,
          role: "user",
          content: buildUserMessageTextPartsFromDraft(
            draft,
            sharedT("attachedResources")
          ),
          userImages: extractUserImagesFromDraft(draft),
          userResources: extractUserResourcesFromDraft(draft),
          timestamp: new Date().toISOString(),
        },
      ])
      updateConversationStatus(conversationId, "in_progress")
        .then(() => refreshConversations())
        .catch((e) => console.error("[ExistingConv] update status:", e))
      statusUpdatedRef.current = false
      handleSend(draft, selectedModeId)
    },
    [conversationId, handleSend, refreshConversations, sharedT]
  )

  // Update status on turn complete
  useEffect(() => {
    const prev = prevStatusRef.current
    prevStatusRef.current = conn.status

    if (prev === "prompting" && conn.status !== "prompting") {
      // Mark as pending_review unless it's a terminal state
      if (conn.status !== "disconnected" && conn.status !== "error") {
        updateConversationStatus(conversationId, "pending_review")
          .then(() => refreshConversations())
          .catch((e: unknown) =>
            console.error("[ExistingConv] update status:", e)
          )
      }
    }
  }, [conn.status, conversationId, refreshConversations])

  // Update status on disconnect/error
  useEffect(() => {
    if (conn.status === "connected" || conn.status === "prompting") {
      statusUpdatedRef.current = false
      return
    }
    if (statusUpdatedRef.current) return
    if (conn.status === "disconnected") {
      statusUpdatedRef.current = true
      updateConversationStatus(conversationId, "completed")
        .then(() => {
          setPendingMessages([])
          refreshConversations()
        })
        .catch((e) => console.error("[ExistingConv] update status:", e))
    } else if (conn.status === "error") {
      statusUpdatedRef.current = true
      updateConversationStatus(conversationId, "cancelled")
        .then(() => {
          setPendingMessages([])
          refreshConversations()
        })
        .catch((e) => console.error("[ExistingConv] update status:", e))
    }
  }, [conn.status, conversationId, refreshConversations])

  useEffect(() => {
    if (reloadSignal === latestReloadSignal.current) return
    latestReloadSignal.current = reloadSignal
    pendingReloadState.current = {
      signal: reloadSignal,
      sawLoading: false,
    }
    refetchConversationDetail()
  }, [reloadSignal, refetchConversationDetail])

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

  return (
    <ConversationShell
      status={conn.status}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={folder?.path}
      error={conn.error}
      pendingPermission={conn.pendingPermission}
      onFocus={handleFocus}
      onSend={handleSendWithPersist}
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
      draftStorageKey={buildConversationDraftStorageKey(
        agentType,
        conversationId
      )}
    >
      <MessageListView
        conversationId={conversationId}
        liveMessage={conn.liveMessage}
        connStatus={conn.status}
        pendingMessages={pendingMessages}
        onPendingClear={clearPending}
        isActive={isActive}
      />
    </ConversationShell>
  )
})

export function ConversationDetailPanel() {
  const t = useTranslations("Folder.conversation")
  const { folder, newConversation } = useFolderContext()
  const { tabs, activeTabId, openNewConversationTab, closeTab } =
    useTabContext()
  const [reloadByTabId, setReloadByTabId] = useState<Record<string, number>>({})

  const conversationTabs = useMemo(
    () =>
      tabs.filter((t) => t.kind === "conversation" && t.conversationId != null),
    [tabs]
  )

  const newConvTabs = useMemo(
    () => tabs.filter((t) => t.kind === "new_conversation"),
    [tabs]
  )
  const hasNoTabs =
    conversationTabs.length === 0 && newConvTabs.length === 0 && !activeTabId
  const activeConversationTab = useMemo(
    () =>
      tabs.find(
        (tab) =>
          tab.id === activeTabId &&
          tab.kind === "conversation" &&
          tab.conversationId != null
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
    return (
      <WelcomeInputPanel
        defaultAgentType={newConversation?.agentType ?? "codex"}
        workingDir={newConversation?.workingDir ?? folder?.path}
      />
    )
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="relative h-full min-h-0 overflow-hidden">
          {conversationTabs.map((tab) => {
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
                <ExistingConversationView
                  tabId={tab.id}
                  conversationId={tab.conversationId!}
                  agentType={tab.agentType}
                  isActive={active}
                  reloadSignal={reloadByTabId[tab.id] ?? 0}
                />
              </div>
            )
          })}
          {newConvTabs.map((tab) => {
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
                <WelcomeInputPanel
                  defaultAgentType={tab.agentType ?? "codex"}
                  workingDir={tab.workingDir ?? folder?.path}
                  tabId={tab.id}
                  isActive={active}
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
