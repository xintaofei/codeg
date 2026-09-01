"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { AgentSelector } from "@/components/chat/agent-selector"
import { ConversationShell } from "@/components/chat/conversation-shell"
import type { ConversationFolderPickerOverride } from "@/components/chat/conversation-context-bar"
import { MessageListView } from "@/components/message/message-list-view"
import { useAcpActions } from "@/contexts/acp-connections-context"
import { useConnectionLifecycle } from "@/hooks/use-connection-lifecycle"
import { useConversationDetail } from "@/hooks/use-conversation-detail"
import {
  createChatConversation,
  createConversation,
  createChatDir,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { getAgentLabel } from "@/lib/custom-agents"
import {
  extractUserImagesFromDraft,
  getPromptDraftDisplayText,
} from "@/lib/prompt-draft"
import {
  getSavedModeId,
  saveModePreference,
} from "@/lib/selector-prefs-storage"
import type {
  AgentType,
  ContentBlock,
  MessageTurn,
  PlanApprovalAnswer,
  PromptDraft,
  QuestionAnswer,
} from "@/lib/types"
import { cn, randomUUID } from "@/lib/utils"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import {
  useConversationRuntimeActions,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import { useShallow } from "zustand/react/shallow"

/**
 * A live conversation rendered INSIDE a canvas card: transcript, composer,
 * streaming reply, permission / question / plan-approval prompts, and the
 * mode & config selectors.
 *
 * It is deliberately NOT `ConversationTabView` (conversation-detail-panel.tsx),
 * which is welded to the tab store — it resolves its folder from its own tab
 * row, binds drafts through `bindConversationTab`, and pins/closes tabs. A
 * canvas card has no tab. Instead this composes the SAME primitives that view
 * is built from, so the behaviour that matters is shared code rather than a
 * copy:
 *
 *   `useConnectionLifecycle`  connect / send / cancel / respond-permission
 *   `useConversationDetail`   the persisted transcript
 *   `registerLiveMessageSink` streaming deltas → the runtime store
 *   `MessageListView`         the transcript (reads turns by conversation id)
 *   `ConversationShell`       composer + the three interrupt dialogs
 *
 * What it deliberately leaves out: the message queue, fork-send, live-feedback
 * steering, transcript export and the welcome-page quick actions. Those belong
 * to the full workspace surface, which the card's "open in workspace" menu
 * entry is one click away from.
 *
 * `contextKey` is the ACP connection key and must be STABLE for the card's
 * lifetime; when this conversation is also open in a workspace tab, the two
 * keys differ and the second one attaches to the first's connection as a
 * co-controlling viewer (see the discovery gate in `acp-connections-context`).
 */

/** Where an unsent draft card will create its conversation. */
export type CanvasDraftTarget =
  | { kind: "folder"; folderId: number; workingDir: string }
  | { kind: "chat" }

interface CanvasConversationSurfaceProps {
  contextKey: string
  /** Persisted conversation, or `null` for a draft that hasn't been sent yet. */
  conversationId: number | null
  agentType: AgentType
  /** Draft cards let the user switch agents before the first send. */
  onAgentTypeChange?: (agentType: AgentType) => void
  /** Draft cards only — where the conversation will be created. */
  draftTarget?: CanvasDraftTarget
  /**
   * Fired once the draft's first send created the row, so the canvas can
   * persist the card as a real `kind=conversation` node and drop the draft.
   */
  onConversationCreated?: (conversationId: number, folderId: number) => void
  /**
   * True while the first send is minting the conversation row. The draft card
   * hides its discard button for that window — the row and the prompt are
   * already in flight and cannot be recalled.
   */
  onCreatingChange?: (creating: boolean) => void
  /** Keep a text selection from this card's transcript as a note on the board.
   *  Omitted where the caller has no geometry to place one (draft cards), and
   *  the bubble then simply doesn't offer it. MUST be referentially stable. */
  onSaveSelectionAsNote?: (text: string) => void
  /**
   * Identity + switching for the folder chip under the composer. A canvas card
   * has no tab, and without this the chip resolves the workspace's ACTIVE tab
   * instead — every card showed that tab's folder and switching moved that tab.
   */
  folderPickerOverride?: ConversationFolderPickerOverride
  /** Focus/priority hint for the connection lifecycle and the transcript. */
  isActive?: boolean
  className?: string
}

/** Stable negative runtime key for a draft (mirrors `ConversationTabView`'s
 *  `buildVirtualConversationId`): the runtime session must exist before the row
 *  does, and it must not move when the row arrives. */
function buildVirtualConversationId(seed: string): number {
  let hash = 0
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) | 0
  }
  return -(Math.abs(hash) + 1)
}

