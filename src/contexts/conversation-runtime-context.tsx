"use client"

import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useReducer,
  type ReactNode,
} from "react"
import type { LiveMessage } from "@/contexts/acp-connections-context"
import type { DbConversationDetail, MessageTurn } from "@/lib/types"
import { inferLiveToolName } from "@/lib/tool-call-normalization"

export type ConversationSyncState =
  | "idle"
  | "awaiting_persist"
  | "reconciling"
  | "failed"

export type ConversationTimelinePhase = "persisted" | "optimistic" | "streaming"

export interface ConversationTimelineTurn {
  key: string
  turn: MessageTurn
  phase: ConversationTimelinePhase
}

export interface ConversationRuntimeSession {
  conversationId: number
  externalId: string | null
  persistedTurns: MessageTurn[]
  optimisticTurns: MessageTurn[]
  liveMessage: LiveMessage | null
  syncState: ConversationSyncState
  activeTurnToken: string | null
  lastHydratedAt: number | null
  lastPersistedAt: number | null
  persistedUpdatedAt: string | null
  persistedMessageCount: number
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
  | { type: "HYDRATE_FROM_DETAIL"; detail: DbConversationDetail }
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
      type: "ACK_PERSISTED_DETAIL"
      conversationId: number
      detail: DbConversationDetail
      turnToken?: string | null
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
  | { type: "REMOVE_CONVERSATION"; conversationId: number }
  | { type: "RESET" }

function createEmptySession(
  conversationId: number
): ConversationRuntimeSession {
  return {
    conversationId,
    externalId: null,
    persistedTurns: [],
    optimisticTurns: [],
    liveMessage: null,
    syncState: "idle",
    activeTurnToken: null,
    lastHydratedAt: null,
    lastPersistedAt: null,
    persistedUpdatedAt: null,
    persistedMessageCount: 0,
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

function buildStreamingTurnFromLiveMessage(
  conversationId: number,
  liveMessage: LiveMessage
): MessageTurn | null {
  const blocks: MessageTurn["blocks"] = []

  for (const block of liveMessage.content) {
    switch (block.type) {
      case "text":
        if (block.text.length > 0) {
          blocks.push({ type: "text", text: block.text })
        }
        break
      case "thinking":
        if (block.text.length > 0) {
          blocks.push({ type: "thinking", text: block.text })
        }
        break
      case "plan": {
        blocks.push({
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
        blocks.push({
          type: "tool_use",
          tool_use_id: block.info.tool_call_id,
          tool_name: toolName,
          input_preview: block.info.raw_input,
        })
        const isFinalState =
          block.info.status === "completed" || block.info.status === "failed"
        if (isFinalState) {
          blocks.push({
            type: "tool_result",
            tool_use_id: block.info.tool_call_id,
            output_preview: block.info.raw_output ?? block.info.content,
            is_error: block.info.status === "failed",
          })
        }
        break
      }
    }
  }

  if (blocks.length === 0) return null

  return {
    id: `live-${conversationId}-${liveMessage.id}`,
    role: "assistant",
    blocks,
    timestamp: new Date(liveMessage.startedAt).toISOString(),
  }
}

function shouldAcceptPersistedSnapshot(
  current: ConversationRuntimeSession | undefined,
  detail: DbConversationDetail
): boolean {
  if (!current) return true

  const nextUpdatedAt = detail.summary.updated_at ?? null
  const nextMessageCount = detail.summary.message_count
  const nextTurnCount = detail.turns.length

  if (nextMessageCount < current.persistedMessageCount) return false
  if (nextTurnCount < current.persistedTurns.length) return false
  if (!current.persistedUpdatedAt || !nextUpdatedAt) return true
  if (nextUpdatedAt < current.persistedUpdatedAt) return false

  return true
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

function reduceHydrateDetail(
  state: ConversationRuntimeState,
  conversationId: number,
  detail: DbConversationDetail
): ConversationRuntimeState {
  const current = state.byConversationId.get(conversationId)
  const nextExternalId = detail.summary.external_id ?? null
  const acceptSnapshot = shouldAcceptPersistedSnapshot(current, detail)
  const prevPersistedTurnCount = current?.persistedTurns.length ?? 0
  const prevPersistedMessageCount = current?.persistedMessageCount ?? 0
  const prevPersistedUpdatedAt = current?.persistedUpdatedAt ?? null
  const optimisticTurns = current?.optimisticTurns ?? []
  const persistedTurns = acceptSnapshot
    ? detail.turns
    : (current?.persistedTurns ?? [])
  const nextPersistedUpdatedAt = acceptSnapshot
    ? (detail.summary.updated_at ?? null)
    : (current?.persistedUpdatedAt ?? null)
  const nextPersistedMessageCount = acceptSnapshot
    ? detail.summary.message_count
    : (current?.persistedMessageCount ?? 0)
  const shouldDropOptimistic =
    optimisticTurns.length > 0 &&
    persistedTurns.length >= (current?.persistedTurns.length ?? 0) + 1
  const nextUpdatedAt = detail.summary.updated_at ?? null
  const hasPersistedAdvance =
    acceptSnapshot &&
    (detail.turns.length > prevPersistedTurnCount ||
      detail.summary.message_count > prevPersistedMessageCount ||
      (nextUpdatedAt !== null &&
        (prevPersistedUpdatedAt === null ||
          nextUpdatedAt > prevPersistedUpdatedAt)))

  const nextSession: ConversationRuntimeSession = {
    ...(current ?? createEmptySession(conversationId)),
    externalId: nextExternalId,
    persistedTurns,
    liveMessage:
      hasPersistedAdvance && current?.syncState !== "awaiting_persist"
        ? null
        : (current?.liveMessage ?? null),
    optimisticTurns: shouldDropOptimistic ? [] : optimisticTurns,
    syncState: shouldDropOptimistic ? "idle" : (current?.syncState ?? "idle"),
    activeTurnToken: shouldDropOptimistic
      ? null
      : (current?.activeTurnToken ?? null),
    lastHydratedAt: Date.now(),
    lastPersistedAt: acceptSnapshot
      ? Date.now()
      : (current?.lastPersistedAt ?? null),
    persistedUpdatedAt: nextPersistedUpdatedAt,
    persistedMessageCount: nextPersistedMessageCount,
  }

  const nextByConversationId = new Map(state.byConversationId)
  nextByConversationId.set(conversationId, nextSession)
  const nextExternalIndex = upsertExternalIdIndex(
    state.conversationIdByExternalId,
    current?.externalId ?? null,
    nextExternalId,
    conversationId
  )

  return {
    byConversationId: nextByConversationId,
    conversationIdByExternalId: nextExternalIndex,
  }
}

function reducer(
  state: ConversationRuntimeState,
  action: Action
): ConversationRuntimeState {
  switch (action.type) {
    case "HYDRATE_FROM_DETAIL":
      return reduceHydrateDetail(state, action.detail.summary.id, action.detail)

    case "APPEND_OPTIMISTIC_TURN": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)
      const nextSession: ConversationRuntimeSession = {
        ...current,
        optimisticTurns: [...current.optimisticTurns, action.turn],
        syncState: "awaiting_persist",
        activeTurnToken: action.turnToken,
      }
      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      return { ...state, byConversationId: nextByConversationId }
    }

    case "SET_LIVE_MESSAGE": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)

      // Guard: prevent stale liveMessage from ACP reconnects overriding
      // persisted data. When a session has no active liveMessage and no
      // pending interaction (idle or reconciling without a live turn),
      // a SET_LIVE_MESSAGE from a reconnected ACP connection carries
      // the completed response that is already in persistedTurns.
      // Accepting it would cause duplicate assistant text in the timeline.
      if (
        action.liveMessage !== null &&
        current.liveMessage === null &&
        current.syncState !== "awaiting_persist" &&
        current.persistedTurns.length > 0
      ) {
        return state
      }

      const nextSession: ConversationRuntimeSession = {
        ...current,
        liveMessage: action.liveMessage,
      }
      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      return { ...state, byConversationId: nextByConversationId }
    }

    case "ACK_PERSISTED_DETAIL": {
      const nextState = reduceHydrateDetail(
        state,
        action.conversationId,
        action.detail
      )
      const session = nextState.byConversationId.get(action.conversationId)
      if (!session) return nextState
      const nextSession: ConversationRuntimeSession = {
        ...session,
        syncState: "idle",
        activeTurnToken:
          action.turnToken != null &&
          action.turnToken === session.activeTurnToken
            ? null
            : session.activeTurnToken,
      }
      const nextByConversationId = new Map(nextState.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      return { ...nextState, byConversationId: nextByConversationId }
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

    case "SET_SYNC_STATE": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)
      const nextSession: ConversationRuntimeSession = {
        ...current,
        syncState: action.syncState,
      }
      const nextByConversationId = new Map(state.byConversationId)
      nextByConversationId.set(action.conversationId, nextSession)
      return { ...state, byConversationId: nextByConversationId }
    }

    case "MIGRATE_CONVERSATION": {
      if (action.fromConversationId === action.toConversationId) return state
      const from = state.byConversationId.get(action.fromConversationId)
      if (!from) return state
      const to =
        state.byConversationId.get(action.toConversationId) ??
        createEmptySession(action.toConversationId)

      const preferFromSnapshot =
        from.persistedTurns.length >= to.persistedTurns.length
      const mergedLiveMessage = to.liveMessage ?? from.liveMessage

      const merged: ConversationRuntimeSession = {
        ...to,
        ...from,
        conversationId: action.toConversationId,
        persistedTurns: preferFromSnapshot
          ? from.persistedTurns
          : to.persistedTurns,
        optimisticTurns: [...from.optimisticTurns, ...to.optimisticTurns],
        liveMessage: mergedLiveMessage,
        syncState: to.syncState !== "idle" ? to.syncState : from.syncState,
        activeTurnToken: to.activeTurnToken ?? from.activeTurnToken,
        lastHydratedAt:
          Math.max(from.lastHydratedAt ?? 0, to.lastHydratedAt ?? 0) || null,
        lastPersistedAt:
          Math.max(from.lastPersistedAt ?? 0, to.lastPersistedAt ?? 0) || null,
        persistedUpdatedAt:
          (to.persistedUpdatedAt ?? "") > (from.persistedUpdatedAt ?? "")
            ? to.persistedUpdatedAt
            : from.persistedUpdatedAt,
        persistedMessageCount: Math.max(
          from.persistedMessageCount,
          to.persistedMessageCount
        ),
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
  hydrateFromDetail: (detail: DbConversationDetail) => void
  appendOptimisticTurn: (
    conversationId: number,
    turn: MessageTurn,
    turnToken: string
  ) => void
  setLiveMessage: (
    conversationId: number,
    liveMessage: LiveMessage | null
  ) => void
  acknowledgePersistedDetail: (
    conversationId: number,
    detail: DbConversationDetail,
    turnToken?: string | null
  ) => void
  setExternalId: (conversationId: number, externalId: string | null) => void
  setSyncState: (
    conversationId: number,
    syncState: ConversationSyncState
  ) => void
  migrateConversation: (
    fromConversationId: number,
    toConversationId: number
  ) => void
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

      const persisted: ConversationTimelineTurn[] = session.persistedTurns.map(
        (turn, index) => ({
          key: `persisted-${conversationId}-${turn.id}-${index}`,
          turn,
          phase: "persisted",
        })
      )
      const optimistic: ConversationTimelineTurn[] =
        session.optimisticTurns.map((turn, index) => ({
          key: `optimistic-${conversationId}-${turn.id}-${index}`,
          turn,
          phase: "optimistic",
        }))
      const streamingMessage = session.liveMessage
      const streamingTurn = streamingMessage
        ? buildStreamingTurnFromLiveMessage(conversationId, streamingMessage)
        : null

      if (!streamingTurn) {
        return [...persisted, ...optimistic]
      }

      return [
        ...persisted,
        ...optimistic,
        {
          key: `streaming-${conversationId}-${streamingMessage?.id ?? "unknown"}`,
          turn: streamingTurn,
          phase: "streaming",
        },
      ]
    },
    [state.byConversationId]
  )

  const hydrateFromDetail = useCallback((detail: DbConversationDetail) => {
    dispatch({ type: "HYDRATE_FROM_DETAIL", detail })
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

  const acknowledgePersistedDetail = useCallback(
    (
      conversationId: number,
      detail: DbConversationDetail,
      turnToken?: string | null
    ) => {
      dispatch({
        type: "ACK_PERSISTED_DETAIL",
        conversationId,
        detail,
        turnToken,
      })
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
      hydrateFromDetail,
      appendOptimisticTurn,
      setLiveMessage,
      acknowledgePersistedDetail,
      setExternalId,
      setSyncState,
      migrateConversation,
      removeConversation,
      reset,
    }),
    [
      getSession,
      getConversationIdByExternalId,
      getTimelineTurns,
      hydrateFromDetail,
      appendOptimisticTurn,
      setLiveMessage,
      acknowledgePersistedDetail,
      setExternalId,
      setSyncState,
      migrateConversation,
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
