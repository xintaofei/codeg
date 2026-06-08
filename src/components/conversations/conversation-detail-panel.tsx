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
  Focus,
  Plus,
  RefreshCw,
  X,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useAcpActions, useAcpEvent } from "@/contexts/acp-connections-context"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import { useSessionStats } from "@/contexts/session-stats-context"
import { useTaskContext } from "@/contexts/task-context"
import { cn, copyTextFromMenu, randomUUID } from "@/lib/utils"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { useMessageQueue, type QueuedMessage } from "@/hooks/use-message-queue"
import { MessageListView } from "@/components/message/message-list-view"
import { ConversationShell } from "@/components/chat/conversation-shell"
import { FeedbackNotesDisplay } from "@/components/chat/feedback-notes-display"
import { FeedbackDialog } from "@/components/chat/feedback-dialog"
import { useFeedbackEnabled } from "@/hooks/use-feedback-enabled"
import { useSessionFeedback } from "@/hooks/use-session-feedback"
import { AgentSelector } from "@/components/chat/agent-selector"
import { ChatInput } from "@/components/chat/chat-input"
import { WelcomeHero, WelcomeTip } from "@/components/chat/welcome-hero"
import { ScrollArea } from "@/components/ui/scroll-area"
import { acpFork, createConversation, openSettingsWindow } from "@/lib/api"
import {
  flushRetryDelayMs,
  forkSendBlockedByQueue,
  shouldQueueDirectSend,
} from "@/lib/queue-flush"
import { TurnBusyError } from "@/lib/turn-busy"
import { useConversationRuntime } from "@/contexts/conversation-runtime-context"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  extractUserImagesFromDraft,
  extractUserResourcesFromDraft,
  getPromptDraftDisplayText,
} from "@/lib/prompt-draft"
import {
  AGENT_LABELS,
  type AgentType,
  type ContentBlock,
  type EventEnvelope,
  type MessageTurn,
  type PromptDraft,
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
  type ExportLabels,
} from "@/lib/export-conversation"

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
  reloadSignal,
}: ConversationTabViewProps) {
  const t = useTranslations("Folder.conversation")
  const tWelcome = useTranslations("Folder.chat.welcomeInputPanel")
  const sharedT = useTranslations("Folder.chat.shared")
  const { activeFolder: folder, activeFolderId } = useActiveFolder()
  const { refreshConversations } = useAppWorkspace()
  const folderId = activeFolderId ?? 0
  const {
    tabs,
    bindConversationTab,
    setTabRuntimeConversationId,
    pinTab,
    openNewConversationTab,
    closeTab,
    confirmDraftAgent,
    setDraftAgentFromFallback,
  } = useTabContext()
  const { setSessionStats } = useSessionStats()
  const {
    appendOptimisticTurn,
    removeOptimisticTurn,
    appendViewerUserTurn,
    completeTurn,
    getSession,
    refetchDetail,
    syncTurnMetadata,
    removeConversation,
    setAcpLoadError,
    setExternalId,
    setLiveMessage,
    setPendingCleanup,
    setSyncState,
  } = useConversationRuntime()
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
  const [agentConnectError, setAgentConnectError] = useState<string | null>(
    null
  )
  const [hasSentMessage, setHasSentMessage] = useState(false)

  const hasPersistedConversation = dbConversationId != null

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
  const sessionIdRef = useRef<string | null>(null)
  const syncCancelRef = useRef<(() => void) | null>(null)

  useEffect(() => {
    dbConvIdRef.current = dbConversationId
  }, [dbConversationId])

  useEffect(() => {
    selectedAgentRef.current = selectedAgent
  }, [selectedAgent])

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

  const runtimeSession = getSession(effectiveConversationId)
  const effectiveSessionStats = runtimeSession?.sessionStats ?? null

  useEffect(() => {
    if (!isActive) return
    setSessionStats(effectiveSessionStats)
  }, [effectiveSessionStats, isActive, setSessionStats])

  // Two-source resolution for the session id passed to acp_connect:
  //   1. detail.summary.external_id — DB value, available for tabs opened
  //      from the sidebar (effectiveConversationId equals the real cid).
  //   2. runtimeSession.externalId — populated by the connSessionId effect
  //      below when SessionStarted fires. This is the ONLY source for tabs
  //      that started as a new conversation: their effectiveConversationId
  //      is locked to a virtual negative id (line 186 useState initializer
  //      runs once), useConversationDetail skips fetching for virtual ids,
  //      and detail stays null forever. Without this fallback, every
  //      reconnect on a new-conversation tab passes sessionId=undefined →
  //      backend takes session/new → DB.external_id is overwritten on the
  //      next prompt → original sid orphaned, agent loses prior context.
  const externalId =
    detail?.summary.external_id ?? runtimeSession?.externalId ?? undefined
  // For persisted conversations opened from the sidebar, wait until the
  // session's external_id has been resolved before auto-connecting.
  // Otherwise the auto-connect effect fires with sessionId=undefined and
  // the backend falls back to session/new, orphaning the historical
  // context. cline doesn't support session resume, so it connects
  // immediately regardless.
  const awaitingHistoricalSessionId =
    hasPersistedConversation && selectedAgent !== "cline" && detailLoading
  const canAutoConnect =
    (hasPersistedConversation || (agentsLoaded && usableAgentCount > 0)) &&
    !awaitingHistoricalSessionId &&
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

  // Mirror the connection's load failure (set on `session_load_failed` from
  // the agent) onto the per-conversation runtime session so the detail UI
  // can surface it next to detail-load errors. Cleared automatically when
  // the connection's loadError clears (e.g. via Reload).
  const connLoadError = conn.loadError
  useEffect(() => {
    setAcpLoadError(effectiveConversationId, connLoadError ?? null)
  }, [connLoadError, effectiveConversationId, setAcpLoadError])

  // completeTurn MUST be declared BEFORE setLiveMessage so that React runs
  // its cleanup/setup before setLiveMessage's cleanup. When connStatus
  // transitions away from "prompting", completeTurn snapshots and promotes
  // the liveMessage first, then setLiveMessage's cleanup safely clears it.
  const prevConnStatusRef = useRef(connStatus)
  useEffect(() => {
    const wasPrompting = prevConnStatusRef.current === "prompting"
    prevConnStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return

    // Turn completed — promote liveMessage + optimisticTurns to localTurns.
    // Pass conn.liveMessage explicitly: when turn_complete arrives in the
    // same React batch as the final STREAM_BATCH (typical case), the mirror
    // effect that syncs conn.liveMessage into the runtime session has not
    // run yet for this render, so session.liveMessage would be missing the
    // final text chunk. The connections-context value is authoritative.
    completeTurn(effectiveConversationId, conn.liveMessage)

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
  }, [
    completeTurn,
    connStatus,
    conn.liveMessage,
    effectiveConversationId,
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
  const runtimeSyncState = runtimeSession?.syncState ?? "idle"
  useEffect(() => {
    if (connStatus !== "connected") return
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
  }, [connStatus, runtimeSyncState, msgQueue.length])

  useEffect(() => {
    // Only sync non-null liveMessage updates to state. When conn.liveMessage
    // goes null (agent finished streaming), don't clear state.liveMessage —
    // COMPLETE_TURN needs to snapshot it when connStatus transitions.
    // Clearing is handled by COMPLETE_TURN (sets liveMessage = null) and
    // by this effect's cleanup (when not prompting).
    if (conn.liveMessage != null) {
      // isLive=true when actively prompting tells the runtime reducer to
      // bypass its stale-reconnect-replay guard. This matters for the
      // rekey path (close+reopen mid-turn): the runtime session for the
      // persisted conversation id is fresh and may have user turns in
      // detail.turns post-load, which would otherwise drop the live
      // assistant stream on the floor.
      setLiveMessage(
        effectiveConversationId,
        conn.liveMessage,
        connStatus === "prompting"
      )
    }
    return () => {
      // Don't clear liveMessage if agent is still responding — the session
      // is kept via pendingCleanup, and clearing here would cause the
      // SET_LIVE_MESSAGE guard to block the reconnect liveMessage on reopen.
      if (connStatusRef.current !== "prompting") {
        setLiveMessage(effectiveConversationId, null)
      }
    }
  }, [conn.liveMessage, connStatus, effectiveConversationId, setLiveMessage])

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
      if (!hasPersistedConversation && !canAutoConnect) {
        setAgentConnectError(tWelcome("enableAgentFirstPlaceholder"))
        return
      }
      if (connStatus !== "connected") return

      const fromQueueFlush = opts?.fromQueueFlush ?? false
      // Preserve FIFO: a direct send issued while the queue is non-empty joins
      // the tail rather than racing ahead of the queued items. Read the
      // queue length synchronously (it reflects a same-tick bounce requeue).
      if (shouldQueueDirectSend(fromQueueFlush, mqGetQueueLength())) {
        mqEnqueue(draft, selectedModeIdArg ?? null)
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
      const currentTab = tabs.find((tab) => tab.id === tabId)
      if (currentTab && !currentTab.isPinned) {
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
      // us to create its own conversation row.
      if (createConversationPendingRef.current) return
      createConversationPendingRef.current = true
      const title = getPromptDraftDisplayText(
        draft,
        sharedT("attachedResources")
      ).slice(0, 80)

      void (async () => {
        try {
          const newConversationId = await createConversation(
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
          clearMessageInputDraft(buildNewConversationDraftStorageKey())
          refreshConversations()

          // Now that the row exists, kick off the actual prompt with the
          // conversation_id pinned so the backend adopts our row instead of
          // creating a duplicate one.
          lifecycleSend(draft, selectedModeIdArg, {
            folderId,
            conversationId: newConversationId,
            clientMessageId: optimisticTurn.id,
            onTurnInProgress,
          })
        } catch (e) {
          console.error("[ConversationTabView] create conversation:", e)
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
      connStatus,
      effectiveConversationId,
      folderId,
      hasPersistedConversation,
      lifecycleSend,
      pinTab,
      refreshConversations,
      selectedAgent,
      setExternalId,
      setPendingCleanup,
      setSyncState,
      sharedT,
      tabs,
      tWelcome,
      tabId,
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
        const { forkedSessionId } = await acpFork(connectionId)
        // Update runtime session id to S2 (frontend in-memory state only)
        sessionIdRef.current = forkedSessionId
        setExternalId(effectiveConversationId, forkedSessionId)

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
      // Persist mode selection to localStorage immediately
      if (conn.modes) {
        saveModePreference(selectedAgent, {
          ...conn.modes,
          current_mode_id: newModeId,
        })
      }
    },
    [conn.modes, selectedAgent]
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
      sessionStats={effectiveSessionStats}
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
      status={connStatus}
      promptCapabilities={conn.promptCapabilities}
      defaultPath={workingDirForConnection}
      agentName={AGENT_LABELS[selectedAgent]}
      error={conn.error}
      claudeApiRetry={conn.claudeApiRetry}
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
        conn.supportsFork &&
        !forkSendBlockedByQueue(msgQueue.length)
          ? handleForkSend
          : undefined
      }
    >
      {isWelcomeMode ? (
        <div className="relative isolate flex h-full min-h-0 flex-col overflow-x-hidden overflow-y-auto">
          <div className="flex-1" />
          <div className="mx-auto flex w-full max-w-2xl shrink-0 flex-col gap-6 px-4 py-4">
            <WelcomeHero />
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
              onAddFeedback={
                feedback.featureEnabled ? feedback.openDialog : undefined
              }
              feedbackAddDisabled={!feedback.canSubmit}
            />
          </div>
          <div className="flex-1" />
          <div className="mx-auto w-full max-w-2xl shrink-0 px-4 pb-6">
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
    </ConversationShell>
  )
})

export function ConversationDetailPanel() {
  const t = useTranslations("Folder.conversation")
  const tStatus = useTranslations("Folder.statusLabels")
  const tExport = useTranslations("Folder.conversation.exportLabels")
  const {
    completeTurn: runtimeCompleteTurn,
    getConversationIdByExternalId,
    getSession,
    removeConversation: runtimeRemoveConversation,
  } = useConversationRuntime()
  const { activeFolder: folder } = useActiveFolder()
  const { conversations, getFolder } = useAppWorkspace()
  const {
    tabs,
    activeTabId,
    isTileMode,
    openNewConversationTab,
    closeTab,
    switchTab,
    onPreviewTabReplaced,
  } = useTabContext()
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

  const exportLabels = useMemo<ExportLabels>(
    () => ({
      untitledConversation: tExport("untitledConversation"),
      agent: tExport("agent"),
      model: tExport("model"),
      status: tExport("status"),
      started: tExport("started"),
      updated: tExport("updated"),
      tokens: tExport("tokens"),
      duration: tExport("duration"),
      inputTokens: tExport("inputTokens"),
      outputTokens: tExport("outputTokens"),
      cacheRead: tExport("cacheRead"),
      cacheWrite: tExport("cacheWrite"),
      user: tExport("user"),
      assistant: tExport("assistant"),
      system: tExport("system"),
      toolResult: tExport("toolResult"),
      toolError: tExport("toolError"),
      statusLabels: {
        in_progress: tStatus("in_progress"),
        pending_review: tStatus("pending_review"),
        completed: tStatus("completed"),
        cancelled: tStatus("cancelled"),
      },
    }),
    [tExport, tStatus]
  )

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

        const runtimeConversationId = getConversationIdByExternalId(
          envelope.session_id
        )
        const summary = conversations.find(
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

        // If tab was closed while agent was responding, clean up now
        const session = getSession(matchedConversationId)
        if (session?.pendingCleanup) {
          runtimeRemoveConversation(matchedConversationId)
        }
      },
      [
        conversations,
        tabs,
        getConversationIdByExternalId,
        getSession,
        runtimeCompleteTurn,
        runtimeRemoveConversation,
      ]
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

  const canExport =
    activeConversationTab?.conversationId != null &&
    getSession(activeConversationTab.conversationId)?.detail != null

  const getExportData = useCallback(() => {
    if (!activeConversationTab?.conversationId) return null
    const session = getSession(activeConversationTab.conversationId)
    if (!session?.detail) return null
    return {
      summary: session.detail.summary,
      turns: session.detail.turns,
      sessionStats: session.detail.session_stats,
      labels: exportLabels,
    }
  }, [activeConversationTab, getSession, exportLabels])

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
                index > 0 && "border-l border-border"
              )
            : active
              ? "h-full"
              : "absolute inset-0 invisible pointer-events-none"
        )}
        onPointerDownCapture={
          canTile && !active ? () => switchTab(tab.id) : undefined
        }
      >
        {canTile && active && (
          <div
            role="img"
            aria-label={t("activeConversationIndicator")}
            title={t("activeConversationIndicator")}
            className="absolute top-2 left-1/2 z-20 flex h-6 w-6 -translate-x-1/2 items-center justify-center rounded-full bg-background/40 text-sidebar-primary shadow-sm ring-1 ring-sidebar-primary/20 backdrop-blur"
          >
            <Focus className="h-4 w-4" />
          </div>
        )}
        <ConversationTabView
          tabId={tab.id}
          conversationId={tab.conversationId}
          agentType={tab.agentType}
          workingDir={tab.workingDir ?? getFolder(tab.folderId)?.path}
          isActive={active}
          reloadSignal={reloadByTabId[tab.id] ?? 0}
        />
      </div>
    )
  })

  return (
    <ContextMenu onOpenChange={handleContextMenuOpenChange}>
      <ContextMenuTrigger asChild>
        <div
          className="relative h-full min-h-0 overflow-hidden"
          onPointerDown={handleContextMenuTriggerPointerDown}
        >
          {/* Stable wrapper across canTile flip — otherwise sibling tabs remount and a live streaming response is torn down. */}
          <ScrollArea
            x={canTile ? "scroll" : "hidden"}
            y="hidden"
            className="h-full w-full"
          >
            <div
              className={cn(
                "relative h-full",
                canTile && "flex min-w-full flex-row"
              )}
            >
              {tabElements}
            </div>
          </ScrollArea>
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
          <Plus className="h-4 w-4" />
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
