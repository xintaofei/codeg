"use client"

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from "react"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import { getFolderConversation } from "@/lib/api"
import type {
  DbConversationDetail,
  MessageTurn,
  SessionStats,
  TurnUsage,
} from "@/lib/types"
import { inferLiveToolName } from "@/lib/tool-call-normalization"

export type ConversationSyncState = "idle" | "awaiting_persist"

export type ConversationTimelinePhase = "persisted" | "optimistic" | "streaming"

export interface ConversationTimelineTurn {
  key: string
  turn: MessageTurn
  phase: ConversationTimelinePhase
}

export interface ConversationRuntimeSession {
  conversationId: number
  externalId: string | null

  // DB data (cold open only)
  detail: DbConversationDetail | null
  detailLoading: boolean
  detailError: string | null

  // Active session accumulated turns (promoted optimistic + completed streaming)
  localTurns: MessageTurn[]

  // Temporary state
  optimisticTurns: MessageTurn[]
  liveMessage: LiveMessage | null

  // Sync
  syncState: ConversationSyncState
  activeTurnToken: string | null

  // Session-level stats (token usage, context window, etc.)
  sessionStats: SessionStats | null

  // Cleanup
  pendingCleanup: boolean
}

interface ConversationRuntimeState {
  byConversationId: Map<number, ConversationRuntimeSession>
  conversationIdByExternalId: Map<string, number>
}

const initialState: ConversationRuntimeState = {
  byConversationId: new Map(),
  conversationIdByExternalId: new Map(),
}

type Action =
  | {
      type: "FETCH_DETAIL_START"
      conversationId: number
    }
  | {
      type: "FETCH_DETAIL_SUCCESS"
      conversationId: number
      detail: DbConversationDetail
    }
  | {
      type: "FETCH_DETAIL_ERROR"
      conversationId: number
      error: string
    }
  | {
      type: "COMPLETE_TURN"
      conversationId: number
    }
  | {
      type: "APPEND_OPTIMISTIC_TURN"
      conversationId: number
      turn: MessageTurn
      turnToken: string
    }
  | {
      type: "SET_LIVE_MESSAGE"
      conversationId: number
      liveMessage: LiveMessage | null
    }
  | {
      type: "SET_EXTERNAL_ID"
      conversationId: number
      externalId: string | null
    }
  | {
      type: "SET_SYNC_STATE"
      conversationId: number
      syncState: ConversationSyncState
    }
  | {
      type: "MIGRATE_CONVERSATION"
      fromConversationId: number
      toConversationId: number
    }
  | {
      type: "SET_PENDING_CLEANUP"
      conversationId: number
      pendingCleanup: boolean
    }
  | {
      type: "PATCH_TURN_METADATA"
      conversationId: number
      turnPatches: Array<{
        index: number
        usage?: TurnUsage | null
        duration_ms?: number | null
        model?: string | null
      }>
      sessionStats?: SessionStats | null
    }
  | { type: "REMOVE_CONVERSATION"; conversationId: number }
  | { type: "RESET" }

function createEmptySession(
  conversationId: number
): ConversationRuntimeSession {
  return {
    conversationId,
    externalId: null,
    detail: null,
    detailLoading: false,
    detailError: null,
    localTurns: [],
    optimisticTurns: [],
    liveMessage: null,
    syncState: "idle",
    activeTurnToken: null,
    sessionStats: null,
    pendingCleanup: false,
  }
}

function formatLivePlanEntries(
  entries: Array<{ content: string; priority: string; status: string }>
): string {
  if (entries.length === 0) {
    return "Plan updated."
  }
  const lines = entries.map(
    (entry) => `- [${entry.status}] ${entry.content} (${entry.priority})`
  )
  return `Plan updated:\n${lines.join("\n")}`
}

