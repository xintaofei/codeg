"use client"

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react"
import {
  Copy,
  Download,
  FileCode,
  FileImage,
  FileText,
  Info,
  RefreshCw,
  SquarePen,
  X,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  getCachedSelectors,
  useAcpActions,
  useAcpEvent,
} from "@/contexts/acp-connections-context"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import { useTaskContext } from "@/contexts/task-context"
import { cn, copyTextFromMenu, randomUUID } from "@/lib/utils"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { useMessageQueue, type QueuedMessage } from "@/hooks/use-message-queue"
import { MessageListView } from "@/components/message/message-list-view"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { SessionConfigStaleBanner } from "@/components/chat/session-config-stale-banner"
import { BackgroundTasksChip } from "@/components/chat/background-tasks-chip"
import { FeedbackNotesDisplay } from "@/components/chat/feedback-notes-display"
import { FeedbackDialog } from "@/components/chat/feedback-dialog"
import { AgentDiagnosticsDialog } from "@/components/settings/agent-diagnostics-dialog"
import { useFeedbackEnabled } from "@/hooks/use-feedback-enabled"
import { useSessionFeedback } from "@/hooks/use-session-feedback"
import { AgentSelector } from "@/components/chat/agent-selector"
import { ChatInput } from "@/components/chat/chat-input"
import { WelcomeHero, WelcomeTip } from "@/components/chat/welcome-hero"
import { QuickActions } from "@/components/chat/quick-actions"
import type { ComposerInjectContent } from "@/components/chat/message-input"
import { TileScrollContainer } from "@/components/conversations/tile-scroll-container"
import {
  acpFork,
  createChatConversation,
  createChatDir,
  createConversation,
  openSettingsWindow,
} from "@/lib/api"
import {
  flushRetryDelayMs,
  forkSendBlockedByQueue,
  isConnectionReady,
  shouldQueueDirectSend,
  shouldRejectDuplicateCreate,
} from "@/lib/queue-flush"
import { TurnBusyError } from "@/lib/turn-busy"
import {
  getConversationIdByExternalIdFromStore,
  getRuntimeSession,
  useConversationRuntimeActions,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import { useShallow } from "zustand/react/shallow"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  extractUserImagesFromDraft,
  getPromptDraftDisplayText,
} from "@/lib/prompt-draft"
import {
  AGENT_LABELS,
  type AgentType,
  type ContentBlock,
  type ConversationStatus,
  type EventEnvelope,
  type MessageTurn,
  type PromptDraft,
  type QuestionAnswer,
  type UserMessageBlock,
} from "@/lib/types"
import {
  getSavedModeId,
  saveModePreference,
} from "@/lib/selector-prefs-storage"
import {
  buildConversationDraftStorageKey,
  buildNewConversationDraftStorageKey,
  clearMessageInputDraft,
  saveMessageInputDraft,
} from "@/lib/message-input-draft"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  exportAsHtml,
  exportAsImage,
  exportAsMarkdown,
  ExportTooLongError,
} from "@/lib/export-conversation"
import { useExportLabels } from "@/lib/use-export-labels"
import { resolveActiveSessionDetails } from "./active-session-details"
import { ConversationDetailHeader } from "./conversation-detail-header"
import { SessionDetailsDialog } from "./session-details-dialog"

interface ConversationTabViewProps {
  tabId: string
  conversationId: number | null
  agentType: AgentType
  workingDir?: string
  isActive: boolean
  /** Drive the composer's flowing active-session border. True only for the
   *  active tab while tiled across multiple sessions — the one place the flow
   *  serves as the "which tile is active" cue. Distinct from `isActive`, which
   *  also governs auto-focus/connect and is true even for a lone session. */
  showActiveFlow: boolean
  reloadSignal: number
}

function buildOptimisticUserTurnFromDraft(
  draft: PromptDraft,
  attachedResourcesFallback: string
): MessageTurn {
  // `draft.displayText` is the composer's full Markdown, which already renders
  // every inline file/resource badge as a `[label](uri)` link (see
  // `referenceToMarkdown`). Re-appending the resource blocks here would duplicate
  // each attached file in the optimistic bubble, so the display text is used
  // as-is — images are the only out-of-band content left to add as blocks.
  const text = getPromptDraftDisplayText(draft, attachedResourcesFallback)

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
    id: `optimistic-${randomUUID()}`,
    role: "user",
    blocks,
    timestamp: new Date().toISOString(),
  }
}

/** Build a user `MessageTurn` from a broadcast `user_message` (event or
 *  snapshot `pending_user_message`). Used by cross-client VIEWERS to render the
 *  sender's prompt. The turn `id` is the broadcast `message_id` so the runtime
 *  reducer can dedup it idempotently. */
