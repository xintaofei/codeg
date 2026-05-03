"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type ReactNode,
} from "react"
import type {
  LiveMessage,
  ToolCallInfo,
} from "@/contexts/acp-connections-context"
import { getFolderConversation } from "@/lib/api"
import {
  clearRecoverableOptimisticTurns,
  loadRecoverableOptimisticTurns,
  saveRecoverableOptimisticTurns,
} from "@/lib/conversation-recovery-storage"
import type {
  AgentExecutionStats,
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
  // Tool call IDs whose results are still streaming (only set for streaming-phase items).
  // The adapter uses this to keep the tool in "running" state while exposing partial output.
  inProgressToolCallIds?: Set<string>
}

export interface ConversationRuntimeSession {
  conversationId: number
  externalId: string | null
  recoveryConversationId: number | null

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
      /**
       * When true, bypass the stale-reconnect-replay guard. The caller has
       * verified that the source connection is currently producing this
       * liveMessage (e.g. status === "prompting"), so the content is fresh
       * rather than a post-completion replay. Required for the rekey path
       * (close+reopen mid-turn): the runtime session for the persisted
       * conversation id is brand-new, has no liveMessage, and may already
       * see the user turn in `detail.turns` once cold-load resolves —
       * which would otherwise trigger the guard and drop the live
       * assistant content.
       */
      isLive?: boolean
    }
  | {
      type: "SET_EXTERNAL_ID"
      conversationId: number
      externalId: string | null
    }
  | {
      type: "SET_RECOVERY_CONVERSATION_ID"
      conversationId: number
      recoveryConversationId: number | null
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
  conversationId: number,
  recoveryConversationId: number | null = conversationId > 0
    ? conversationId
    : null
): ConversationRuntimeSession {
  const recoveredLocalTurns =
    recoveryConversationId != null
      ? loadRecoverableOptimisticTurns(recoveryConversationId)
      : []

  return {
    conversationId,
    externalId: null,
    recoveryConversationId,
    detail: null,
    detailLoading: false,
    detailError: null,
    localTurns: recoveredLocalTurns,
    optimisticTurns: [],
    liveMessage: null,
    syncState: "idle",
    activeTurnToken: null,
    sessionStats: null,
    pendingCleanup: false,
  }
}

function getTurnBlocksSignature(turn: MessageTurn): string {
  try {
    return JSON.stringify(turn.blocks)
  } catch {
    return `blocks:${turn.blocks.length}`
  }
}

function getTimestampMs(timestamp: string): number | null {
  const value = Date.parse(timestamp)
  return Number.isFinite(value) ? value : null
}

function looksLikeOptimisticAnchorId(
  anchorId: string | null | undefined
): boolean {
  return typeof anchorId === "string" && anchorId.startsWith("optimistic:")
}

function countUserTurns(turns: MessageTurn[]): number {
  return turns.filter((turn) => turn.role === "user").length
}

function reconcileOptimisticTurns(
  optimisticTurns: MessageTurn[],
  knownCompletedTurns: MessageTurn[],
  persistedTurns: MessageTurn[]
): MessageTurn[] {
  if (optimisticTurns.length === 0 || persistedTurns.length === 0) {
    return optimisticTurns
  }

  const persistedUsers = persistedTurns
    .filter((turn) => {
      return (
        turn.role === "user" && !looksLikeOptimisticAnchorId(turn.anchor_id)
      )
    })
    .map((turn) => ({
      signature: getTurnBlocksSignature(turn),
      timestampMs: getTimestampMs(turn.timestamp),
    }))

  const baselineKnownUserCount = countUserTurns(knownCompletedTurns)
  const optimisticUserCount = countUserTurns(optimisticTurns)
  const appendedPersistedUsers =
    baselineKnownUserCount === 0 && optimisticUserCount > 0
      ? persistedUsers.slice(-optimisticUserCount)
      : persistedUsers.slice(baselineKnownUserCount)

  if (appendedPersistedUsers.length === 0) return optimisticTurns

  let nextCandidateIndex = 0

  return optimisticTurns.filter((turn) => {
    if (turn.role !== "user") return true

    const candidate = appendedPersistedUsers[nextCandidateIndex]
    if (!candidate) return true

    const optimisticSignature = getTurnBlocksSignature(turn)
    if (candidate.signature !== optimisticSignature) {
      return true
    }

    const optimisticTimestampMs = getTimestampMs(turn.timestamp)
    const distance =
      candidate.timestampMs !== null && optimisticTimestampMs !== null
        ? Math.abs(candidate.timestampMs - optimisticTimestampMs)
        : 0

    if (distance > 5 * 60 * 1000) {
      return true
    }

    nextCandidateIndex += 1
    return false
  })
}