function buildStreamingTurnsFromLiveMessage(
  conversationId: number,
  liveMessage: LiveMessage
): MessageTurn[] {
  // Split streaming content into multiple turns matching the historical
  // pattern: each "round" (text/thinking + tool calls + tool results) is a
  // separate turn. A new turn starts when a text/thinking/plan block appears
  // after completed tool calls in the current group.
  const groups: MessageTurn["blocks"][] = [[]]
  let currentGroupHasCompletedTool = false

  for (const block of liveMessage.content) {
    const isContentBlock =
      block.type === "text" ||
      block.type === "thinking" ||
      block.type === "plan"

    if (isContentBlock && currentGroupHasCompletedTool) {
      groups.push([])
      currentGroupHasCompletedTool = false
    }

    const currentBlocks = groups[groups.length - 1]

    switch (block.type) {
      case "text":
        if (block.text.length > 0) {
          currentBlocks.push({ type: "text", text: block.text })
        }
        break
      case "thinking":
        if (block.text.length > 0) {
          currentBlocks.push({ type: "thinking", text: block.text })
        }
        break
      case "plan": {
        currentBlocks.push({
          type: "thinking",
          text: formatLivePlanEntries(block.entries),
        })
        break
      }
      case "tool_call": {
        const toolName = inferLiveToolName({
          title: block.info.title,
          kind: block.info.kind,
          rawInput: block.info.raw_input,
        })
        currentBlocks.push({
          type: "tool_use",
          tool_use_id: block.info.tool_call_id,
          tool_name: toolName,
          input_preview: block.info.raw_input,
        })
        const isFinalState =
          block.info.status === "completed" || block.info.status === "failed"
        if (isFinalState) {
          currentBlocks.push({
            type: "tool_result",
            tool_use_id: block.info.tool_call_id,
            output_preview:
              block.info.raw_output_chunks.length > 0
                ? block.info.raw_output_chunks.join("")
                : block.info.content,
            is_error: block.info.status === "failed",
          })
          currentGroupHasCompletedTool = true
        }
        break
      }
    }
  }

  const timestamp = new Date(liveMessage.startedAt).toISOString()
  return groups
    .filter((blocks) => blocks.length > 0)
    .map((blocks, i) => ({
      id:
        i === 0
          ? `live-${conversationId}-${liveMessage.id}`
          : `live-${conversationId}-${liveMessage.id}-${i}`,
      role: "assistant" as const,
      blocks,
      timestamp,
    }))
}

function upsertExternalIdIndex(
  index: Map<string, number>,
  previousExternalId: string | null,
  nextExternalId: string | null,
  conversationId: number
): Map<string, number> {
  const next = new Map(index)
  if (previousExternalId) {
    next.delete(previousExternalId)
  }
  if (nextExternalId) {
    next.set(nextExternalId, conversationId)
  }
  return next
}

function updateSessionInState(
  state: ConversationRuntimeState,
  conversationId: number,
  updater: (current: ConversationRuntimeSession) => ConversationRuntimeSession
): ConversationRuntimeState {
  const current =
    state.byConversationId.get(conversationId) ??
    createEmptySession(conversationId)
  const nextSession = updater(current)
  const nextByConversationId = new Map(state.byConversationId)
  nextByConversationId.set(conversationId, nextSession)
  return { ...state, byConversationId: nextByConversationId }
}

