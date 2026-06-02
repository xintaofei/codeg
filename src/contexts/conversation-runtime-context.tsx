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
import type {
  LiveMessage,
  ToolCallInfo,
} from "@/contexts/acp-connections-context"
import { getFolderConversation } from "@/lib/api"
import type {
  AgentExecutionStats,
  DbConversationDetail,
  MessageTurn,
  SessionStats,
  ToolCallStatus,
  TurnUsage,
} from "@/lib/types"
import {
  inferLiveToolName,
  parseGoalUpdateTitle,
} from "@/lib/tool-call-normalization"
import { toErrorMessage } from "@/lib/app-error"

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

  // DB data (cold open only)
  detail: DbConversationDetail | null
  detailLoading: boolean
  detailError: string | null

  // ACP `session/load` failed in a non-recoverable way (currently only when
  // the agent reports ResourceNotFound for the historical session_id). Set
  // by the connections layer via setAcpLoadError; cleared by the user
  // pressing Reload, by a successful detail refetch, or when a new ACP
  // session takes over.
  acpLoadError: string | null

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

// Shared stable reference for the "no session" timeline result, so callers
// memoizing on the returned array (MessageListView's `threadItems`) don't see
// a fresh array on every render for conversations that don't exist yet.
const EMPTY_TIMELINE: ConversationTimelineTurn[] = []

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
      /**
       * Optional authoritative liveMessage from the caller. Used to avoid a
       * race where the connections-context batches the final STREAM_BATCH
       * and STATUS_CHANGED into one render: by the time COMPLETE_TURN runs,
       * the panel's mirror effect that copies conn.liveMessage into
       * session.liveMessage has not yet executed for the current render,
       * so session.liveMessage is one render stale and missing the final
       * text chunk. When provided, this value is preferred over
       * session.liveMessage for the snapshot.
       */
      liveMessage?: LiveMessage | null
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
        completed_at?: string | null
      }>
      sessionStats?: SessionStats | null
    }
  | {
      type: "SET_ACP_LOAD_ERROR"
      conversationId: number
      error: string | null
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
    acpLoadError: null,
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

/**
 * Decide whether a live `ToolCallInfo` is codex-acp's image-generation
 * tool call. Detection has to fire during the in-flight window
 * (ImageGenerationBegin, no images yet) so we can't rely on `images.length`
 * alone — the load-bearing signal during that window is the title.
 *
 * Layered detection:
 *   1. `title === "Image generation"` — codex-acp PR #271 hardcodes this
 *      exact English string in `start_image_generation` and
 *      `end_image_generation`. Primary path.
 *   2. Case-insensitive title match — defensive for any future codex-acp
 *      casing/whitespace drift.
 *   3. `images.length > 0` — defensive when title is somehow lost but
 *      images are present (e.g. a snapshot replay that drops the title).
 *
 * The function is intentionally NOT a generic `kind === "other"` matcher
 * because many tools surface as ToolKind::Other.
 */
function isImageGenerationToolCall(info: {
  title?: string | null
  images?: { length: number } | null
}): boolean {
  const title = (info.title ?? "").trim()
  if (title === "Image generation") return true
  if (title.toLowerCase() === "image generation") return true
  return (info.images?.length ?? 0) > 0
}

/**
 * Narrow the wire-typed `ToolCallInfo.status` (declared as `string`) into
 * the strict `ToolCallStatus` union — the reducer only ever stores wire
 * values, but the type system doesn't see that. Anything else falls back
 * to `null` and the renderer treats it as in-flight.
 */
function narrowToolCallStatus(status: string): ToolCallStatus | null {
  switch (status) {
    case "pending":
    case "in_progress":
    case "completed":
    case "failed":
      return status
    default:
      return null
  }
}

/**
 * Strip codex-acp's `"Revised prompt: <text>"` framing from a live
 * `ToolCallInfo.content` string and return the inner text. codex-acp PR #271
 * wraps the codex `revised_prompt` field this way before serialising it as
 * `ToolCallContent::Text` (see `image_generation_content` in codex-acp). The
 * prefix is hardcoded English in upstream, so we match it literally.
 *
 * Returns `null` when content is missing, empty after trimming, or doesn't
 * carry a recognisable revised-prompt frame.
 */
function extractRevisedPrompt(content: string | null): string | null {
  if (!content) return null
  const trimmed = content.trim()
  if (trimmed.length === 0) return null
  const PREFIX = "Revised prompt:"
  if (trimmed.startsWith(PREFIX)) {
    const rest = trimmed.slice(PREFIX.length).trim()
    return rest.length > 0 ? rest : null
  }
  // Fall back to the raw content for unforeseen wrappers (e.g. localized
  // frames in future codex-acp versions). Better to surface something to the
  // user than silently drop it.
  return trimmed
}

function resolveGoalToolInputFromLiveTitle(
  toolName: string,
  info: ToolCallInfo
): string | null {
  if (info.raw_input && info.raw_input.trim().length > 0) {
    return info.raw_input
  }

  const goal = parseGoalUpdateTitle(info.title)
  if (!goal) return info.raw_input

  if (toolName === "create_goal") {
    return JSON.stringify({ objective: goal.objective })
  }
  if (toolName === "update_goal") {
    return JSON.stringify({
      status: goal.status,
      objective: goal.objective,
    })
  }

  return info.raw_input
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
      meta: info.meta,
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
        // Keep empty thinking blocks during streaming so the reasoning UI
        // can show its "Thinking..." indicator before any reasoning text
        // arrives (and for newer Claude models that redact reasoning text
        // entirely while still emitting thinking blocks).
        currentBlocks.push({ type: "thinking", text: block.text })
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

        // codex-acp v0.14+ image generation surfaces as a `ToolCall` whose
        // ACP-wire shape is `(title="Image generation", kind=Other,
        // content=[Text("Revised prompt: ..."), Image{...}])`. Render this
        // as a dedicated `image_generation` block instead of the generic
        // tool_use + tool_result pair so:
        //   - live and historical (JSONL) paths converge on the same
        //     ContentBlock variant (zero asymmetry)
        //   - the user sees one labeled "Image generation" card instead of
        //     a generic tool card sitting above a detached image
        //   - the new card is not folded into `groupConsecutiveToolCalls`
        //     (which only consumes `tool-call` parts)
        if (isImageGenerationToolCall(block.info)) {
          // codex-acp emits one image per ToolCall (each `call_id` is a
          // single ImageGenerationBegin/End pair). One block per image —
          // multiple images in a turn become multiple consecutive blocks.
          // Defensive fallback: if a future agent ever sends multiple
          // images in one ToolCall, we still emit one block per image so
          // each renders as its own card.
          const imgs = block.info.images ?? []
          const revisedPrompt = extractRevisedPrompt(block.info.content)
          // Live ToolCallStatus is forwarded so the renderer can show a
          // failure slot when codex reports the call failed before any
          // image bytes arrived. Without this the in-flight skeleton would
          // sit there until TurnComplete clears `active_tool_calls`.
          const status = narrowToolCallStatus(block.info.status)
          if (imgs.length === 0) {
            // In-flight placeholder: title arrived, image hasn't (or the
            // call failed without producing one).
            currentBlocks.push({
              type: "image_generation",
              revised_prompt: revisedPrompt,
              image: null,
              status,
            })
          } else {
            for (const img of imgs) {
              currentBlocks.push({
                type: "image_generation",
                revised_prompt: revisedPrompt,
                image: {
                  data: img.data,
                  mime_type: img.mime_type,
                  uri: img.uri ?? null,
                },
                status,
              })
            }
          }
          if (status === "completed" || status === "failed") {
            currentGroupHasCompletedTool = true
          }
          break
        }

        const toolName = getToolName(block.info)
        currentBlocks.push({
          type: "tool_use",
          tool_use_id: block.info.tool_call_id,
          tool_name: toolName,
          input_preview: resolveGoalToolInputFromLiveTitle(
            toolName,
            block.info
          ),
          // Forward the ACP `meta` field downstream so the renderer can
          // read delegation state (`meta["codeg.delegation"]`) for
          // pre-binding / post-refresh fallback rendering of
          // <DelegatedSubThread>. Opaque pass-through — adapter layer
          // does not interpret.
          meta: block.info.meta,
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

      // Idempotency guard — a single turn can be promoted twice when the
      // panel's connStatus-edge effect and ConversationDetailPanel's
      // background turn_complete listener both fire (e.g. when the bg
      // listener's tab-membership check misses the new-conversation race
      // and proceeds). The first call drains liveMessage + optimisticTurns
      // into localTurns and lands syncState=idle; a second pass with a
      // caller-provided action.liveMessage would otherwise rebuild
      // streamingTurns from action.liveMessage and append them on top of
      // the already-promoted turns, producing a duplicated assistant
      // message in the timeline. If the session is already drained, the
      // turn is a no-op regardless of action.liveMessage.
      if (
        current.liveMessage === null &&
        current.optimisticTurns.length === 0 &&
        current.syncState === "idle"
      ) {
        // Surface the unexpected double-invocation so future regressions
        // are noticed in the console rather than silently swallowed.
        // Reaching this branch means an upstream guard (e.g. the bg
        // listener's tab-membership check) failed to dedupe.
        console.warn(
          "[conversation-runtime] COMPLETE_TURN dispatched on an already-drained session; ignoring",
          { conversationId: action.conversationId }
        )
        return state
      }

      // Prefer the caller-provided liveMessage when present. The panel's
      // mirror effect that syncs conn.liveMessage → session.liveMessage runs
      // AFTER this effect within the same render, so session.liveMessage
      // misses the final stream chunk that arrived in the same React batch
      // as the status transition.
      const sourceLiveMessage =
        action.liveMessage !== undefined
          ? action.liveMessage
          : current.liveMessage

      // Convert liveMessage to completed MessageTurns (split into rounds)
      const streamingTurns = sourceLiveMessage
        ? buildStreamingTurnsFromLiveMessage(
            current.conversationId,
            sourceLiveMessage
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
        const newCompletedAt = turn.completed_at ?? patch.completed_at
        if (
          newUsage !== turn.usage ||
          newDuration !== turn.duration_ms ||
          newModel !== turn.model ||
          newCompletedAt !== turn.completed_at
        ) {
          patchedTurns[patch.index] = {
            ...turn,
            usage: newUsage,
            duration_ms: newDuration,
            model: newModel,
            completed_at: newCompletedAt,
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

    case "SET_ACP_LOAD_ERROR":
      return updateSessionInState(state, action.conversationId, (current) => ({
        ...current,
        acpLoadError: action.error,
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
  completeTurn: (
    conversationId: number,
    liveMessage?: LiveMessage | null
  ) => void
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
  setAcpLoadError: (conversationId: number, error: string | null) => void
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

  // Timeline cache keyed by the session OBJECT (not the id). `updateSessionInState`
  // always allocates a fresh session object for the conversation it touches and
  // preserves the reference for every other conversation, so an unrelated
  // dispatch (e.g. another tab's streaming token) leaves this conversation's
  // session ref untouched — returning the identical timeline array then lets
  // MessageListView's `threadItems` useMemo short-circuit instead of re-running
  // the adapt/merge/scan pipeline on every cross-tab broadcast render.
  //
  // A WeakMap is used so a cache entry is collected automatically once its
  // session object is no longer referenced by state (the session is replaced on
  // update, or dropped on REMOVE_CONVERSATION / RESET / migration). The value
  // can transitively retain a full transcript (detail.turns, live message,
  // images, diffs), so a plain Map keyed by id would leak those indefinitely in
  // a long-lived desktop provider as conversations are opened and closed.
  // Keying by session is sound because each session object belongs to exactly
  // one conversation id (no reducer path aliases a session across ids), so the
  // conversation id baked into the result's keys is always consistent.
  const timelineCacheRef = useRef(
    new WeakMap<ConversationRuntimeSession, ConversationTimelineTurn[]>()
  )

  const getTimelineTurns = useCallback(
    (conversationId: number): ConversationTimelineTurn[] => {
      const session = state.byConversationId.get(conversationId)
      if (!session) return EMPTY_TIMELINE

      const cached = timelineCacheRef.current.get(session)
      if (cached) return cached

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

      timelineCacheRef.current.set(session, result)
      return result
    },
    [state.byConversationId]
  )

  // Per-conversation fetch-generation counter. Each `fetchDetail` /
  // `refetchDetail` / `removeConversation` increments the counter for
  // that conversationId; an outstanding fetch promise captures the
  // value it was issued with and refuses to dispatch its
  // `FETCH_DETAIL_SUCCESS` / `FETCH_DETAIL_ERROR` if the counter has
  // moved on. Closes two races:
  //
  //   1. Stale-response overwrite — open A → close A → open B → A
  //      resolves last and clobbers B's fresh detail with stale data.
  //   2. Resurrection after removeConversation — an in-flight fetch
  //      resolves after the session was removed and recreates it with
  //      stale data, blocking the next open from seeing fresh state.
  //
  // The counter lives in a ref so callers don't trigger renders. Cells
  // are kept indefinitely (small int per conversation); a cleanup
  // sweep isn't needed for the expected cardinality.
  const fetchGenerationRef = useRef(new Map<number, number>())

  const bumpFetchGeneration = useCallback((conversationId: number): number => {
    const map = fetchGenerationRef.current
    const next = (map.get(conversationId) ?? 0) + 1
    map.set(conversationId, next)
    return next
  }, [])

  const isLatestGeneration = useCallback(
    (conversationId: number, generation: number): boolean =>
      fetchGenerationRef.current.get(conversationId) === generation,
    []
  )

  const fetchDetail = useCallback(
    (conversationId: number) => {
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

      const generation = bumpFetchGeneration(conversationId)
      dispatch({ type: "FETCH_DETAIL_START", conversationId })
      getFolderConversation(conversationId)
        .then((detail) => {
          if (!isLatestGeneration(conversationId, generation)) return
          dispatch({ type: "FETCH_DETAIL_SUCCESS", conversationId, detail })
        })
        .catch((error: unknown) => {
          if (!isLatestGeneration(conversationId, generation)) return
          dispatch({
            type: "FETCH_DETAIL_ERROR",
            conversationId,
            error: toErrorMessage(error),
          })
        })
    },
    [bumpFetchGeneration, isLatestGeneration]
  )

  const refetchDetail = useCallback(
    (conversationId: number) => {
      const generation = bumpFetchGeneration(conversationId)
      dispatch({ type: "FETCH_DETAIL_START", conversationId })
      getFolderConversation(conversationId)
        .then((detail) => {
          if (!isLatestGeneration(conversationId, generation)) return
          dispatch({ type: "FETCH_DETAIL_SUCCESS", conversationId, detail })
        })
        .catch((error: unknown) => {
          if (!isLatestGeneration(conversationId, generation)) return
          dispatch({
            type: "FETCH_DETAIL_ERROR",
            conversationId,
            error: toErrorMessage(error),
          })
        })
    },
    [bumpFetchGeneration, isLatestGeneration]
  )

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
                completed_at?: string | null
              }> = []

              for (let i = 0; i < localAssistantIndices.length; i++) {
                const parsedIdx = offset + i
                let usageToApply: TurnUsage | null | undefined
                let durationToApply: number | null | undefined
                let modelToApply: string | null | undefined
                // For the merged-sub-turn case (offset > 0), the latest
                // completion is parsed[offset + i] (the sub-turn we matched);
                // earlier rolled-in parsed turns precede it in time, so we
                // don't aggregate completion timestamps.
                let completedAtToApply: string | null | undefined

                if (parsedIdx >= 0 && parsedIdx < parsedAssistantTurns.length) {
                  const pt = parsedAssistantTurns[parsedIdx]
                  usageToApply = pt.usage
                  durationToApply = pt.duration_ms
                  modelToApply = pt.model
                  completedAtToApply = pt.completed_at
                }

                // When the parser splits the response into more sub-turns
                // than the live stream did (offset > 0), roll the leading
                // unmatched parsed turns' usage/duration into local[0] so
                // that sum(local) equals sum(parsed). Without this, the
                // mid-stream stats row under-reports tokens vs. a fresh
                // historical reload, which clears localTurns and shows
                // every parsed turn directly.
                if (i === 0 && offset > 0) {
                  for (let j = 0; j < offset; j++) {
                    const extra = parsedAssistantTurns[j]
                    if (extra.usage) {
                      if (!usageToApply) {
                        usageToApply = { ...extra.usage }
                      } else {
                        usageToApply = {
                          input_tokens:
                            usageToApply.input_tokens +
                            extra.usage.input_tokens,
                          output_tokens:
                            usageToApply.output_tokens +
                            extra.usage.output_tokens,
                          cache_creation_input_tokens:
                            usageToApply.cache_creation_input_tokens +
                            extra.usage.cache_creation_input_tokens,
                          cache_read_input_tokens:
                            usageToApply.cache_read_input_tokens +
                            extra.usage.cache_read_input_tokens,
                        }
                      }
                    }
                    if (typeof extra.duration_ms === "number") {
                      durationToApply =
                        (durationToApply ?? 0) + extra.duration_ms
                    }
                    if (!modelToApply && extra.model) {
                      modelToApply = extra.model
                    }
                  }
                }

                if (
                  !usageToApply &&
                  !durationToApply &&
                  !modelToApply &&
                  !completedAtToApply
                )
                  continue
                patches.push({
                  index: localAssistantIndices[i],
                  usage: usageToApply,
                  duration_ms: durationToApply,
                  model: modelToApply,
                  completed_at: completedAtToApply,
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

  const completeTurn = useCallback(
    (conversationId: number, liveMessage?: LiveMessage | null) => {
      dispatch({ type: "COMPLETE_TURN", conversationId, liveMessage })
    },
    []
  )

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

  const setAcpLoadError = useCallback(
    (conversationId: number, error: string | null) => {
      dispatch({ type: "SET_ACP_LOAD_ERROR", conversationId, error })
    },
    []
  )

  const removeConversation = useCallback(
    (conversationId: number) => {
      // Invalidate any outstanding fetch for this conversation so a
      // late-arriving response can't resurrect the session with stale
      // detail. See `fetchGenerationRef` comment above.
      bumpFetchGeneration(conversationId)
      dispatch({ type: "REMOVE_CONVERSATION", conversationId })
    },
    [bumpFetchGeneration]
  )

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
      setAcpLoadError,
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
      setAcpLoadError,
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