function isRecoverableUserTurn(turn: MessageTurn): boolean {
  return turn.role === "user" && looksLikeOptimisticAnchorId(turn.anchor_id)
}

function dedupeRecoverableTurns(turns: MessageTurn[]): MessageTurn[] {
  const seen = new Set<string>()
  return turns.filter((turn) => {
    const key = `${turn.id}:${turn.timestamp}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

function collectRecoverableUserTurns(
  session: ConversationRuntimeSession
): MessageTurn[] {
  return dedupeRecoverableTurns([
    ...session.optimisticTurns.filter(isRecoverableUserTurn),
    ...session.localTurns.filter(isRecoverableUserTurn),
  ])
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

interface BuiltStreamingTurns {
  turns: MessageTurn[]
  inProgressToolCallIds: Set<string>
}

// Cache joined chunk output keyed by chunks-array identity. The ACP reducer
// creates a new chunks array only when streaming output actually changes, so
// a WeakMap keyed on the array reference lets repeated renders reuse the
// joined string without re-running O(n) concatenation.
const joinedOutputCache = new WeakMap<readonly string[], string>()

function getJoinedChunks(chunks: readonly string[]): string {
  if (chunks.length === 0) return ""
  if (chunks.length === 1) return chunks[0]
  const cached = joinedOutputCache.get(chunks)
  if (cached !== undefined) return cached
  const joined = chunks.join("")
  joinedOutputCache.set(chunks, joined)
  return joined
}

/**
 * Clean raw Agent tool output that may be JSON or XML wrapped.
 *
 * Streaming Agent results often arrive as raw JSON (e.g. content block
 * arrays from Claude Code, or status wrappers from Codex) or with
 * `<task_result>` XML tags (OpenCode). This function extracts the human-
 * readable text so the Agent card displays clean output.
 */
function cleanAgentOutput(output: string | null): string | null {
  if (!output) return null
  let text = output.trim()
  if (!text) return null

  // Step 1: Unwrap JSON containers (no recursion — single-level unwrap)
  // JSON array of content blocks: [{"type":"text","text":"..."},...]
  if (text.startsWith("[")) {
    try {
      const arr = JSON.parse(text)
      if (Array.isArray(arr)) {
        const texts: string[] = []
        for (const item of arr) {
          if (
            item &&
            typeof item === "object" &&
            typeof item.text === "string"
          ) {
            texts.push(item.text)
          }
        }
        if (texts.length > 0) text = texts.join("\n")
      }
    } catch {
      /* not valid JSON */
    }
  } else if (text.startsWith("{")) {
    // JSON object with common result fields
    try {
      const obj = JSON.parse(text) as Record<string, unknown>
      for (const key of ["result", "output", "text", "content", "completed"]) {
        if (typeof obj[key] === "string") {
          text = (obj[key] as string).trim()
          break
        }
      }
    } catch {
      /* not valid JSON */
    }
  }

  // Step 2: Strip leading session / task_id lines that some agents prepend
  // before the actual result (e.g. "task_id: ses_xxx (for resuming ...)").
  text = text.replace(/^task_id:\s*\S+[^\n]*\n+/, "").trim()
  if (!text) return null

  // Step 3: Extract from <task_result> XML wrapper (OpenCode)
  const tagStart = text.indexOf("<task_result>")
  if (tagStart !== -1) {
    const contentStart = tagStart + "<task_result>".length
    const contentEnd = text.indexOf("</task_result>", contentStart)
    const extracted = text
      .substring(contentStart, contentEnd === -1 ? undefined : contentEnd)
      .trim()
    if (extracted) return extracted
  }

  return text
}

function buildStreamingTurnsFromLiveMessage(
  conversationId: number,
  liveMessage: LiveMessage
): BuiltStreamingTurns {
  // ── Phase 1: Identify agent → child relationships ──────────────────
  // Uses meta.claudeCode.parentToolUseId when available (precise), with
  // position-based fallback for agents that don't provide it.
  const agentChildren = new Map<
    string,
    Array<{ info: ToolCallInfo; toolName: string }>
  >()
  const childToolCallIds = new Set<string>()

  // Cache inferred tool names — inferLiveToolName is called per tool_call
  // in both Phase 1 and Phase 2; caching avoids redundant computation.
  const inferredNames = new Map<string, string>()
  const getToolName = (info: ToolCallInfo): string => {
    const cached = inferredNames.get(info.tool_call_id)
    if (cached !== undefined) return cached
    const name = inferLiveToolName({
      title: info.title,
      kind: info.kind,
      rawInput: info.raw_input,
    })
    inferredNames.set(info.tool_call_id, name)
    return name
  }

  // First pass: register all agent tool_call IDs
  const agentIds = new Set<string>()
  for (const block of liveMessage.content) {
    if (block.type !== "tool_call") continue
    if (getToolName(block.info) === "agent") {
      agentIds.add(block.info.tool_call_id)
      agentChildren.set(block.info.tool_call_id, [])
    }
  }

  // Second pass: assign children using parentToolUseId or position fallback.
  // Positional fallback only captures while the agent is still in-progress;
  // once it completes/fails, subsequent tool calls are treated as top-level.
  let positionalAgentId: string | null = null

  for (const block of liveMessage.content) {
    if (block.type === "tool_call") {
      const toolName = getToolName(block.info)

      if (toolName === "agent") {
        const isFinal =
          block.info.status === "completed" || block.info.status === "failed"
        // Only capture children while the agent is still running
        positionalAgentId = isFinal ? null : block.info.tool_call_id
      } else {
        // Extract parentToolUseId from ACP meta (Claude Code embeds this
        // under meta.claudeCode.parentToolUseId). Guard each access level
        // to avoid crashes on unexpected shapes from other agents.
        const meta = block.info.meta
        let parentId: string | undefined
        if (meta && typeof meta === "object" && "claudeCode" in meta) {
          const cc = (meta as Record<string, unknown>).claudeCode
          if (cc && typeof cc === "object" && "parentToolUseId" in cc) {
            const pid = (cc as Record<string, unknown>).parentToolUseId
            if (typeof pid === "string") parentId = pid
          }
        }

        // Use explicit parentToolUseId when available, positional fallback
        // only for in-progress agents
        const resolvedParent =
          parentId && agentIds.has(parentId) ? parentId : positionalAgentId

        if (resolvedParent) {
          childToolCallIds.add(block.info.tool_call_id)
          agentChildren
            .get(resolvedParent)
            ?.push({ info: block.info, toolName })
        }
      }
    } else if (positionalAgentId) {
      // A non-tool block (text/thinking/plan) means the main agent is
      // producing new content — stop position-based capture.
      positionalAgentId = null
    }
  }

  // ── Phase 2: Build turns, nesting children inside Agent results ────
  // Split streaming content into multiple turns matching the historical
  // pattern: each "round" (text/thinking + tool calls + tool results) is a
  // separate turn. A new turn starts when a text/thinking/plan block appears
  // after completed tool calls in the current group.
  const groups: MessageTurn["blocks"][] = [[]]
  let currentGroupHasCompletedTool = false
  const inProgressToolCallIds = new Set<string>()

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
        // Skip child tool calls — they are nested inside Agent cards
        if (childToolCallIds.has(block.info.tool_call_id)) break

        const toolName = getToolName(block.info)
        currentBlocks.push({
          type: "tool_use",
          tool_use_id: block.info.tool_call_id,
          tool_name: toolName,
          input_preview: block.info.raw_input,
        })
        const isFinalState =
          block.info.status === "completed" || block.info.status === "failed"
        // Output precedence: raw_output_chunks (terminal polling / SDK
        // raw_output field) wins over content. Some agents stream bash output
        // via raw_output with raw_output_append, others via content-only
        // tool_call_update notifications — we support both.
        const resolvedOutput =
          block.info.raw_output_chunks.length > 0
            ? getJoinedChunks(block.info.raw_output_chunks)
            : block.info.content

        // For agent tool calls, build agent_stats from collected children
        const isAgent = toolName === "agent"
        const children = isAgent
          ? (agentChildren.get(block.info.tool_call_id) ?? [])
          : []
        // Lazy: only construct agentStats when there are children to show
        const agentStats: AgentExecutionStats | undefined =
          isAgent && children.length > 0
            ? {
                tool_calls: children.map(({ info: ci, toolName: cn }) => {
                  const cFinal =
                    ci.status === "completed" || ci.status === "failed"
                  const cOutput =
                    ci.raw_output_chunks.length > 0
                      ? getJoinedChunks(ci.raw_output_chunks)
                      : ci.content
                  return {
                    tool_name: cn,
                    input_preview: ci.raw_input?.substring(0, 500) ?? null,
                    output_preview: cFinal
                      ? (cOutput?.substring(0, 500) ?? null)
                      : null,
                    is_error: ci.status === "failed",
                  }
                }),
              }
            : undefined

        if (isFinalState) {
          currentBlocks.push({
            type: "tool_result",
            tool_use_id: block.info.tool_call_id,
            output_preview: isAgent
              ? cleanAgentOutput(resolvedOutput)
              : resolvedOutput,
            is_error: block.info.status === "failed",
            ...(agentStats ? { agent_stats: agentStats } : {}),
          })
          currentGroupHasCompletedTool = true
        } else if (resolvedOutput || (isAgent && children.length > 0)) {
          // In-progress tool that already produced partial output (or an
          // agent with child calls). Emit the running result so the renderer
          // can display live output / nested tool calls, and flag the
          // tool_call so the adapter keeps state="input-available".
          //
          // For Agents specifically, partial `content` from Claude Code's
          // Task tool echoes the prompt (and subagent message fragments)
          // before the real result arrives — suppress it so the Agent card
          // doesn't duplicate the prompt already shown in the collapsible.
          currentBlocks.push({
            type: "tool_result",
            tool_use_id: block.info.tool_call_id,
            output_preview: isAgent ? null : (resolvedOutput ?? null),
            is_error: false,
            ...(agentStats ? { agent_stats: agentStats } : {}),
          })
          inProgressToolCallIds.add(block.info.tool_call_id)
        }
        break
      }
    }
  }

  const timestamp = new Date(liveMessage.startedAt).toISOString()
  const turns = groups
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

  return { turns, inProgressToolCallIds }
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

      // DB data is authoritative for persisted turns, but keep localTurns while
      // they still represent failed/unpersisted user messages that must survive
      // reload until the parser catches up.
      const isActivelyInteracting = current.syncState === "awaiting_persist"

      const recoverableLocalTurns = current.localTurns.filter(
        isRecoverableUserTurn
      )
      const reconciledLocalTurns = reconcileOptimisticTurns(
        current.localTurns,
        current.detail?.turns ?? [],
        action.detail.turns ?? []
      )
      const reconciledRecoverableLocalTurns = reconcileOptimisticTurns(
        recoverableLocalTurns,
        current.detail?.turns ?? [],
        action.detail.turns ?? []
      )

      const nextLocalTurns = isActivelyInteracting
        ? reconciledLocalTurns
        : reconciledRecoverableLocalTurns

      const nextSession: ConversationRuntimeSession = {
        ...current,
        detail: action.detail,
        detailLoading: false,
        detailError: null,
        externalId: nextExternalId ?? current.externalId,
        localTurns: nextLocalTurns,
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
          ).turns
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
        !action.isLive &&
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

    case "SET_RECOVERY_CONVERSATION_ID":
      return updateSessionInState(state, action.conversationId, (current) => {
        if (current.recoveryConversationId === action.recoveryConversationId) {
          return current
        }

        const nextLocalTurns =
          current.localTurns.length > 0 || action.recoveryConversationId == null
            ? current.localTurns
            : loadRecoverableOptimisticTurns(action.recoveryConversationId)

        return {
          ...current,
          recoveryConversationId: action.recoveryConversationId,
          localTurns: dedupeRecoverableTurns(nextLocalTurns),
        }
      })

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
        recoveryConversationId:
          to.recoveryConversationId ??
          from.recoveryConversationId ??
          (action.toConversationId > 0 ? action.toConversationId : null),
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
    liveMessage: LiveMessage | null,
    isLive?: boolean
  ) => void
  setExternalId: (conversationId: number, externalId: string | null) => void
  setRecoveryConversationId: (
    conversationId: number,
    recoveryConversationId: number | null
  ) => void
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

  useEffect(() => {
    for (const session of state.byConversationId.values()) {
      const recoveryConversationId = session.recoveryConversationId
      if (recoveryConversationId == null) {
        continue
      }

      const recoverableTurns = collectRecoverableUserTurns(session)
      if (recoverableTurns.length > 0) {
        saveRecoverableOptimisticTurns(recoveryConversationId, recoverableTurns)
        continue
      }

      clearRecoverableOptimisticTurns(recoveryConversationId)
    }
  }, [state.byConversationId])

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
      const built = streamingMessage
        ? buildStreamingTurnsFromLiveMessage(conversationId, streamingMessage)
        : null

      const result = [...persisted, ...local, ...optimistic]

      if (built) {
        for (const [i, turn] of built.turns.entries()) {
          result.push({
            key: `streaming-${conversationId}-${streamingMessage?.id ?? "unknown"}-${i}`,
            turn,
            phase: "streaming",
            inProgressToolCallIds: built.inProgressToolCallIds,
          })
        }
      }

      return result
    },
    [state.byConversationId]
  )

  const fetchDetail = useCallback((conversationId: number) => {
    const session = stateRef.current.byConversationId.get(conversationId)
    if (session?.detail || session?.detailLoading) return

    // Allow cold detail loads even when the runtime restored recoverable
    // failed-user turns from localStorage. Only skip when there is truly
    // in-flight data that would make the fetch immediately stale.
    if (
      session &&
      (session.optimisticTurns.length > 0 || session.liveMessage)
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
    (
      conversationId: number,
      liveMessage: LiveMessage | null,
      isLive?: boolean
    ) => {
      dispatch({
        type: "SET_LIVE_MESSAGE",
        conversationId,
        liveMessage,
        isLive,
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

  const setRecoveryConversationId = useCallback(
    (conversationId: number, recoveryConversationId: number | null) => {
      dispatch({
        type: "SET_RECOVERY_CONVERSATION_ID",
        conversationId,
        recoveryConversationId,
      })
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
    const recoveryConversationId =
      stateRef.current.byConversationId.get(conversationId)
        ?.recoveryConversationId ?? null
    if (recoveryConversationId != null) {
      clearRecoverableOptimisticTurns(recoveryConversationId)
    }
    dispatch({ type: "REMOVE_CONVERSATION", conversationId })
  }, [])

  const reset = useCallback(() => {
    for (const session of stateRef.current.byConversationId.values()) {
      if (session.recoveryConversationId != null) {
        clearRecoverableOptimisticTurns(session.recoveryConversationId)
      }
    }
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
      setRecoveryConversationId,
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
      setRecoveryConversationId,
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