function reducer(
  state: ConversationRuntimeState,
  action: Action
): ConversationRuntimeState {
  switch (action.type) {
    case "FETCH_DETAIL_START":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        detailLoading: true,
        detailError: null,
      }))

    case "FETCH_DETAIL_SUCCESS": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)
      const nextExternalId = action.detail.summary.external_id ?? null

      // DB data is authoritative for completed turns — always clear localTurns.
      // Only preserve optimisticTurns + liveMessage if user actively sent
      // a message and is awaiting agent response.
      const isActivelyInteracting = current.syncState === "awaiting_persist"

      const nextSession: ConversationRuntimeSession = {
        ...current,
        detail: action.detail,
        detailLoading: false,
        detailError: null,
        externalId: nextExternalId ?? current.externalId,
        localTurns: [],
        sessionStats: action.detail.session_stats ?? current.sessionStats,
        ...(isActivelyInteracting
          ? {}
          : { optimisticTurns: [], liveMessage: null }),
      }

      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      const nextExternalIndex = upsertExternalIdIndex(
        state.conversationIdByExternalId,
        current.externalId,
        nextExternalId ?? current.externalId,
        action.conversationId
      )

      return {
        byConversationId: nextByConversationId,
        conversationIdByExternalId: nextExternalIndex,
      }
    }

    case "FETCH_DETAIL_ERROR":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        detailLoading: false,
        detailError: action.error,
      }))

    case "COMPLETE_TURN": {
      const current = state.byConversationId.get(action.conversationId)
      if (!current) return state

      // Convert liveMessage to completed MessageTurns (split into rounds)
      const streamingTurns = current.liveMessage
        ? buildStreamingTurnsFromLiveMessage(
            current.conversationId,
            current.liveMessage
          )
        : []

      // Promote: optimisticTurns + streamingTurns → localTurns
      const promoted = [...current.localTurns, ...current.optimisticTurns]
      promoted.push(...streamingTurns)

      return updateSessionInState(state, action.conversationId, () => ({
        ...current,
        localTurns: promoted,
        optimisticTurns: [],
        liveMessage: null,
        syncState: "idle",
        activeTurnToken: null,
      }))
    }

    case "APPEND_OPTIMISTIC_TURN":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        optimisticTurns: [...current.optimisticTurns, action.turn],
        syncState: "awaiting_persist",
        activeTurnToken: action.turnToken,
      }))

    case "SET_LIVE_MESSAGE": {
      const current = state.byConversationId.get(action.conversationId)

      // Avoid creating a ghost session when clearing liveMessage on a deleted session
      if (!current && action.liveMessage === null) return state

      const session = current ?? createEmptySession(action.conversationId)

      // Guard: prevent stale liveMessage from ACP reconnects overriding
      // persisted data. When a session has no active liveMessage and no
      // pending interaction (idle without a live turn), a SET_LIVE_MESSAGE
      // from a reconnected ACP connection carries the completed response
      // that is already in localTurns/detail.turns.
      // Accepting it would cause duplicate assistant text in the timeline.
      // Also block during cold loading (detailLoading) — the reconnect
      // liveMessage arrives before DB data, causing overlap after fetch.
      const hasExistingTurns =
        (session.detail?.turns.length ?? 0) > 0 || session.localTurns.length > 0
      if (
        action.liveMessage !== null &&
        session.liveMessage === null &&
        session.syncState !== "awaiting_persist" &&
        (hasExistingTurns || session.detailLoading)
      ) {
        return state
      }

      return updateSessionInState(state, action.conversationId, () => ({
        ...session,
        liveMessage: action.liveMessage,
      }))
    }

    case "SET_EXTERNAL_ID": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)
      const nextSession: ConversationRuntimeSession = {
        ...current,
        externalId: action.externalId,
      }
      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      const nextExternalIndex = upsertExternalIdIndex(
        state.conversationIdByExternalId,
        current.externalId,
        action.externalId,
        action.conversationId
      )
      return {
        byConversationId: nextByConversationId,
        conversationIdByExternalId: nextExternalIndex,
      }
    }

    case "SET_SYNC_STATE":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        syncState: action.syncState,
      }))

    case "MIGRATE_CONVERSATION": {
      if (action.fromConversationId === action.toConversationId) return state
      const from = state.byConversationId.get(action.fromConversationId)
      if (!from) return state
      const to =
        state.byConversationId.get(action.toConversationId) ??
        createEmptySession(action.toConversationId)

      const mergedLiveMessage = to.liveMessage ?? from.liveMessage

      const merged: ConversationRuntimeSession = {
        ...to,
        ...from,
        conversationId: action.toConversationId,
        detail: to.detail ?? from.detail,
        detailLoading: to.detailLoading || from.detailLoading,
        detailError: to.detailError ?? from.detailError,
        localTurns: [...from.localTurns, ...to.localTurns],
        optimisticTurns: [...from.optimisticTurns, ...to.optimisticTurns],
        liveMessage: mergedLiveMessage,
        syncState: to.syncState !== "idle" ? to.syncState : from.syncState,
        activeTurnToken: to.activeTurnToken ?? from.activeTurnToken,
      }

      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.delete(action.fromConversationId)
      nextByConversationId.set(action.toConversationId, merged)

      const nextExternalIndex = new Map(state.conversationIdByExternalId)
      for (const [externalId, conversationId] of nextExternalIndex.entries()) {
        if (conversationId === action.fromConversationId) {
          nextExternalIndex.set(externalId, action.toConversationId)
        }
      }
      if (merged.externalId) {
        nextExternalIndex.set(merged.externalId, action.toConversationId)
      }

      return {
        byConversationId: nextByConversationId,
        conversationIdByExternalId: nextExternalIndex,
      }
    }

    case "PATCH_TURN_METADATA": {
      const current = state.byConversationId.get(action.conversationId)
      if (!current || current.localTurns.length === 0) return state

      const patchedTurns = [...current.localTurns]
      let changed = false
      for (const patch of action.turnPatches) {
        const turn = patchedTurns[patch.index]
        if (!turn) continue
        const newUsage = turn.usage ?? patch.usage
        const newDuration = turn.duration_ms ?? patch.duration_ms
        const newModel = turn.model ?? patch.model
        if (
          newUsage !== turn.usage ||
          newDuration !== turn.duration_ms ||
          newModel !== turn.model
        ) {
          patchedTurns[patch.index] = {
            ...turn,
            usage: newUsage,
            duration_ms: newDuration,
            model: newModel,
          }
          changed = true
        }
      }

      if (!changed && !action.sessionStats) return state

      const patchedDetail =
        current.detail && action.sessionStats
          ? { ...current.detail, session_stats: action.sessionStats }
          : current.detail

      return updateSessionInState(state, action.conversationId, () => ({
        ...current,
        localTurns: changed ? patchedTurns : current.localTurns,
        detail: patchedDetail,
        sessionStats: action.sessionStats ?? current.sessionStats,
      }))
    }

    case "SET_PENDING_CLEANUP":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        pendingCleanup: action.pendingCleanup,
      }))

    case "REMOVE_CONVERSATION": {
      const current = state.byConversationId.get(action.conversationId)
      if (!current) return state
      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.delete(action.conversationId)
      const nextExternalIndex = new Map(state.conversationIdByExternalId)
      if (current.externalId) {
        nextExternalIndex.delete(current.externalId)
      }
      return {
        byConversationId: nextByConversationId,
        conversationIdByExternalId: nextExternalIndex,
      }
    }

    case "RESET":
      return initialState
  }
}