function buildOptimisticUserTurn(draft: PromptDraft, fallback: string) {
  const text = getPromptDraftDisplayText(draft, fallback)
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
  } satisfies MessageTurn
}

export function CanvasConversationSurface({
  contextKey,
  conversationId,
  agentType,
  onAgentTypeChange,
  draftTarget,
  onConversationCreated,
  onCreatingChange,
  onSaveSelectionAsNote,
  folderPickerOverride,
  isActive = true,
  className,
}: CanvasConversationSurfaceProps) {
  const t = useTranslations("Canvas")
  const sharedT = useTranslations("Folder.chat.shared")
  const acpActions = useAcpActions()
  const {
    appendOptimisticTurn,
    removeOptimisticTurn,
    completeTurn,
    setDbConversationId,
    setExternalId,
    setLiveMessage,
    setSyncState,
    syncTurnMetadata,
  } = useConversationRuntimeActions()
  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const upsertFolder = useAppWorkspaceStore((s) => s.upsertFolder)

  // Fixed at mount, like the tab view's: a draft streams under a virtual id and
  // must keep it after the real row lands, or the live turn loses its session.
  const [effectiveConversationId] = useState(
    () => conversationId ?? buildVirtualConversationId(contextKey)
  )
  const [createdConversationId, setCreatedConversationId] = useState<
    number | null
  >(null)
  const dbConversationId = conversationId ?? createdConversationId
  const dbConversationIdRef = useRef(dbConversationId)
  useEffect(() => {
    dbConversationIdRef.current = dbConversationId
  }, [dbConversationId])

  const [modeId, setModeId] = useState<string | null>(() =>
    getSavedModeId(agentType)
  )
  const [sendSignal, setSendSignal] = useState(0)
  const [createError, setCreateError] = useState<string | null>(null)
  const creatingRef = useRef(false)
  // Mirrors `creatingRef` as state, purely so the card can refuse to be thrown
  // away mid-creation: the row is already being written and the prompt is
  // already on its way, so a discard here would leave a conversation nobody can
  // see and an agent nobody asked to run.
  const [creating, setCreating] = useState(false)
  useEffect(() => {
    onCreatingChange?.(creating)
  }, [creating, onCreatingChange])
  // Draft cards mint their chat scratch dir once, so the ACP cwd never moves
  // between the eager connect and the first send.
  const chatDirRef = useRef<string | null>(null)
  const [chatDir, setChatDir] = useState<string | null>(null)

  const {
    detail,
    loading: detailLoading,
    error: detailError,
    acpLoadError,
  } = useConversationDetail(effectiveConversationId)

  const { externalId: runtimeExternalId } = useConversationRuntimeStore(
    useShallow((s) => ({
      externalId: s.byConversationId.get(effectiveConversationId)?.externalId,
    }))
  )
  const externalId =
    detail?.summary.external_id ?? runtimeExternalId ?? undefined

  // A persisted conversation must not connect before its stored session id has
  // arrived. `sessionId: undefined` makes the backend take `session/new`, and
  // the next prompt overwrites `external_id` with that fresh session — the
  // history the card is displaying is orphaned to an agent nobody can reach
  // again. The tab view gates on the same condition; a canvas card needs it
  // MORE, because a dormant card's detail fetch is still in flight at the
  // moment the user's first touch flips it live. cline can't resume a session
  // at all, so it never waits. A failed fetch waits forever rather than
  // guessing: the error is already on screen with a retry.
  const awaitingHistoricalSessionId =
    dbConversationId != null &&
    agentType !== "cline" &&
    (detailLoading || detailError != null || acpLoadError != null) &&
    externalId == null

  const summary = useAppWorkspaceStore((s) =>
    dbConversationId != null
      ? (s.conversations.find((c) => c.id === dbConversationId) ?? null)
      : null
  )
  const boundFolderId = summary?.folder_id ?? null
  const folderPath = useAppWorkspaceStore((s) => {
    const id =
      boundFolderId ??
      (draftTarget?.kind === "folder" ? draftTarget.folderId : null)
    return id != null
      ? (s.allFolders.find((f) => f.id === id)?.path ?? null)
      : null
  })

  const workingDir =
    folderPath ??
    (draftTarget?.kind === "folder" ? draftTarget.workingDir : null) ??
    chatDir ??
    undefined

  // A folderless draft needs its scratch dir before the connection can be
  // established at all — mint it once, eagerly. Gated on `isActive` because a
  // draft card restored from a previous visit is dormant: minting there would
  // leave a fresh scratch directory on disk on every single canvas visit, for a
  // card the user may never touch again.
  useEffect(() => {
    if (!isActive) return
    if (draftTarget?.kind !== "chat") return
    if (chatDirRef.current != null) return
    let cancelled = false
    void (async () => {
      try {
        const dir = await createChatDir()
        if (cancelled) return
        chatDirRef.current = dir.path
        setChatDir(dir.path)
      } catch (e) {
        console.error("[canvas] prepare chat dir:", e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [draftTarget?.kind, isActive])

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
    contextKey,
    agentType,
    // Without a cwd there is nothing to connect to yet (a chat draft before its
    // scratch dir lands); auto-connect would fire with an undefined dir.
    isActive: isActive && workingDir != null && !awaitingHistoricalSessionId,
    workingDir,
    sessionId:
      dbConversationId != null && agentType !== "cline"
        ? externalId
        : undefined,
    // Cross-surface viewer attach: when this conversation is already live on
    // another surface (a workspace tab, another window), join that connection
    // instead of spawning a second agent for the same session.
    conversationId: dbConversationId ?? undefined,
  })
  const connStatus = conn.status
  const connSessionId = conn.sessionId

  // Streaming deltas → runtime store, keyed by the CONNECTION and written into
  // this card's runtime session. Registered per contextKey, so a workspace tab
  // showing the same conversation keeps its own sink; both write the same data.
  useEffect(() => {
    return acpActions.registerLiveMessageSink(
      contextKey,
      (liveMessage, isLive) =>
        setLiveMessage(effectiveConversationId, liveMessage, isLive)
    )
  }, [acpActions, contextKey, effectiveConversationId, setLiveMessage])

  // Mirror the resolved session id into the runtime store so a reconnect
  // resumes the same agent session instead of falling back to session/new.
  useEffect(() => {
    if (!connSessionId) return
    setExternalId(effectiveConversationId, connSessionId)
  }, [connSessionId, effectiveConversationId, setExternalId])

  // Promote the finished turn on the prompting → idle edge, then refresh the
  // per-turn metadata (token counts, duration) the summary row shows.
  const prevStatusRef = useRef(connStatus)
  const syncCancelRef = useRef<(() => void) | null>(null)
  useEffect(() => {
    const wasPrompting = prevStatusRef.current === "prompting"
    prevStatusRef.current = connStatus
    if (!wasPrompting || connStatus === "prompting") return
    completeTurn(effectiveConversationId)
    syncCancelRef.current?.()
    syncCancelRef.current = null
    const persistedId = dbConversationIdRef.current
    if (persistedId && persistedId > 0) {
      syncCancelRef.current = syncTurnMetadata(
        persistedId,
        effectiveConversationId
      )
    }
  }, [completeTurn, connStatus, effectiveConversationId, syncTurnMetadata])
  useEffect(() => () => syncCancelRef.current?.(), [])

  const handleModeChange = useCallback(
    (nextModeId: string) => {
      setModeId(nextModeId)
      // The preference stores the whole mode SHAPE, not just the id, so it can
      // be shipped to the backend at connect time.
      if (conn.modes) {
        saveModePreference(agentType, {
          ...conn.modes,
          current_mode_id: nextModeId,
        })
      }
    },
    [agentType, conn.modes]
  )

  const handleSend = useCallback(
    (draft: PromptDraft, selectedModeIdArg?: string | null) => {
      // Bail BEFORE the optimistic turn exists. A second submit while the first
      // one is still minting the conversation row cannot be sent (there is no
      // row to send it against yet), and painting it into the transcript first
      // would leave the user looking at a message that was never delivered and
      // never rolled back.
      const alreadyPersisted = dbConversationIdRef.current != null
      if (!alreadyPersisted && (creatingRef.current || !draftTarget)) return

      const optimisticTurn = buildOptimisticUserTurn(
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
      // No message queue on this surface: a send the backend bounces (a turn
      // already in flight) rolls back and surfaces as a toast rather than
      // silently parking the draft somewhere the card doesn't render.
      const onSendFailed = () => {
        removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
      }

      const persistedId = dbConversationIdRef.current
      if (persistedId) {
        lifecycleSend(draft, selectedModeIdArg, {
          folderId: boundFolderId ?? undefined,
          conversationId: persistedId,
          clientMessageId: optimisticTurn.id,
          onTurnInProgress: onSendFailed,
          onSendFailed,
        })
        return
      }

      // Draft: create the row BEFORE the prompt goes out, so the backend adopts
      // it instead of minting a duplicate (same ordering as the tab view). The
      // guard for this branch already ran above, before the optimistic turn.
      if (!draftTarget) return
      creatingRef.current = true
      setCreating(true)
      setCreateError(null)
      void (async () => {
        try {
          const title = getPromptDraftDisplayText(
            draft,
            sharedT("attachedResources")
          )
            .trim()
            .slice(0, 80)
          let newId: number
          let sendFolderId: number
          if (draftTarget.kind === "chat") {
            const res = await createChatConversation(
              agentType,
              title || undefined,
              chatDirRef.current ?? undefined
            )
            newId = res.conversationId
            sendFolderId = res.folderId
            // Seed the hidden chat folder so the card's cwd resolves on the
            // next render without waiting for a workspace refresh.
            upsertFolder(res.folder)
          } else {
            newId = await createConversation(
              draftTarget.folderId,
              agentType,
              title || undefined
            )
            sendFolderId = draftTarget.folderId
          }
          dbConversationIdRef.current = newId
          setExternalId(effectiveConversationId, connSessionId ?? null)
          setDbConversationId(effectiveConversationId, newId)
          setCreatedConversationId(newId)
          refreshConversations()
          onConversationCreated?.(newId, sendFolderId)
          lifecycleSend(draft, selectedModeIdArg, {
            folderId: sendFolderId,
            conversationId: newId,
            clientMessageId: optimisticTurn.id,
            onTurnInProgress: onSendFailed,
            onSendFailed,
          })
        } catch (e) {
          console.error("[canvas] create conversation:", e)
          // Restore the pre-send state whole: no ghost turn stuck in
          // awaiting_persist, and the error visible rather than silent.
          removeOptimisticTurn(effectiveConversationId, optimisticTurn.id)
          setSyncState(effectiveConversationId, "idle")
          setCreateError(toErrorMessage(e))
          toast.error(t("createFailed"))
        } finally {
          creatingRef.current = false
          setCreating(false)
        }
      })()
    },
    [
      agentType,
      appendOptimisticTurn,
      boundFolderId,
      connSessionId,
      draftTarget,
      effectiveConversationId,
      lifecycleSend,
      onConversationCreated,
      refreshConversations,
      removeOptimisticTurn,
      setDbConversationId,
      setExternalId,
      setSyncState,
      sharedT,
      t,
      upsertFolder,
    ]
  )

  const handleAnswerQuestion = useCallback(
    (answer: string) => {
      if (connStatus !== "connected") return
      handleSend({
        blocks: [{ type: "text", text: answer }],
        displayText: answer,
      })
    },
    [connStatus, handleSend]
  )

  const handleAnswerAskQuestion = useCallback(
    (questionId: string, answer: QuestionAnswer) =>
      acpActions.answerQuestion(contextKey, questionId, answer),
    [acpActions, contextKey]
  )

  const handleAnswerPlanApproval = useCallback(
    (approvalId: string, answer: PlanApprovalAnswer) =>
      acpActions.answerPlanApproval(contextKey, approvalId, answer),
    [acpActions, contextKey]
  )

  const connectionModes = useMemo(
    () => conn.modes?.available_modes ?? [],
    [conn.modes]
  )
  const connectionConfigOptions = useMemo(
    () => conn.configOptions ?? [],
    [conn.configOptions]
  )
  const selectedModeId = useMemo(() => {
    if (connectionModes.length === 0) return null
    if (modeId && connectionModes.some((mode) => mode.id === modeId)) {
      return modeId
    }
    return conn.modes?.current_mode_id ?? connectionModes[0]?.id ?? null
  }, [conn.modes, connectionModes, modeId])

  const isDraft = dbConversationId == null

  return (
    <div className={cn("flex h-full min-h-0 w-full flex-col", className)}>
      {/* `nowheel` keeps the transcript's own scroll from being eaten by the
          canvas zoom. Dragging is already confined to the card's title bar by
          the node's `dragHandle`, so the body needs no `nodrag` — and must not
          have one, or it would be one more thing to remember for every child
          added here later. */}
      <div className="nowheel flex min-h-0 flex-1 flex-col">
        <ConversationShell
          status={connStatus}
          promptCapabilities={conn.promptCapabilities}
          defaultPath={workingDir}
          agentName={getAgentLabel(agentType)}
          error={conn.error ?? autoConnectError ?? createError}
          claudeApiRetry={conn.claudeApiRetry}
          sessionFailures={conn.sessionFailures}
          pendingPermission={conn.pendingPermission}
          pendingQuestion={conn.pendingQuestion}
          pendingAskQuestion={conn.pendingAskQuestion}
          pendingPlanApproval={conn.pendingPlanApproval}
          onFocus={handleFocus}
          onSend={handleSend}
          onCancel={handleCancel}
          onRespondPermission={handleRespondPermission}
          onAnswerQuestion={handleAnswerQuestion}
          onAnswerAskQuestion={handleAnswerAskQuestion}
          onAnswerPlanApproval={handleAnswerPlanApproval}
          modes={connectionModes}
          configOptions={connectionConfigOptions}
          modeLoading={modeLoading}
          configOptionsLoading={configOptionsLoading}
          selectorsLoading={selectorsLoading}
          selectedModeId={selectedModeId}
          onModeChange={handleModeChange}
          onConfigOptionChange={handleSetConfigOption}
          agentType={agentType}
          availableCommands={conn.availableCommands ?? []}
          draftStorageKey={`canvas-draft:${contextKey}`}
          // The card's own connection key doubles as its composer scope: the
          // context-usage ring and connection dot in the picker row read it as
          // a contextKey (they showed nothing at all while it was undefined).
          attachmentTabId={contextKey}
          folderPickerOverride={folderPickerOverride}
          isActive={isActive}
        >
          {isDraft && onAgentTypeChange && (
            <div className="flex shrink-0 justify-center border-b border-border/60 px-3 py-2">
              <AgentSelector
                align="center"
                defaultAgentType={agentType}
                onSelect={onAgentTypeChange}
                onFallback={onAgentTypeChange}
                disabled={conn.status === "connecting"}
              />
            </div>
          )}
          <MessageListView
            conversationId={effectiveConversationId}
            agentType={agentType}
            connStatus={connStatus}
            isActive={isActive}
            sendSignal={sendSignal}
            detailLoading={detailLoading}
            detailError={detailError}
            acpLoadError={acpLoadError}
            hideEmptyState={isDraft}
            showMessageNav={false}
            onSaveNoteSelection={onSaveSelectionAsNote}
          />
        </ConversationShell>
      </div>
    </div>
  )
}