function buildUserTurnFromMessageBlocks(
  messageId: string,
  blocks: UserMessageBlock[]
): MessageTurn {
  const contentBlocks: ContentBlock[] = blocks.map((b) =>
    b.type === "image"
      ? { type: "image", data: b.data, mime_type: b.mime_type, uri: null }
      : { type: "text", text: b.text }
  )
  return {
    id: messageId,
    role: "user",
    blocks: contentBlocks,
    timestamp: new Date().toISOString(),
  }
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
  showActiveFlow,
  reloadSignal,
}: ConversationTabViewProps) {
  const t = useTranslations("Folder.conversation")
  const tWelcome = useTranslations("Folder.chat.welcomeInputPanel")
  const tDiag = useTranslations("DiagnosticsSettings")
  const sharedT = useTranslations("Folder.chat.shared")
  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const upsertFolder = useAppWorkspaceStore((s) => s.upsertFolder)
  // Subscribe to ONLY this tab's own row (identified by `tabId`), not the whole
  // `tabs` array — so a sibling tab changing, or a tab-switch (isActive rides in
  // as a prop), never re-renders this keep-alive panel. `find` returns the same
  // object reference across derives until this tab itself changes.
  const ownTab = useTabStore(
    (s) => s.tabs.find((tab) => tab.id === tabId) ?? null
  )
  // Resolve this panel's folder from ITS OWN tab, not the global active folder.
  // A keep-alive panel for a background tab must NOT re-render when the active
  // tab switches to a different folder. For the active tab this equals the old
  // `activeFolderId` (which is itself derived from the active tab's folderId via
  // `syncActiveFolderId`); it also avoids the brief post-switch window where the
  // global `activeFolderId` still lags on the previous tab's folder (same
  // rationale as the per-tab `workingDir` used for the connection below).
  const ownFolderId = ownTab?.folderId ?? null
  const folder = useAppWorkspaceStore((s) =>
    ownFolderId != null
      ? (s.allFolders.find((f) => f.id === ownFolderId) ?? null)
      : null
  )
  const folderId = ownFolderId ?? 0
  const {
    bindConversationTab,
    setChatDraftWorkingDir,
    setTabRuntimeConversationId,
    pinTab,
    openNewConversationTab,
    closeTab,
    confirmDraftAgent,
    setDraftAgentFromFallback,
  } = useTabActions()
  const {
    appendOptimisticTurn,
    removeOptimisticTurn,
    appendViewerUserTurn,
    completeTurn,
    refetchDetail,
    syncTurnMetadata,
    removeConversation,
    setAcpLoadError,
    setDbConversationId,
    setExternalId,
    setLiveMessage,
    setPendingCleanup,
    setSyncState,
  } = useConversationRuntimeActions()
  const acpActions = useAcpActions()

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
  // Seed from localStorage so the React state reflects the user's saved
  // mode for this agent immediately on mount. Without this seed, a reuse-
  // path connect (idle window after a refresh, before the agent is GC'd)
  // would silently fall back to whatever `current_mode_id` the backend
  // happens to be on: `handleModeChange` updates only React state and
  // localStorage, not the agent — the agent gets synced inside
  // `handleSend` by diffing `modeId` against `modes.current_mode_id`.
  // A null seed here means that diff is "agent default vs null", which
  // resolves the displayed mode through `conn.modes.current_mode_id`
  // and never triggers the catch-up `setMode`.
  const [modeId, setModeId] = useState<string | null>(() =>
    getSavedModeId(agentType)
  )
  const [sendSignal, setSendSignal] = useState(0)
  const [agentsLoaded, setAgentsLoaded] = useState(false)
  const [usableAgentCount, setUsableAgentCount] = useState(0)
  const [composerDiagnosticsOpen, setComposerDiagnosticsOpen] = useState(false)
  const [agentConnectError, setAgentConnectError] = useState<string | null>(
    null
  )
  const [hasSentMessage, setHasSentMessage] = useState(false)
  const [quickActionInject, setQuickActionInject] =
    useState<ComposerInjectContent | null>(null)

  const hasPersistedConversation = dbConversationId != null

  // A folderless chat draft before its first send (chat tab, not yet persisted).
  // Used to trigger the eager scratch-dir prepare below, which gives the draft a
  // real workingDir so the ACP connection can spawn BEFORE the first send — the
  // composer is gated on `connected` like any normal conversation (no offline
  // compose). Once bound it has a persisted row + workingDir and this is false.
  const isChatDraft = useMemo(
    () => ownTab?.isChat === true && !hasPersistedConversation,
    [ownTab, hasPersistedConversation]
  )

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
  const selectedAgentRef = useRef(selectedAgent)
  const createConversationPendingRef = useRef(false)
  // Single-flight guard for the eager scratch-dir prepare (on chat-mode select).
  const prepareChatDirPendingRef = useRef(false)
  const sessionIdRef = useRef<string | null>(null)
  const syncCancelRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    dbConvIdRef.current = dbConversationId
    // Bind the DB row id onto the runtime session when the two ids diverge
    // (draft-started tab: virtual runtime key, row created on first send).
    // `refetchDetail` on the runtime key fetches with this binding — without
    // it, a settle-driven refetch (background task finished) asks the backend
    // for the virtual id and silently fails, leaving stale live turns on
    // screen forever.
    if (
      dbConversationId != null &&
      dbConversationId !== effectiveConversationId
    ) {
      setDbConversationId(effectiveConversationId, dbConversationId)
    }
  }, [dbConversationId, effectiveConversationId, setDbConversationId])

  useEffect(() => {
    selectedAgentRef.current = selectedAgent
  }, [selectedAgent])

  // Eagerly create the chat-mode scratch dir the moment this becomes an unbound
  // chat draft, so the ACP connection can spawn at a real cwd BEFORE the first
  // send — picking "no-folder mode" no longer leaves the agent unconnected.
  // Filesystem-only (writes no DB rows), so the lazy-conversation invariant
  // holds; the first send reuses this dir via createChatConversation(existingDir),
  // keeping the connection's cwd put across the bind. Single-flight and
  // self-disarming: once workingDir lands the guard flips false. openChatModeTab
  // clears workingDir on re-entry, so a fresh dir is prepared each time.
  useEffect(() => {
    if (!isActive || !isChatDraft || workingDir) return
    if (prepareChatDirPendingRef.current) return
    prepareChatDirPendingRef.current = true
    void (async () => {
      try {
        const res = await createChatDir()
        if (mountedRef.current) {
          setChatDraftWorkingDir(tabId, res.path)
        }
      } catch (e) {
        // The composer is gated on a live connection (no offline compose), and
        // the connection needs this scratch dir. If the mkdir fails the draft
        // would otherwise sit with a permanently disabled composer and no
        // explanation — surface it on the welcome screen's error banner so the
        // user can re-enter chat mode to retry.
        console.error("[ConversationTabView] prepare chat dir:", e)
        if (mountedRef.current) {
          setAgentConnectError(tWelcome("prepareSessionFailed"))
        }
      } finally {
        prepareChatDirPendingRef.current = false
      }
    })()
  }, [
    isActive,
    isChatDraft,
    workingDir,
    tabId,
    setChatDraftWorkingDir,
    tWelcome,
  ])

  // Sync the agentType prop into draftAgentType for draft tabs. The prop
  // changes when openNewConversationTab re-points an existing draft at a
  // different folder's default agent (or when any other external mutation
  // updates tab.agentType). Without this mirror, the local draftAgentType
  // would stay frozen at its mount value and the UI/connection would not
  // follow. Persisted conversations read agentType directly from the prop
  // via selectedAgent, so they are unaffected.
  useEffect(() => {
    if (conversationId != null) return
    if (agentType === selectedAgentRef.current) return
    setDraftAgentType(agentType)
    setModeId(getSavedModeId(agentType))
    setAgentConnectError(null)
  }, [agentType, conversationId])

  const {
    detail,
    loading: detailLoading,
    error: detailError,
    acpLoadError,
  } = useConversationDetail(effectiveConversationId)

  // Subscribe to only the fields this panel actually reads from its runtime
  // session — NOT the whole session object. The live-message sink rewrites the
  // session object on every streaming batch (~60/s, via SET_LIVE_MESSAGE); a
  // whole-object selector here would re-render this keep-alive panel (and the
  // composer subtree it wraps) on every streaming token, even though neither of
  // these two fields changes mid-stream. `useShallow` keeps the returned slice
  // reference-stable across batches, so the panel re-renders only when one of
  // them actually changes. (message-list-view subscribes to the session's
  // liveMessage separately to render the live stream; the context indicator
  // reads its own session stats from the runtime store directly.)
  const { externalId: runtimeExternalId, syncState: runtimeSyncState } =
    useConversationRuntimeStore(
      useShallow((s) => {
        const session = s.byConversationId.get(effectiveConversationId)
        return {
          externalId: session?.externalId ?? null,
          syncState: session?.syncState ?? "idle",
        }
      })
    )

  // Two-source resolution for the session id passed to acp_connect:
  //   1. detail.summary.external_id — DB value, available for tabs opened
  //      from the sidebar (effectiveConversationId equals the real cid).
  //   2. runtimeExternalId — populated by the connSessionId effect
  //      below when SessionStarted fires. This is the ONLY source for tabs
  //      that started as a new conversation: their effectiveConversationId
  //      is locked to a virtual negative id (line 186 useState initializer
  //      runs once), useConversationDetail skips fetching for virtual ids,
  //      and detail stays null forever. Without this fallback, every
  //      reconnect on a new-conversation tab passes sessionId=undefined →
  //      backend takes session/new → DB.external_id is overwritten on the
  //      next prompt → original sid orphaned, agent loses prior context.
  const externalId =
    detail?.summary.external_id ?? runtimeExternalId ?? undefined
  // For persisted conversations opened from the sidebar, wait until the
  // session's external_id has been resolved before auto-connecting.
  // Otherwise the auto-connect effect fires with sessionId=undefined and
  // the backend falls back to session/new, orphaning the historical
  // context. cline doesn't support session resume, so it connects
  // immediately regardless.
  const awaitingHistoricalSessionId =
    hasPersistedConversation && selectedAgent !== "cline" && detailLoading
  // Install status of the currently selected agent. An agent can be enabled and
  // platform-available yet have no CLI/SDK installed; selecting one can never
  // connect. Rather than firing a doomed (and racy) auto-connect whose only
  // outcome is a transient "not installed" toast, we skip the connect and
  // surface a persistent install prompt instead (see composerBlockedMessage).
  const { agents: acpAgents } = useAcpAgents()
  const selectedAgentNotInstalled = useMemo(() => {
    const info = acpAgents.find((a) => a.agent_type === selectedAgent)
    return (
      info != null && info.enabled && info.available && !info.installed_version
    )
  }, [acpAgents, selectedAgent])
  const canAutoConnect =
    (hasPersistedConversation || (agentsLoaded && usableAgentCount > 0)) &&
    !awaitingHistoricalSessionId &&
    // Skip the doomed auto-connect for a not-installed agent ONLY in the draft
    // surfaces, where the persistent install banner explains it instead. A
    // persisted conversation keeps its existing connect-and-surface-the-error
    // behavior (its agent can't be swapped from the picker anyway).
    !(selectedAgentNotInstalled && !hasPersistedConversation) &&
    !(hasPersistedConversation && detailError) &&
    !(hasPersistedConversation && acpLoadError)
  const draftStorageKey = useMemo(() => {
    if (dbConversationId != null) {
      return buildConversationDraftStorageKey(dbConversationId)
    }
    return buildNewConversationDraftStorageKey()
  }, [dbConversationId])
  // Use the per-tab workingDir (derived from the tab's own folderId by the
  // parent) rather than the active folder's path — otherwise switching tabs
  // briefly exposes the previous folder's path to the ACP auto-connect
  // effect, and the connection sticks with the wrong cwd.
  const workingDirForConnection = workingDir ?? folder?.path

  const {
    conn,
    modeLoading,
    configOptionsLoading,
    selectorsLoading,
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
    sessionId:
      dbConversationId != null && selectedAgent !== "cline"
        ? externalId
        : undefined,
    // Drives cross-client viewer discovery: when another client is already
    // live on this conversation, attach to its connection instead of spawning.
    conversationId: dbConversationId ?? undefined,
  })
  const { status: connStatus, sessionId: connSessionId } = conn
  const messageQueue = useMessageQueue()
  const {
    queue: msgQueue,
    enqueue: mqEnqueue,
    requeueFront: mqRequeueFront,
    getQueueLength: mqGetQueueLength,
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
  const isViewerRef = useRef(conn.isViewer)
  useEffect(() => {
    isViewerRef.current = conn.isViewer
  }, [conn.isViewer])
  const isConnecting = connStatus === "connecting"
  // The tab's connection is keyed by a stable tabId, but agent switching is
  // async — and for a not-installed target, connect()'s preflight throws BEFORE
  // it tears down the old connection. So `conn` can still describe the PREVIOUS
  // agent while `selectedAgent` has already advanced. When that's the case we
  // must NOT surface the previous agent's selectors / ready-state as the
  // selected one's: doing so showed the old agent's model + config list and
  // (worse) let a send reach the wrong agent. Reconcile everything the composer
  // reads against `selectedAgent`, falling back to that agent's own cached
  // selectors (empty until it connects).
  const connIsForOtherAgent =
    conn.agentType != null && conn.agentType !== selectedAgent
  const effectiveModes = connIsForOtherAgent
    ? (getCachedSelectors(selectedAgent)?.modes ?? null)
    : conn.modes
  const effectiveConfigOptions = connIsForOtherAgent
    ? (getCachedSelectors(selectedAgent)?.configOptions ?? null)
    : conn.configOptions
  // The live connection is ready for THIS tab only when it's connected AND its
  // cwd matches the tab's intended working dir. A just-retargeted chat draft (or
  // any mid-reconnect) can briefly read a stale "connected" for the PREVIOUS cwd;
  // sending then would deliver the prompt to the wrong agent/workspace. Every
  // direct send gates on this (handleSend), mirroring the flush effect's guard.
  // No-op for normal conversations, whose connected cwd always equals intended.
  // A connection still bound to a different agent is never "ready" for the
  // selected one — it would otherwise let a send reach the previous agent.
  const connectionReady =
    !connIsForOtherAgent &&
    isConnectionReady(
      connStatus,
      conn.connectedWorkingDir,
      workingDirForConnection
    )
  // Present "connecting" to the composer while connected-but-not-ready, so it
  // disables its send affordance instead of inviting a submit handleSend rejects.
  // While the live connection still belongs to a different agent, present the
  // selected agent's real state: "disconnected" when it isn't installed (the
  // install banner explains why), otherwise "connecting" (the switch is in
  // flight). Only ever differs from connStatus during those transient windows.
  const composerConnStatus = connIsForOtherAgent
    ? selectedAgentNotInstalled
      ? "disconnected"
      : "connecting"
    : connStatus === "connected" && !connectionReady
      ? "connecting"
      : connStatus
  const connectionModes = useMemo(
    () => effectiveModes?.available_modes ?? [],
    [effectiveModes]
  )
  const connectionConfigOptions = useMemo(
    () => effectiveConfigOptions ?? [],
    [effectiveConfigOptions]
  )
  const connectionCommands = useMemo(
    () => (connIsForOtherAgent ? [] : (conn.availableCommands ?? [])),
    [connIsForOtherAgent, conn.availableCommands]
  )
  const selectedModeId = useMemo(() => {
    if (connectionModes.length === 0) return null
    if (modeId && connectionModes.some((mode) => mode.id === modeId)) {
      return modeId
    }
    return effectiveModes?.current_mode_id ?? connectionModes[0]?.id ?? null
  }, [effectiveModes, connectionModes, modeId])

  // The single blocking message shown in the composer's inline banner (clicking
  // it opens Agent Settings). The not-installed prompt takes priority: it's the
  // actionable one and, unlike the connect-time toast, it's deterministic — it
  // appears the moment a not-installed agent is selected, independent of whether
  // a (deduped/superseded) connect attempt ever reached the preflight.
  const composerBlockedMessage = selectedAgentNotInstalled
    ? tWelcome("agentNotInstalled", { agent: AGENT_LABELS[selectedAgent] })
    : (autoConnectError ?? agentConnectError)

  useEffect(() => {
    if (connSessionId) {
      sessionIdRef.current = connSessionId
    }
  }, [connSessionId])

  // Mirror the connection's load failure (set on `session_load_failed` from
  // the agent) onto the per-conversation runtime session so the detail UI
  // can surface it next to detail-load errors. Cleared automatically when
  // the connection's loadError clears (e.g. via Reload).
  const connLoadError = conn.loadError
  useEffect(() => {
    setAcpLoadError(effectiveConversationId, connLoadError ?? null)
  }, [connLoadError, effectiveConversationId, setAcpLoadError])

  // Promote the completed turn on the prompting→idle edge. (There is no longer
  // an ordering constraint against a setLiveMessage cleanup: the liveMessage
  // sink writes the runtime store from the connection dispatch, not a React
  // effect — see registerLiveMessageSink.)
  const prevConnStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevConnStatusRef.current === "prompting"
    prevConnStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return

    // Turn completed — promote liveMessage + optimisticTurns to localTurns.
    // Don't pass conn.liveMessage: this panel no longer subscribes to it (the
    // connection snapshot is stable across streaming tokens — see useConnection),
    // so reading it here would be stale. COMPLETE_TURN falls back to
    // session.liveMessage, which the connection dispatch's sink wrote
    // synchronously as the final chunk landed (turn_complete flushes the stream
    // queue BEFORE the status change), so it already holds the final message.
    completeTurn(effectiveConversationId)

    // Cancel previous metadata sync (handles rapid consecutive turns)
    syncCancelRef.current?.()
    syncCancelRef.current = null

    const persistedId = dbConvIdRef.current
    if (persistedId && persistedId > 0) {
      syncCancelRef.current = syncTurnMetadata(
        persistedId,
        effectiveConversationId
      )
    }
  }, [completeTurn, connStatus, effectiveConversationId, syncTurnMetadata])

  // Auto-send queued messages when agent finishes responding.
  // Refs are synced via useEffect; the auto-send effect is declared
  // AFTER completeTurn so React runs it second.
  const autoSendQueueRef = useRef<() => QueuedMessage | undefined>(mqDequeue)
  useEffect(() => {
    autoSendQueueRef.current = mqDequeue
  }, [mqDequeue])
  const handleSendRef = useRef<
    (
      draft: PromptDraft,
      modeId?: string | null,
      opts?: { fromQueueFlush?: boolean }
    ) => void
  >(() => {})
  // Timestamp of the last send that bounced with TurnBusyError. The flush below
  // backs off after a bounce so repeated busy rejections (backend still running
  // another turn while this client believes it is idle) don't spin one failed
  // send per round-trip.
  const lastFlushBounceAtRef = useRef(0)

  // Flush queued messages whenever the agent is idle. This is the queue's send
  // engine, covering BOTH:
  //   - the normal case: a message queued while the agent was prompting, sent
  //     once the turn completes (prompting→connected drives syncState→idle); and
  //   - a draft re-queued by a bounced concurrent send that landed AFTER the
  //     prompting→connected transition already passed — which an edge-triggered
  //     flush would strand until the next turn.
  // Gated on syncState !== "awaiting_persist" so exactly one item flushes at a
  // time: dequeuing + sending appends an optimistic turn → awaiting_persist,
  // which blocks re-entry until that send settles (the turn completes, or it
  // bounces and rolls back to idle to retry the next item). A bounce backoff
  // rate-limits retries against a still-busy backend.
  useEffect(() => {
    if (connStatus !== "connected") return
    // Don't flush onto a connection whose cwd doesn't match the tab's intended
    // working dir. This matters for a just-bound chat conversation: bind switches
    // the tab's workingDir from the draft's previous folder to the scratch dir,
    // and for one render `connStatus` can still read the stale "connected" of the
    // old-folder session before the reconnect lands. Flushing then would deliver
    // the queued prompt to the wrong folder's agent. (No-op for normal
    // conversations, whose connection cwd always equals the intended one.)
    if (
      (conn.connectedWorkingDir ?? null) !== (workingDirForConnection ?? null)
    ) {
      return
    }
    if (runtimeSyncState === "awaiting_persist") return
    if (msgQueue.length === 0) return
    // setTimeout (not microtask) so a COMPLETE_TURN commit settles first AND so
    // a just-bounced retry waits out the backoff window before re-sending.
    const wait = flushRetryDelayMs(Date.now(), lastFlushBounceAtRef.current)
    const timer = setTimeout(() => {
      if (connStatusRef.current !== "connected") return
      const next = autoSendQueueRef.current()
      if (next) {
        // Mark this as the queue auto-flush: it sends the dequeued head now and,
        // on a bounce, returns it to the FRONT (vs a direct send → tail).
        handleSendRef.current(next.draft, next.modeId, { fromQueueFlush: true })
      }
    }, wait)
    return () => clearTimeout(timer)
  }, [
    connStatus,
    runtimeSyncState,
    msgQueue.length,
    conn.connectedWorkingDir,
    workingDirForConnection,
  ])

  // Mirror the connection's liveMessage into the runtime session OUTSIDE React.
  // The connection dispatch invokes this sink synchronously whenever liveMessage
  // changes (streaming deltas, tool updates, the prompt-start reset), so the
  // streaming content flows straight to the runtime store — which the message
  // list renders — WITHOUT this keep-alive panel re-rendering per token (the old
  // mirror effect required a per-token render just to run). The sink writes
  // non-null values with isLive = (status === "prompting"), which tells the
  // runtime reducer to bypass its stale-reconnect-replay guard (matters for the
  // rekey path: close+reopen mid-turn, where detail.turns may already hold user
  // turns that would otherwise drop the live assistant stream). Turn-end clearing
  // is owned by COMPLETE_TURN (nulls liveMessage); unmount clearing by
  // removeConversation. `tabId` is the connection contextKey.
  useEffect(() => {
    return acpActions.registerLiveMessageSink(tabId, (liveMessage, isLive) =>
      setLiveMessage(effectiveConversationId, liveMessage, isLive)
    )
  }, [acpActions, tabId, effectiveConversationId, setLiveMessage])

  // Cross-client VIEWER (Bug 2): mirror the connection's in-flight user prompt
  // (from a snapshot's `pending_user_message`, captured when we attach
  // mid-turn) into the runtime as a synthesized user turn. The reducer
  // sender-guards + dedups by id, so this is a no-op on the sender and
  // idempotent against the live `user_message` event below. This branch covers
  // the prompt that was sent BEFORE we attached; the live handler covers
  // prompts sent AFTER.
  useEffect(() => {
    const pending = conn.pendingUserMessage
    if (!pending) return
    appendViewerUserTurn(
      effectiveConversationId,
      buildUserTurnFromMessageBlocks(pending.messageId, pending.blocks)
    )
  }, [conn.pendingUserMessage, effectiveConversationId, appendViewerUserTurn])

  // Cross-client VIEWER (Bug 2): a `user_message` event for THIS connection
  // that arrives while we're attached. The owner added its user turn
  // optimistically; a viewer only receives the assistant stream, so without
  // this the reply would render with no user message above it. Sender-guarded +
  // idempotent in the reducer (the sender's own echo is a no-op).
  useAcpEvent(
    useCallback(
      (envelope: EventEnvelope) => {
        if (envelope.type !== "user_message") return
        if (envelope.connection_id !== conn.connectionId) return
        appendViewerUserTurn(
          effectiveConversationId,
          buildUserTurnFromMessageBlocks(envelope.message_id, envelope.blocks)
        )
      },
      [conn.connectionId, effectiveConversationId, appendViewerUserTurn]
    )
  )

  useEffect(() => {
    if (effectiveConversationId <= 0) return
    setExternalId(effectiveConversationId, detail?.summary.external_id ?? null)
  }, [effectiveConversationId, detail?.summary.external_id, setExternalId])

  useEffect(() => {
    if (!connSessionId) return
    setExternalId(effectiveConversationId, connSessionId)
  }, [connSessionId, effectiveConversationId, setExternalId])

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
      if (connStatusRef.current === "prompting" && !isViewerRef.current) {
        // Owner, agent still responding — keep the session for deferred cleanup
        // (the background turn_complete handler removes it once done).
        setPendingCleanup(effectiveConversationId, true)
      } else {
        // Idle owner, or a VIEWER (any status): remove immediately. A viewer's
        // unmount detaches its attach subscription, so no turn_complete will
        // arrive to resolve a deferred cleanup — deferring would leak the
        // runtime session (especially in web mode, which has no event firehose
        // after detach).
        removeConversation(effectiveConversationId)
      }
    }
  }, [effectiveConversationId, removeConversation, setPendingCleanup])

  const handleSend = useCallback(
    (
      draft: PromptDraft,
      selectedModeIdArg?: string | null,
      // `fromQueueFlush` marks the auto-flush draining the queue head — that
      // path always sends and, on a bounce, re-queues at the FRONT. A direct
      // input send (no flag) must NOT jump ahead of already-queued items: when
      // a queue exists it tail-enqueues instead of sending, and on a bounce it
      // re-queues at the TAIL.
      opts?: { fromQueueFlush?: boolean }
    ) => {
      // Capture the tab's chat-draft state + eager scratch dir synchronously,
      // before any await. A folderless chat draft is NOT special-cased here:
      // its first send takes the exact same gated, inline path as a normal new
      // conversation (the new-tab branch below just creates the row via
      // createChatConversation, reusing this eager dir). The composer is gated
      // on `connected` for chat drafts too, so by the time we get here the agent
      // is live and the prompt is delivered inline — never parked in the queue.
      const sendOwnTab = ownTab

      if (!hasPersistedConversation && !canAutoConnect) {
        setAgentConnectError(tWelcome("enableAgentFirstPlaceholder"))
        return
      }
      // Connected AND the connection's cwd matches this tab's working dir. Bare
      // `connStatus === "connected"` is not enough: a chat draft mid-reconnect can
      // read a stale "connected" for the old cwd, and an inline send then would
      // deliver to the wrong workspace. Same predicate the flush effect uses.
      if (!connectionReady) return

      const fromQueueFlush = opts?.fromQueueFlush ?? false
      // Preserve FIFO: a direct send issued while the queue is non-empty joins
      // the tail rather than racing ahead of the queued items. Read the
      // queue length synchronously (it reflects a same-tick bounce requeue).
      if (shouldQueueDirectSend(fromQueueFlush, mqGetQueueLength())) {
        mqEnqueue(draft, selectedModeIdArg ?? null)
        return
      }

      // Single-flight the unbound new-tab create. A second direct submit fired
      // before the first create resolves (a double Enter / double click) would
      // otherwise append an optimistic turn it can never deliver: the
      // createConversationPendingRef guard further down returns AFTER the
      // optimistic append. Reject the duplicate here, before any optimistic
      // mutation. Only the unbound path (no persisted id yet) is single-flighted,
      // so persisted sends keep their concurrent queued-send behavior. Applies
      // equally to chat and normal new conversations.
      if (
        shouldRejectDuplicateCreate(
          dbConvIdRef.current != null,
          createConversationPendingRef.current
        )
      ) {
        return
      }

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

      // Backend rejected the send because a turn was already in flight (another
      // co-controlling client, or a "prompting" status this client hadn't
      // observed yet). Roll back the optimistic user turn and drop the draft
      // into the queue above the input box — it auto-sends when the current
      // turn completes, identical to enqueuing while already prompting. Stamp
      // the bounce so the flush backs off instead of immediately retrying.
      const onTurnInProgress = () => {
        lastFlushBounceAtRef.current = Date.now()
        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
        // FIFO: the auto-flush draft WAS the queue head → return it to the
        // front; a direct send (queue was empty when it left) → tail.
        if (fromQueueFlush) {
          mqRequeueFront(draft, selectedModeIdArg ?? null)
        } else {
          mqEnqueue(draft, selectedModeIdArg ?? null)
        }
      }

      // Pin the tab if it was a temporary preview (single-click opened)
      if (ownTab && !ownTab.isPinned) {
        pinTab(tabId)
      }

      const persistedId = dbConvIdRef.current
      if (persistedId) {
        // Existing-tab path: row already exists, send immediately with the
        // conversation_id pinned so the backend reuses our row instead of
        // creating a duplicate.
        lifecycleSend(draft, selectedModeIdArg, {
          folderId,
          conversationId: persistedId,
          // The backend echoes this as the broadcast UserMessage's message_id,
          // so viewers' synthesized user turn dedups against our own optimistic
          // turn by exact id (and never suppresses a different sender's prompt).
          clientMessageId: optimisticTurn.id,
          onTurnInProgress,
        })
        return
      }

      // New-tab path: create the DB row first, then send with the new id
      // pinned. This prevents the backend's send_prompt_linked from racing
      // us to create its own conversation row. A folderless chat draft creates
      // via createChatConversation (reusing the eager scratch dir) and binds to
      // its hidden chat folder; every other step — the optimistic turn
      // appended above, the inline lifecycleSend, the rollback — is identical to
      // a normal new conversation. This is the whole point of the fix: after the
      // scratch dir exists, chat mode shares the normal send path and never
      // depends on the flush-on-connect queue to deliver its first prompt.
      if (createConversationPendingRef.current) return
      createConversationPendingRef.current = true
      const title = getPromptDraftDisplayText(
        draft,
        sharedT("attachedResources")
      ).slice(0, 80)
      const chatSend = sendOwnTab?.isChat === true
      const chatExistingDir = sendOwnTab?.workingDir

      void (async () => {
        try {
          let newConversationId: number
          // The send's folderId defaults to the active folder; a chat send
          // overrides it with the backend-created hidden chat folder.
          let sendFolderId = folderId
          if (chatSend) {
            const res = await createChatConversation(
              selectedAgent,
              title,
              chatExistingDir
            )
            newConversationId = res.conversationId
            sendFolderId = res.folderId
            dbConvIdRef.current = newConversationId
            setExternalId(effectiveConversationId, sessionIdRef.current ?? null)
            // Bind the DB id BEFORE the prompt goes out. The mirror effect
            // below also binds, but only after a re-render — this closes that
            // window and covers the unmounted-early return just under it.
            setDbConversationId(effectiveConversationId, newConversationId)
            if (!mountedRef.current) {
              setPendingCleanup(effectiveConversationId, true)
              refreshConversations()
              return
            }
            // Seed allFolders with the hidden chat folder so the tab's new
            // folderId resolves (cwd / active-folder) on the next render. bind
            // reuses the eager scratch dir as workingDir, so the connection's
            // cwd does not move and no reconnect is triggered.
            upsertFolder(res.folder)
            setCreatedConversationId(newConversationId)
            bindConversationTab(
              tabId,
              newConversationId,
              selectedAgent,
              title,
              effectiveConversationId,
              res.folderId,
              res.folder.path
            )
          } else {
            newConversationId = await createConversation(
              folderId,
              selectedAgent,
              title
            )
            dbConvIdRef.current = newConversationId
            // Set external ID on the stable virtual session (no migration needed —
            // effectiveConversationId never changes, so the session stays in place).
            // DB persistence of external_id is now backend-driven from
            // send_prompt_linked once the row is linked, so no explicit DB write here.
            setExternalId(effectiveConversationId, sessionIdRef.current ?? null)
            // Bind the DB id BEFORE the prompt goes out (see the chat branch).
            setDbConversationId(effectiveConversationId, newConversationId)
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
          }
          clearMessageInputDraft(buildNewConversationDraftStorageKey())
          refreshConversations()

          // Now that the row exists, kick off the actual prompt with the
          // conversation_id pinned so the backend adopts our row instead of
          // creating a duplicate one.
          lifecycleSend(draft, selectedModeIdArg, {
            folderId: sendFolderId,
            conversationId: newConversationId,
            clientMessageId: optimisticTurn.id,
            onTurnInProgress,
          })
        } catch (e) {
          console.error("[ConversationTabView] create conversation:", e)
          // A failed create (chat OR normal) must fully restore the pre-send
          // state, not strand the user behind a blank panel:
          //   1. drop the optimistic turn (no ghost stuck in awaiting_persist),
          //   2. return syncState to idle,
          //   3. setHasSentMessage(false) → re-enters welcome mode (otherwise the
          //      welcome screen never returns and the list is empty),
          //   4. re-seed the draft text — message-input clears it synchronously on
          //      send, so without this the user's prompt is lost on failure,
          //   5. surface the error on the welcome banner so it isn't silent.
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          setSyncState(effectiveConversationId, "idle")
          setHasSentMessage(false)
          const draftText = draft.displayText.trim()
          if (draftText) {
            saveMessageInputDraft(
              buildNewConversationDraftStorageKey(),
              draftText
            )
          }
          if (mountedRef.current) {
            setAgentConnectError(tWelcome("createConversationFailed"))
          }
        } finally {
          createConversationPendingRef.current = false
        }
      })()
    },
    [
      appendOptimisticTurn,
      removeOptimisticTurn,
      mqEnqueue,
      mqRequeueFront,
      mqGetQueueLength,
      bindConversationTab,
      canAutoConnect,
      connectionReady,
      effectiveConversationId,
      folderId,
      hasPersistedConversation,
      lifecycleSend,
      pinTab,
      refreshConversations,
      selectedAgent,
      setDbConversationId,
      setExternalId,
      setPendingCleanup,
      setSyncState,
      sharedT,
      ownTab,
      tWelcome,
      tabId,
      upsertFolder,
    ]
  )

  // Sync handleSend ref for auto-send effect (declared before handleSend)
  useEffect(() => {
    handleSendRef.current = handleSend
  }, [handleSend])

  const handleForkSend = useCallback(
    // Fire-and-forget: the input clears the draft synchronously on click (like a
    // normal send), so there is no in-flight editable window. If the fork can't
    // run right now — disconnected, or the queue is non-empty (a fork is an
    // immediate session side effect and must not jump ahead of queued items) —
    // the draft is NOT lost: it is queued as a normal send (it flushes after any
    // queued items). The same on a fork failure.
    async (draft: PromptDraft, selectedModeIdArg?: string | null) => {
      const connectionId = conn.connectionId
      if (
        !connectionId ||
        connStatus !== "connected" ||
        // Read the queue length SYNCHRONOUSLY so a draft re-queued by a same-
        // tick bounce is seen even before React commits. The UI also hides the
        // fork affordance while the queue is non-empty; this is the guard.
        forkSendBlockedByQueue(mqGetQueueLength())
      ) {
        mqEnqueue(draft, selectedModeIdArg ?? null)
        return
      }
      try {
        // Backend performs all DB writes in one transaction-shaped call:
        // - current row: external_id=S2, title="[Fork] ..."
        // - sibling row: created with external_id=S1, status=pending_review
        // Pass (conversationId, folderId) so a conversation opened from history
        // — whose connection resumed via session_id but isn't row-linked until
        // its first prompt — is adopted by the backend before forking (a
        // fork-send forks BEFORE that prompt). No-op once already linked. Use
        // the real persisted DB id (`dbConvIdRef`, same as the send path below),
        // NOT the runtime key `effectiveConversationId` which can be virtual.
        const { forkedSessionId } = await acpFork(
          connectionId,
          dbConvIdRef.current,
          folderId
        )
        // Update runtime session id to S2 (frontend in-memory state only)
        sessionIdRef.current = forkedSessionId
        setExternalId(effectiveConversationId, forkedSessionId)

        // NOTE: a fork is a transcript discontinuity — the row's session flips
        // S1→S2, and S2 is a COPY of S1's transcript plus the turns to come.
        // The pre-fork history is NOT re-surfaced here: the backend background
        // watcher correctly excludes the fork-copied prefix from the out-of-turn
        // overlay (see `baseline_offset_since`), so `detail.turns` (S1 parse) +
        // the new local turns render each exchange exactly once. No detail
        // refetch is needed or wanted — an early one races the forked turn and
        // can drop the just-sent message.
        refreshConversations()
        // Send the message on the forked session (S2)
        handleSend(draft, selectedModeIdArg)
      } catch (err) {
        // Busy (a turn is in flight, e.g. another co-controlling client started
        // one): NOT a fork failure — silently re-queue, like a normal bounce.
        // It sends after the current turn.
        if (err instanceof TurnBusyError) {
          mqEnqueue(draft, selectedModeIdArg ?? null)
          return
        }
        // Real fork failure: surface it. EXPLICIT product decision — fork-send
        // is best-effort, so the draft is never lost; it is re-queued and sent
        // on the current (un-forked) session.
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
        mqEnqueue(draft, selectedModeIdArg ?? null)
      }
    },
    [
      conn.connectionId,
      connStatus,
      mqGetQueueLength,
      mqEnqueue,
      effectiveConversationId,
      folderId,
      handleSend,
      refreshConversations,
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

  // Manual agent switch only updates local draft state. The single source of
  // truth for (dis)connecting is `useConnectionLifecycle`'s auto-connect
  // effect: when `selectedAgent` changes, the hook re-fires `connect()`,
  // which internally disconnects the old agent's connection at the same
  // contextKey before creating the new one (acp-connections-context.tsx).
  // Doing the disconnect+reconnect here too would race the lifecycle path:
  // a late-returning disconnect would dispatch CONNECTION_REMOVED by
  // contextKey and wipe the new connection's frontend state, leaving a
  // backend orphan.
  const handleAgentSelect = useCallback(
    (nextAgentType: AgentType) => {
      if (nextAgentType === selectedAgentRef.current) return
      if (dbConvIdRef.current) return

      setDraftAgentType(nextAgentType)
      setModeId(getSavedModeId(nextAgentType))
      setAgentConnectError(null)
      // Real user click — clear the provisional flag so TabProvider's
      // correction effect leaves this tab alone.
      confirmDraftAgent(tabId, nextAgentType)
    },
    [confirmDraftAgent, tabId]
  )

  // AgentSelector auto-fallback: the requested default agent was missing
  // or unavailable, so it picked a substitute on its own. Sync local UI
  // state (so the connection points at the right agent immediately) but
  // mark the tab as still provisional — TabProvider's correction effect
  // will re-resolve against the folder's saved default once all three
  // hydration gates are open, and overwrite this substitute if needed.
  const handleAgentFallback = useCallback(
    (nextAgentType: AgentType) => {
      if (nextAgentType === selectedAgentRef.current) return
      if (dbConvIdRef.current) return

      setDraftAgentType(nextAgentType)
      setModeId(getSavedModeId(nextAgentType))
      setAgentConnectError(null)
      setDraftAgentFromFallback(tabId, nextAgentType)
    },
    [setDraftAgentFromFallback, tabId]
  )

  const handleModeChange = useCallback(
    (newModeId: string) => {
      setModeId(newModeId)
      // Persist mode selection to localStorage immediately. Use effectiveModes
      // (reconciled to selectedAgent) rather than the raw connection modes, so a
      // mode change made during a cross-agent switch window can't save the
      // previous agent's mode shape under the selected agent.
      if (effectiveModes) {
        saveModePreference(selectedAgent, {
          ...effectiveModes,
          current_mode_id: newModeId,
        })
      }
    },
    [effectiveModes, selectedAgent]
  )

  const handleAnswerQuestion = useCallback(
    (answer: string) => {
      if (connStatus !== "connected") return
      const optimisticTurn: MessageTurn = {
        id: `optimistic-${randomUUID()}`,
        role: "user",
        blocks: [{ type: "text", text: answer }],
        timestamp: new Date().toISOString(),
      }
      const draft: PromptDraft = {
        blocks: [{ type: "text", text: answer }],
        displayText: answer,
      }
      appendOptimisticTurn(
        effectiveConversationId,
        optimisticTurn,
        optimisticTurn.id
      )
      setSendSignal((prev) => prev + 1)
      setSyncState(effectiveConversationId, "awaiting_persist")
      lifecycleSend(draft, null, {
        clientMessageId: optimisticTurn.id,
        // Rejected because a turn was already in flight — roll back the
        // optimistic turn and re-queue so it isn't stranded or lost.
        onTurnInProgress: () => {
          lastFlushBounceAtRef.current = Date.now()
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          // A direct answer (never dequeued from the queue) re-queues at the
          // TAIL — it was sent after any already-queued items, so FIFO keeps it
          // behind them. (Only the auto-flush path, whose draft WAS the head,
          // re-queues at the front.)
          mqEnqueue(draft, null)
        },
      })
    },
    [
      appendOptimisticTurn,
      removeOptimisticTurn,
      mqEnqueue,
      connStatus,
      effectiveConversationId,
      lifecycleSend,
      setSyncState,
    ]
  )

  // Answer a blocking multiple-choice `ask_user_question`. Routes straight to
  // the dedicated answer endpoint (NOT a prompt) so it resolves the parked tool
  // call; the backend broadcasts `question_resolved` to clear the card on every
  // client.
  const handleAnswerAskQuestion = useCallback(
    (questionId: string, answer: QuestionAnswer) =>
      acpActions.answerQuestion(tabId, questionId, answer),
    [acpActions, tabId]
  )

  // Queue edit flow: derive editing draft text from queue state
  const editingQueueDraftText = useMemo(() => {
    if (!mqEditingItemId) return null
    const item = msgQueue.find((m) => m.id === mqEditingItemId)
    return item?.draft.displayText ?? null
  }, [mqEditingItemId, msgQueue])

  // The editing item's full blocks, so the composer can restore inline badges +
  // attachments (not just the display text) when re-opening a queued message.
  const editingQueueDraftBlocks = useMemo(() => {
    if (!mqEditingItemId) return null
    const item = msgQueue.find((m) => m.id === mqEditingItemId)
    return item?.draft.blocks ?? null
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

  const handleQuickAction = useCallback((payload: ComposerInjectContent) => {
    setQuickActionInject(payload)
  }, [])

  const handleQuickActionConsumed = useCallback(() => {
    setQuickActionInject(null)
  }, [])

  const canShowDetailErrorActions =
    hasPersistedConversation && dbConversationId != null && !!folder
  const handleReloadDetail = useCallback(() => {
    if (dbConversationId == null) return
    // Clear the ACP load failure so canAutoConnect re-enables and the next
    // auto-connect attempt is allowed to retry session/load. The mirror
    // effect above syncs this back into the runtime session as null.
    if (acpLoadError) {
      acpActions.clearAcpLoadError(tabId)
    }
    refetchDetail(dbConversationId)
  }, [acpActions, acpLoadError, dbConversationId, refetchDetail, tabId])
  // Open (or re-activate) the singleton draft tab BEFORE closing the failing
  // tab. closeTab auto-creates a replacement draft when it removes the last
  // tab, and `openNewConversationTab` reads `rawTabsRef.current` which
  // wouldn't yet reflect either pending update if we closed first — the
  // singleton check would miss the replacement and we'd end up with two
  // drafts. Doing it in this order means the second `setTabs` (closeTab)
  // runs against the result of the first.
  const handleOpenNewSession = useCallback(() => {
    if (!folder) return
    // Retry-from-error: user wants a fresh draft in the same conversation
    // context, so inherit the active tab's agent when the folder has no
    // pinned default.
    openNewConversationTab(folder.id, workingDirForConnection ?? folder.path, {
      inheritFromActive: true,
    })
    closeTab(tabId)
  }, [closeTab, folder, openNewConversationTab, tabId, workingDirForConnection])

  const messageListNode = (
    <MessageListView
      conversationId={effectiveConversationId}
      agentType={selectedAgent}
      connStatus={connStatus}
      isActive={isActive}
      sendSignal={sendSignal}
      detailLoading={detailLoading}
      detailError={detailError}
      acpLoadError={acpLoadError}
      hideEmptyState={!hasPersistedConversation || hasSentMessage}
      onReload={canShowDetailErrorActions ? handleReloadDetail : undefined}
      onNewSession={
        canShowDetailErrorActions ? handleOpenNewSession : undefined
      }
    />
  )

  // Live-feedback bar gating + the "agent never read your note" resend fallback.
  // Enqueue rather than `handleSend`: this fallback fires on a turn-end race
  // where the backend already reports no active turn but the frontend may still
  // read `connStatus === "prompting"`, and `handleSend` no-ops unless
  // "connected" — which would silently drop the note. The message queue holds it
  // (visible above the composer) and auto-flushes when the turn completes, so
  // the user's note is never lost.
  const feedbackEnabled = useFeedbackEnabled()
  const resendFeedbackAsPrompt = useCallback(
    (text: string) => {
      mqEnqueue(
        { blocks: [{ type: "text", text }], displayText: text },
        selectedModeId
      )
    },
    [mqEnqueue, selectedModeId]
  )
  const feedback = useSessionFeedback({
    connectionId: conn.connectionId,
    connStatus,
    enabled: feedbackEnabled,
    onResendAsPrompt: resendFeedbackAsPrompt,
  })

  return (
    <ConversationShell
      topBanner={
        <>
          <SessionConfigStaleBanner contextKey={tabId} />
          <BackgroundTasksChip contextKey={tabId} />
        </>
      }
      status={connStatus}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={workingDirForConnection}
      agentName={AGENT_LABELS[selectedAgent]}
      error={conn.error}
      claudeApiRetry={conn.claudeApiRetry}
      pendingPermission={conn.pendingPermission}
      pendingQuestion={conn.pendingQuestion}
      pendingAskQuestion={conn.pendingAskQuestion}
      onFocus={handleFocus}
      onSend={handleSend}
      onCancel={handleCancel}
      onRespondPermission={handleRespondPermission}
      onAnswerQuestion={handleAnswerQuestion}
      onAnswerAskQuestion={handleAnswerAskQuestion}
      modes={connectionModes}
      configOptions={connectionConfigOptions}
      modeLoading={modeLoading}
      configOptionsLoading={configOptionsLoading}
      selectorsLoading={selectorsLoading}
      selectedModeId={selectedModeId}
      onModeChange={handleModeChange}
      onConfigOptionChange={handleSetConfigOption}
      agentType={selectedAgent}
      availableCommands={connectionCommands}
      attachmentTabId={tabId}
      draftStorageKey={draftStorageKey}
      hideInput={isWelcomeMode || Boolean(acpLoadError)}
      feedbackList={
        feedback.showList ? (
          <FeedbackNotesDisplay notes={feedback.notes} />
        ) : null
      }
      onAddFeedback={feedback.featureEnabled ? feedback.openDialog : undefined}
      feedbackAddDisabled={!feedback.canSubmit}
      isActive={isActive}
      showActiveFlow={showActiveFlow}
      queue={msgQueue}
      onEnqueue={mqEnqueue}
      onQueueReorder={mqReorder}
      onQueueEdit={handleQueueEdit}
      onQueueDelete={mqRemove}
      editingItemId={mqEditingItemId}
      editingDraftText={editingQueueDraftText}
      editingDraftBlocks={editingQueueDraftBlocks}
      isEditingQueueItem={mqEditingItemId != null}
      onSaveQueueEdit={handleSaveQueueEdit}
      onCancelQueueEdit={handleQueueCancelEdit}
      onForkSend={
        connStatus === "connected" &&
        hasPersistedConversation &&
        conn.supportsFork &&
        !forkSendBlockedByQueue(msgQueue.length)
          ? handleForkSend
          : undefined
      }
    >
      {isWelcomeMode ? (
        <div className="relative isolate flex h-full min-h-0 flex-col overflow-x-hidden overflow-y-auto">
          <div className="flex-1" />
          <div className="mx-auto flex w-full max-w-3xl shrink-0 flex-col gap-6 px-4 py-4">
            <WelcomeHero />
            <QuickActions
              onSelect={handleQuickAction}
              agentType={selectedAgent}
            />
            <div className="flex justify-center">
              <AgentSelector
                defaultAgentType={selectedAgent}
                onSelect={handleAgentSelect}
                onFallback={handleAgentFallback}
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
            </div>
            {composerBlockedMessage ? (
              <div className="flex w-full items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                <button
                  type="button"
                  onClick={handleOpenAgentsSettings}
                  title={composerBlockedMessage}
                  className="min-w-0 flex-1 cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap text-left transition-colors hover:text-destructive/80"
                >
                  {composerBlockedMessage}
                </button>
                {selectedAgentNotInstalled ? (
                  <button
                    type="button"
                    onClick={() => setComposerDiagnosticsOpen(true)}
                    className="shrink-0 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10"
                  >
                    {tDiag("button")}
                  </button>
                ) : null}
              </div>
            ) : null}
            <ChatInput
              // composerConnStatus (not connStatus): a chat draft mid-reconnect
              // reads "connecting" until the connection's cwd matches, so the
              // send affordance stays disabled until handleSend would accept it.
              status={composerConnStatus}
              promptCapabilities={conn.promptCapabilities}
              defaultPath={workingDirForConnection}
              agentName={AGENT_LABELS[selectedAgent]}
              onFocus={handleFocus}
              onSend={handleSend}
              onCancel={handleCancel}
              modes={connectionModes}
              configOptions={connectionConfigOptions}
              modeLoading={modeLoading}
              configOptionsLoading={configOptionsLoading}
              selectorsLoading={selectorsLoading}
              selectedModeId={selectedModeId}
              onModeChange={handleModeChange}
              onConfigOptionChange={handleSetConfigOption}
              agentType={selectedAgent}
              availableCommands={connectionCommands}
              attachmentTabId={tabId}
              draftStorageKey={draftStorageKey}
              isActive={isActive}
              showActiveFlow={showActiveFlow}
              onAddFeedback={
                feedback.featureEnabled ? feedback.openDialog : undefined
              }
              feedbackAddDisabled={!feedback.canSubmit}
              injectContent={quickActionInject}
              onInjectConsumed={handleQuickActionConsumed}
              flush
              tall
            />
          </div>
          <div className="flex-1" />
          <div className="mx-auto w-full max-w-3xl shrink-0 px-4 pb-6">
            <WelcomeTip />
          </div>
        </div>
      ) : showDraftHeader ? (
        <div className="flex h-full min-h-0 flex-col">
          <div className="px-4 pt-3 pb-2">
            <AgentSelector
              defaultAgentType={selectedAgent}
              onSelect={handleAgentSelect}
              onFallback={handleAgentFallback}
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
            {composerBlockedMessage ? (
              <div className="mt-2 flex w-full items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                <button
                  type="button"
                  onClick={handleOpenAgentsSettings}
                  title={composerBlockedMessage}
                  className="min-w-0 flex-1 cursor-pointer overflow-hidden text-ellipsis whitespace-nowrap text-left transition-colors hover:text-destructive/80"
                >
                  {composerBlockedMessage}
                </button>
                {selectedAgentNotInstalled ? (
                  <button
                    type="button"
                    onClick={() => setComposerDiagnosticsOpen(true)}
                    className="shrink-0 rounded border border-destructive/40 px-2 py-0.5 font-medium transition-colors hover:bg-destructive/10"
                  >
                    {tDiag("button")}
                  </button>
                ) : null}
              </div>
            ) : null}
          </div>
          <div className="min-h-0 flex-1">{messageListNode}</div>
        </div>
      ) : (
        messageListNode
      )}
      <FeedbackDialog
        open={feedback.dialogOpen}
        onOpenChange={(open) => {
          if (open) feedback.openDialog()
          else feedback.closeDialog()
        }}
        onSubmit={feedback.submit}
        submitting={feedback.submitting}
        agentName={AGENT_LABELS[selectedAgent]}
      />
      <AgentDiagnosticsDialog
        open={composerDiagnosticsOpen}
        onOpenChange={setComposerDiagnosticsOpen}
        agentType={selectedAgent}
      />
    </ConversationShell>
  )
})

export function ConversationDetailPanel() {
  const t = useTranslations("Folder.conversation")
  const tDetails = useTranslations("Folder.sessionDetails")
  const {
    completeTurn: runtimeCompleteTurn,
    removeConversation: runtimeRemoveConversation,
  } = useConversationRuntimeActions()
  const { activeFolder: folder } = useActiveFolder()
  const conversations = useAppWorkspaceStore((s) => s.conversations)
  const allFolders = useAppWorkspaceStore((s) => s.allFolders)
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const isTileMode = useTabStore((s) => s.isTileMode)
  const { openNewConversationTab, closeTab, switchTab, onPreviewTabReplaced } =
    useTabActions()
  const newConversation = useMemo(() => {
    const activeTab = tabs.find((tab) => tab.id === activeTabId)
    if (!activeTab || activeTab.conversationId != null) return null
    const workingDir = activeTab.workingDir ?? folder?.path
    if (!workingDir) return null
    return { workingDir, folderId: activeTab.folderId }
  }, [tabs, activeTabId, folder?.path])
  const { disconnect: disconnectByKey } = useAcpActions()
  const { addTask, updateTask } = useTaskContext()
  const [reloadByTabId, setReloadByTabId] = useState<Record<string, number>>({})
  const [detailsOpen, setDetailsOpen] = useState(false)

  const exportLabels = useExportLabels()

  // Disconnect the old connection immediately when a preview tab is replaced
  useEffect(() => {
    return onPreviewTabReplaced((replacedTabId) => {
      disconnectByKey(replacedTabId).catch(() => {})
    })
  }, [onPreviewTabReplaced, disconnectByKey])

  // Background turn_complete handler: for conversations not open in tabs.
  // Subscribes via the context's primary `acp://event` listener (single
  // physical Tauri/WebSocket subscription, plus seq dedup from Phase 3b).
  // `useAcpEvent` stabilizes handler identity internally, so the callback
  // can read closure values directly — no caller-side refs needed.
  useAcpEvent(
    useCallback(
      (envelope: EventEnvelope) => {
        if (envelope.type !== "turn_complete") return

        const runtimeConversationId = getConversationIdByExternalIdFromStore(
          envelope.session_id
        )
        // Event-time read: fresher than a render capture ("`conversations`
        // may lag the tab update on fast turns" below applies to the render
        // snapshot; getState() narrows that window).
        const summary = useAppWorkspaceStore
          .getState()
          .conversations.find(
            (item) => item.external_id === envelope.session_id
          )
        const matchedConversationId =
          runtimeConversationId ?? summary?.id ?? null
        if (!matchedConversationId) return

        // Match against every identifier the panel may carry for the same
        // runtime session — otherwise this background handler races the
        // panel's own completeTurn effect and double-promotes streamingTurns
        // into localTurns (visible as a duplicated assistant message until
        // the conversation is reopened from DB).
        //
        // Invariant: `tab.runtimeConversationId` is only set when the panel's
        // effectiveConversationId differs from its bound conversationId, i.e.
        // for new conversations whose session lives under a virtual (negative)
        // id. `dbId2` is always a real DB id, so a runtimeConversationId vs.
        // dbId2 comparison is unreachable and intentionally omitted.
        // `conversations` may lag the tab update on fast turns, so dbId2
        // alone (without the runtime id branch) is not a reliable signal.
        const dbId2 = summary?.id
        const isOpenInTabs = tabs.some(
          (tab) =>
            tab.conversationId === matchedConversationId ||
            tab.runtimeConversationId === matchedConversationId ||
            (dbId2 != null && tab.conversationId === dbId2)
        )
        if (isOpenInTabs) return

        // Promote liveMessage + optimisticTurns to localTurns immediately
        runtimeCompleteTurn(matchedConversationId)

        // If tab was closed while agent was responding, clean up now.
        // Event-time read: fresh via getState(), no reactive subscription.
        const session = getRuntimeSession(matchedConversationId)
        if (session?.pendingCleanup) {
          runtimeRemoveConversation(matchedConversationId)
        }
      },
      [tabs, runtimeCompleteTurn, runtimeRemoveConversation]
    )
  )

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

  const [contextMenuSelectedText, setContextMenuSelectedText] = useState("")
  const savedSelectionRangeRef = useRef<Range | null>(null)
  const isContextMenuOpenRef = useRef(false)

  const handleContextMenuOpenChange = useCallback((open: boolean) => {
    isContextMenuOpenRef.current = open
    if (!open) {
      savedSelectionRangeRef.current = null
      return
    }
    const selection = window.getSelection()
    const text = selection?.toString() ?? ""
    setContextMenuSelectedText(text)
    savedSelectionRangeRef.current =
      selection && selection.rangeCount > 0 && !selection.isCollapsed
        ? selection.getRangeAt(0).cloneRange()
        : null
  }, [])

  const handleContextMenuTriggerPointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 2) return
      const selection = window.getSelection()
      if (selection && !selection.isCollapsed) {
        event.preventDefault()
      }
    },
    []
  )

  useEffect(() => {
    const handler = () => {
      if (!isContextMenuOpenRef.current) return
      const range = savedSelectionRangeRef.current
      if (!range) return
      if (
        !document.contains(range.startContainer) ||
        !document.contains(range.endContainer)
      ) {
        savedSelectionRangeRef.current = null
        return
      }
      const selection = window.getSelection()
      if (!selection) return
      if (selection.toString().length > 0) return
      selection.removeAllRanges()
      selection.addRange(range)
    }
    document.addEventListener("selectionchange", handler)
    return () => document.removeEventListener("selectionchange", handler)
  }, [])

  const handleCopySelectedText = useCallback(async () => {
    if (!contextMenuSelectedText) return
    const ok = await copyTextFromMenu(contextMenuSelectedText)
    if (ok) {
      toast.success(t("copyTextSuccess"))
    } else {
      toast.error(t("copyTextFailed"))
    }
  }, [contextMenuSelectedText, t])

  const handleNewConversation = useCallback(() => {
    if (!folder) return
    // Right-click "new conversation" inside a conversation tab: keep the
    // active agent when the target folder has no pinned default.
    openNewConversationTab(folder.id, folder.path, { inheritFromActive: true })
  }, [folder, openNewConversationTab])

  const handleCloseActiveTab = useCallback(() => {
    if (!activeTabId) return
    closeTab(activeTabId)
  }, [activeTabId, closeTab])

  // Narrow reactive reads for the ACTIVE conversation only — a background
  // conversation's streaming token no longer re-renders this panel. `canExport`
  // keys on the tab's persisted `conversationId`; the session-details
  // resolution keys on `runtimeConversationId ?? conversationId` (a brand-new
  // conversation streams under a virtual runtime id whose live stats differ), so
  // the two are subscribed SEPARATELY — collapsing them to one lookup would
  // diverge during the virtual→persisted reconciliation window.
  const activeExportConversationId =
    activeConversationTab?.conversationId ?? null
  const canExport = useConversationRuntimeStore(
    (s) =>
      activeExportConversationId != null &&
      s.byConversationId.get(activeExportConversationId)?.detail != null
  )

  // Resolve the active conversation's summary + live token usage the same way
  // the tab view renders them — a new conversation streams under a virtual
  // `runtimeConversationId` with its usage on `sessionStats`. Extracted so the
  // resolution is unit-tested (see active-session-details.test.ts).
  const activeRuntimeId =
    activeConversationTab?.runtimeConversationId ??
    activeConversationTab?.conversationId ??
    null
  const activeRuntimeSession = useConversationRuntimeStore((s) =>
    activeRuntimeId != null
      ? (s.byConversationId.get(activeRuntimeId) ?? null)
      : null
  )
  const {
    summary: activeSessionSummary,
    stats: activeSessionStats,
    model: activeSessionModel,
  } = resolveActiveSessionDetails(
    activeConversationTab,
    // resolveActiveSessionDetails reads only `getSession(runtimeId)`, and its
    // internal `runtimeId` equals `activeRuntimeId` (identical computation), so
    // resolving that single pre-selected session is exact.
    (id) => (id === activeRuntimeId ? activeRuntimeSession : null),
    conversations
  )

  const getExportData = useCallback(() => {
    if (!activeConversationTab?.conversationId) return null
    const session = getRuntimeSession(activeConversationTab.conversationId)
    if (!session?.detail) return null
    return {
      summary: session.detail.summary,
      turns: session.detail.turns,
      sessionStats: session.detail.session_stats,
      labels: exportLabels,
    }
  }, [activeConversationTab, exportLabels])

  const handleExportMarkdown = useCallback(async () => {
    const data = getExportData()
    if (!data) return
    try {
      const result = await exportAsMarkdown(data)
      if (result === "saved") toast.success(t("exportSuccess"))
      // "cancelled": user dismissed the Save dialog — stay silent,
      // matching the downloadImage / workspace-download conventions.
    } catch (err) {
      toast.error(t("exportFailed"))
      console.error("[ConversationDetailPanel] export markdown:", err)
    }
  }, [getExportData, t])

  const handleExportHtml = useCallback(async () => {
    const data = getExportData()
    if (!data) return
    try {
      const result = await exportAsHtml(data)
      if (result === "saved") toast.success(t("exportSuccess"))
    } catch (err) {
      toast.error(t("exportFailed"))
      console.error("[ConversationDetailPanel] export html:", err)
    }
  }, [getExportData, t])

  const handleExportImage = useCallback(async () => {
    const data = getExportData()
    if (!data) return
    const taskId = `export-image-${Date.now()}`
    addTask(taskId, t("exportImage"))
    updateTask(taskId, { status: "running" })
    try {
      const result = await exportAsImage(data)
      updateTask(taskId, { status: "completed" })
      if (result === "saved") toast.success(t("exportSuccess"))
    } catch (err) {
      updateTask(taskId, { status: "failed" })
      if (err instanceof ExportTooLongError) {
        toast.error(t("exportImageTooLong"))
      } else {
        toast.error(t("exportFailed"))
      }
      console.error("[ConversationDetailPanel] export image:", err)
    }
  }, [getExportData, t, addTask, updateTask])

  // Ensure no-tab state is immediately bridged to a real new-conversation tab.
  useEffect(() => {
    if (!folder) return

    if (hasNoTabs) {
      openNewConversationTab(
        folder.id,
        newConversation?.workingDir ?? folder.path
      )
    }
  }, [folder, hasNoTabs, newConversation?.workingDir, openNewConversationTab])

  const canTile = isTileMode && tabs.length > 1

  const tileTabRefs = useRef<Map<string, HTMLDivElement | null>>(new Map())

  useEffect(() => {
    if (!canTile || !activeTabId) return
    const el = tileTabRefs.current.get(activeTabId)
    if (!el) return
    el.scrollIntoView({
      behavior: "smooth",
      inline: "center",
      block: "nearest",
    })
  }, [canTile, activeTabId])

  if (hasNoTabs) {
    return null
  }

  const tabElements = tabs.map((tab, index) => {
    const active = tab.id === activeTabId
    const folderPath = allFolders.find((f) => f.id === tab.folderId)?.path
    const view = (
      <ConversationTabView
        tabId={tab.id}
        conversationId={tab.conversationId}
        agentType={tab.agentType}
        workingDir={tab.workingDir ?? folderPath}
        isActive={active}
        showActiveFlow={canTile && active}
        reloadSignal={reloadByTabId[tab.id] ?? 0}
      />
    )
    return (
      <div
        key={tab.id}
        ref={(el) => {
          if (el) {
            tileTabRefs.current.set(tab.id, el)
          } else {
            tileTabRefs.current.delete(tab.id)
          }
        }}
        className={cn(
          canTile
            ? cn(
                "relative h-full min-w-[24rem] flex-1 overflow-hidden",
                index > 0 && "border-l border-border/50"
              )
            : active
              ? "h-full"
              : "absolute inset-0 invisible pointer-events-none"
        )}
        onPointerDownCapture={
          canTile && !active ? () => switchTab(tab.id) : undefined
        }
      >
        {/* The visible active cue is now the composer's flowing gradient border
            (see message-input.tsx); keep a non-visual cue for assistive tech in
            tiled mode, where the old top-center icon used to provide it. */}
        {canTile && active && (
          <span className="sr-only">{t("activeConversationIndicator")}</span>
        )}
        {view}
      </div>
    )
  })

  // A single header sits fixed above the horizontally-scrolling tile row, so it
  // never scrolls on the x-axis when conversations are tiled. It reflects the
  // ACTIVE conversation (title + owning folder). On mobile there's no tile row —
  // it's simply the sole conversation's header.
  const activeTab = tabs.find((tab) => tab.id === activeTabId) ?? null
  const activeTabFolder = activeTab
    ? allFolders.find((f) => f.id === activeTab.folderId)
    : undefined

  return (
    <>
      <div className="flex h-full min-h-0 flex-col overflow-hidden">
        {activeTab && (
          <ConversationDetailHeader
            tabId={activeTab.id}
            conversationId={activeTab.conversationId}
            runtimeConversationId={activeTab.runtimeConversationId ?? null}
            folderId={activeTab.folderId}
            folderPath={activeTabFolder?.path}
            title={activeTab.title}
            status={activeTab.status as ConversationStatus | undefined}
          />
        )}
        <ContextMenu onOpenChange={handleContextMenuOpenChange}>
          <ContextMenuTrigger asChild>
            <div
              className="relative min-h-0 flex-1 overflow-hidden"
              onPointerDown={handleContextMenuTriggerPointerDown}
            >
              {/* Stable wrapper across canTile flip — otherwise sibling tabs remount and a live streaming response is torn down. */}
              <TileScrollContainer canTile={canTile}>
                <div
                  className={cn(
                    "relative h-full",
                    canTile && "flex min-w-full flex-row"
                  )}
                >
                  {tabElements}
                </div>
              </TileScrollContainer>
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              disabled={!contextMenuSelectedText}
              onSelect={handleCopySelectedText}
            >
              <Copy className="h-4 w-4" />
              {t("copyText")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={!folder?.path}
              onSelect={handleNewConversation}
            >
              <SquarePen className="h-4 w-4" />
              {t("newConversation")}
            </ContextMenuItem>
            <ContextMenuSub>
              <ContextMenuSubTrigger disabled={!canExport}>
                <Download className="h-4 w-4" />
                {t("exportConversation")}
              </ContextMenuSubTrigger>
              <ContextMenuSubContent>
                <ContextMenuItem onSelect={handleExportImage}>
                  <FileImage className="h-4 w-4" />
                  {t("exportImage")}
                </ContextMenuItem>
                <ContextMenuItem onSelect={handleExportMarkdown}>
                  <FileText className="h-4 w-4" />
                  {t("exportMarkdown")}
                </ContextMenuItem>
                <ContextMenuItem onSelect={handleExportHtml}>
                  <FileCode className="h-4 w-4" />
                  {t("exportHtml")}
                </ContextMenuItem>
              </ContextMenuSubContent>
            </ContextMenuSub>
            <ContextMenuItem
              disabled={!canReloadActiveConversation}
              onSelect={handleReloadActiveConversation}
            >
              <RefreshCw className="h-4 w-4" />
              {t("reload")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!activeSessionSummary}
              onSelect={() => setDetailsOpen(true)}
            >
              <Info className="h-4 w-4" />
              {tDetails("menuLabel")}
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
      </div>
      {activeSessionSummary && (
        <SessionDetailsDialog
          open={detailsOpen}
          onOpenChange={setDetailsOpen}
          summary={activeSessionSummary}
          stats={activeSessionStats}
          model={activeSessionModel}
        />
      )}
    </>
  )
}