interface ConversationRuntimeContextValue {
  getSession: (conversationId: number) => ConversationRuntimeSession | null
  getConversationIdByExternalId: (externalId: string) => number | null
  getTimelineTurns: (conversationId: number) => ConversationTimelineTurn[]
  fetchDetail: (conversationId: number) => void
  refetchDetail: (conversationId: number) => void
  completeTurn: (conversationId: number) => void
  appendOptimisticTurn: (
    conversationId: number,
    turn: MessageTurn,
    turnToken: string
  ) => void
  setLiveMessage: (
    conversationId: number,
    liveMessage: LiveMessage | null
  ) => void
  setExternalId: (conversationId: number, externalId: string | null) => void
  setSyncState: (
    conversationId: number,
    syncState: ConversationSyncState
  ) => void
  syncTurnMetadata: (
    dbConversationId: number,
    runtimeConversationId?: number
  ) => () => void
  migrateConversation: (
    fromConversationId: number,
    toConversationId: number
  ) => void
  setPendingCleanup: (conversationId: number, pendingCleanup: boolean) => void
  removeConversation: (conversationId: number) => void
  reset: () => void
}

const ConversationRuntimeContext =
  createContext<ConversationRuntimeContextValue | null>(null)

export function ConversationRuntimeProvider({
  children,
}: {
  children: ReactNode
}) {
  const [state, dispatch] = useReducer(reducer, initialState)

  const stateRef = useRef(state)
  // eslint-disable-next-line react-hooks/refs -- stateRef is only read in callbacks, not during render
  stateRef.current = state

  const getSession = useCallback(
    (conversationId: number) =>
      state.byConversationId.get(conversationId) ?? null,
    [state.byConversationId]
  )

  const getConversationIdByExternalId = useCallback(
    (externalId: string) =>
      state.conversationIdByExternalId.get(externalId) ?? null,
    [state.conversationIdByExternalId]
  )

  const getTimelineTurns = useCallback(
    (conversationId: number): ConversationTimelineTurn[] => {
      const session = state.byConversationId.get(conversationId)
      if (!session) return []

      // Phase 1: DB historical turns
      const persisted: ConversationTimelineTurn[] = (
        session.detail?.turns ?? []
      ).map((turn, index) => ({
        key: `persisted-${conversationId}-${turn.id}-${index}`,
        turn,
        phase: "persisted",
      }))

      // Phase 2: Locally completed turns (promoted optimistic + completed streaming)
      const local: ConversationTimelineTurn[] = session.localTurns.map(
        (turn, index) => ({
          key: `local-${conversationId}-${turn.id}-${index}`,
          turn,
          phase: "persisted",
        })
      )

      // Phase 3: Optimistic turns (pending user messages)
      const optimistic: ConversationTimelineTurn[] =
        session.optimisticTurns.map((turn, index) => ({
          key: `optimistic-${conversationId}-${turn.id}-${index}`,
          turn,
          phase: "optimistic",
        }))

      // Phase 4: Streaming turns (live agent response, split into rounds)
      const streamingMessage = session.liveMessage
      const streamingTurns = streamingMessage
        ? buildStreamingTurnsFromLiveMessage(conversationId, streamingMessage)
        : []

      const result = [...persisted, ...local, ...optimistic]

      for (const [i, turn] of streamingTurns.entries()) {
        result.push({
          key: `streaming-${conversationId}-${streamingMessage?.id ?? "unknown"}-${i}`,
          turn,
          phase: "streaming",
        })
      }

      return result
    },
    [state.byConversationId]
  )

  const fetchDetail = useCallback((conversationId: number) => {
    const session = stateRef.current.byConversationId.get(conversationId)
    if (session?.detail || session?.detailLoading) return

    // Skip fetch if session has active data (ongoing conversation)
    if (
      session &&
      (session.optimisticTurns.length > 0 ||
        session.liveMessage !== null ||
        session.localTurns.length > 0)
    ) {
      return
    }

    dispatch({ type: "FETCH_DETAIL_START", conversationId })
    getFolderConversation(conversationId)
      .then((detail) => {
        dispatch({ type: "FETCH_DETAIL_SUCCESS", conversationId, detail })
      })
      .catch((error: unknown) => {
        dispatch({
          type: "FETCH_DETAIL_ERROR",
          conversationId,
          error: error instanceof Error ? error.message : String(error),
        })
      })
  }, [])

  const refetchDetail = useCallback((conversationId: number) => {
    dispatch({ type: "FETCH_DETAIL_START", conversationId })
    getFolderConversation(conversationId)
      .then((detail) => {
        dispatch({ type: "FETCH_DETAIL_SUCCESS", conversationId, detail })
      })
      .catch((error: unknown) => {
        dispatch({
          type: "FETCH_DETAIL_ERROR",
          conversationId,
          error: error instanceof Error ? error.message : String(error),
        })
      })
  }, [])

  const syncTurnMetadata = useCallback(
    (
      dbConversationId: number,
      runtimeConversationId?: number
    ): (() => void) => {
      const runtimeId = runtimeConversationId ?? dbConversationId
      let cancelled = false
      let timerId: ReturnType<typeof setTimeout> | null = null

      const trySync = (attempt: number) => {
        const delay = attempt === 0 ? 1500 : 3000
        timerId = setTimeout(() => {
          if (cancelled) return
          const session = stateRef.current.byConversationId.get(runtimeId)
          if (!session || session.localTurns.length === 0) return
          if (session.syncState === "awaiting_persist") return

          getFolderConversation(dbConversationId)
            .then((parsed) => {
              if (cancelled) return
              const cur = stateRef.current.byConversationId.get(runtimeId)
              if (!cur || cur.localTurns.length === 0) return
              if (cur.syncState === "awaiting_persist") return

              const localAssistantIndices: number[] = []
              for (let i = 0; i < cur.localTurns.length; i++) {
                if (cur.localTurns[i].role === "assistant") {
                  localAssistantIndices.push(i)
                }
              }

              const parsedAssistantTurns = parsed.turns.filter(
                (t) => t.role === "assistant"
              )

              const offset =
                parsedAssistantTurns.length - localAssistantIndices.length
              const patches: Array<{
                index: number
                usage?: TurnUsage | null
                duration_ms?: number | null
                model?: string | null
              }> = []

              for (let i = 0; i < localAssistantIndices.length; i++) {
                const parsedIdx = offset + i
                if (parsedIdx < 0 || parsedIdx >= parsedAssistantTurns.length)
                  continue
                const pt = parsedAssistantTurns[parsedIdx]
                if (!pt.usage && !pt.duration_ms && !pt.model) continue
                patches.push({
                  index: localAssistantIndices[i],
                  usage: pt.usage,
                  duration_ms: pt.duration_ms,
                  model: pt.model,
                })
              }

              if (patches.length > 0 || parsed.session_stats) {
                dispatch({
                  type: "PATCH_TURN_METADATA",
                  conversationId: runtimeId,
                  turnPatches: patches,
                  sessionStats: parsed.session_stats,
                })
              }

              const latestPatch = patches[patches.length - 1]
              if (!latestPatch?.usage && attempt < 1) {
                trySync(attempt + 1)
              }
            })
            .catch(() => {
              // Silent — localTurns content remains visible
            })
        }, delay)
      }

      trySync(0)

      return () => {
        cancelled = true
        if (timerId) clearTimeout(timerId)
      }
    },
    []
  )

  const completeTurn = useCallback((conversationId: number) => {
    dispatch({ type: "COMPLETE_TURN", conversationId })
  }, [])

  const appendOptimisticTurn = useCallback(
    (conversationId: number, turn: MessageTurn, turnToken: string) => {
      dispatch({
        type: "APPEND_OPTIMISTIC_TURN",
        conversationId,
        turn,
        turnToken,
      })
    },
    []
  )

  const setLiveMessage = useCallback(
    (conversationId: number, liveMessage: LiveMessage | null) => {
      dispatch({ type: "SET_LIVE_MESSAGE", conversationId, liveMessage })
    },
    []
  )

  const setExternalId = useCallback(
    (conversationId: number, externalId: string | null) => {
      dispatch({ type: "SET_EXTERNAL_ID", conversationId, externalId })
    },
    []
  )

  const setSyncState = useCallback(
    (conversationId: number, syncState: ConversationSyncState) => {
      dispatch({ type: "SET_SYNC_STATE", conversationId, syncState })
    },
    []
  )

  const migrateConversation = useCallback(
    (fromConversationId: number, toConversationId: number) => {
      dispatch({
        type: "MIGRATE_CONVERSATION",
        fromConversationId,
        toConversationId,
      })
    },
    []
  )

  const setPendingCleanup = useCallback(
    (conversationId: number, pendingCleanup: boolean) => {
      dispatch({ type: "SET_PENDING_CLEANUP", conversationId, pendingCleanup })
    },
    []
  )

  const removeConversation = useCallback((conversationId: number) => {
    dispatch({ type: "REMOVE_CONVERSATION", conversationId })
  }, [])

  const reset = useCallback(() => {
    dispatch({ type: "RESET" })
  }, [])

  const value = useMemo<ConversationRuntimeContextValue>(
    () => ({
      getSession,
      getConversationIdByExternalId,
      getTimelineTurns,
      fetchDetail,
      refetchDetail,
      syncTurnMetadata,
      completeTurn,
      appendOptimisticTurn,
      setLiveMessage,
      setExternalId,
      setSyncState,
      migrateConversation,
      setPendingCleanup,
      removeConversation,
      reset,
    }),
    [
      getSession,
      getConversationIdByExternalId,
      getTimelineTurns,
      fetchDetail,
      refetchDetail,
      syncTurnMetadata,
      completeTurn,
      appendOptimisticTurn,
      setLiveMessage,
      setExternalId,
      setSyncState,
      migrateConversation,
      setPendingCleanup,
      removeConversation,
      reset,
    ]
  )

  return (
    <ConversationRuntimeContext.Provider value={value}>
      {children}
    </ConversationRuntimeContext.Provider>
  )
}

export function useConversationRuntime() {
  const ctx = useContext(ConversationRuntimeContext)
  if (!ctx) {
    throw new Error(
      "useConversationRuntime must be used within ConversationRuntimeProvider"
    )
  }
  return ctx
}
