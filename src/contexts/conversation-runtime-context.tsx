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

  // Read-only delegation-child viewer marker. When true, `getTimelineTurns`
  // suppresses the persisted copy of the (single) reply turn while this
  // session has a live or just-promoted reply — so the sub-agent dialog shows
  // the kickoff + live/local reply exactly once, never a persisted partial
  // beside the live stream. Off for normal panels (which never set it), so
  // their multi-turn history is untouched. See `getTimelineTurns`.
  liveOwnsActiveTurn: boolean

  // Known kickoff prompt text for a delegation-child viewer (the parent's
  // `delegate_to_agent` task, available synchronously in the card). While
  // `liveOwnsActiveTurn` is set and the persisted transcript has not yet
  // surfaced the child's user turn (the agent CLI writes its JSONL
  // asynchronously, so the DB read lags the stream by up to seconds),
  // `getTimelineTurns` synthesizes the kickoff user turn from this so it
  // shows immediately above the streaming reply instead of after the child
  // finishes. Cleared automatically once the real persisted user turn lands.
  delegationKickoffText: string | null

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
      /**
       * Keep `liveMessage` / `optimisticTurns` / `localTurns` across this
       * detail load even though `syncState` isn't "awaiting_persist". The
       * sub-agent dialog sets this for a fetch issued while the child is
       * mid-stream: it loads the persisted detail to surface the user kickoff
       * turn, but the bridged/promoted reply must survive the fetch (otherwise
       * the streamed turn would blank until the next ContentDelta re-bridges
       * it, and a late-resolving partial could momentarily replace it).
       */
      preserveLive?: boolean
    }
  | {
      type: "SET_LIVE_OWNS_ACTIVE_TURN"
      conversationId: number
      value: boolean
      /**
       * Optional kickoff prompt text to store alongside the flag. `undefined`
       * leaves the existing `delegationKickoffText` untouched (e.g. a pure
       * clear); a string (or null) overwrites it. The sub-agent dialog passes
       * the parent's known `delegate_to_agent` task so the kickoff user turn
       * can be synthesized before the async transcript catches up.
       */
      kickoffText?: string | null
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
      // Roll back an optimistic user turn that never reached the backend
      // (e.g. the send was rejected because a turn was already in flight, and
      // the draft is being re-queued instead). Resets syncState to idle when no
      // other optimistic turns remain so a stranded `awaiting_persist` doesn't
      // block the next detail reconciliation.
      type: "REMOVE_OPTIMISTIC_TURN"
      conversationId: number
      id: string
    }
  | {
      // Cross-client VIEWER synthesizes the sender's user turn from a
      // `user_message` event / snapshot. Idempotent + sender-guarded in the
      // reducer (never fires on a client that has its own in-flight send).
      type: "APPEND_VIEWER_USER_TURN"
      conversationId: number
      turn: MessageTurn
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
    liveOwnsActiveTurn: false,
    delegationKickoffText: null,
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

/**
 * Stable content signature for a USER turn. The same prompt surfaces under two
 * unrelated id namespaces: a cross-client viewer's synthesized turn uses the
 * broadcast `message_id`, while the SAME prompt, once the agent has written it
 * to its JSONL transcript, comes back from the parser (in `detail.turns`) under
 * a parser-assigned id. Id-based dedup therefore can't recognize the two as one
 * message — only content can. Ids and timestamps are deliberately excluded.
 * The encoding is structurally unambiguous (JSON, so block boundaries can't
 * collide) and compares FULL payload — text verbatim and full image data — so a
 * genuinely different prompt is never mistaken for a match. That matters because
 * a match SUPPRESSES a visible user turn (see `APPEND_VIEWER_USER_TURN`), and it
 * runs only on the rare cross-client viewer append, so comparing full data is
 * fine. Unknown block types are serialized whole rather than collapsed to their
 * tag, so no distinguishing content is silently dropped.
 */
function userTurnContentKey(turn: MessageTurn): string {
  return JSON.stringify(
    turn.blocks.map((b) => {
      switch (b.type) {
        case "text":
          return { t: b.text }
        case "image":
          return { i: b.mime_type, d: b.data }
        default:
          return b
      }
    })
  )
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

      // DB data is authoritative for completed turns. Normally clear all the
      // in-flight buffers (localTurns/optimisticTurns/liveMessage). Preserve
      // them when the user actively sent a message and is awaiting the agent
      // response (awaiting_persist), OR the caller asked to keep the live state
      // via `preserveLive` (the sub-agent dialog, folding the persisted user
      // kickoff in while the child still streams/just-finished its reply — the
      // bridged/promoted reply must outlive the fetch so a late partial can't
      // momentarily replace it).
      //
      // A detail that carries `in_flight_user_turn_id` is itself a MID-TURN
      // snapshot (the backend only stamps it while a turn is running). Such a
      // response must not clobber a more-complete live/promoted reply: a stale one
      // landing just after `completeTurn` promoted the reply would otherwise clear
      // `localTurns`, and the next live turn's in-flight suppression (keyed off the
      // stale id) could then hide that completed reply. So treat it like
      // `preserveLive` and keep every live buffer; a settled (non-in-flight) load
      // replaces them authoritatively.
      const detailIsInFlight = action.detail.in_flight_user_turn_id != null
      const isActivelyInteracting =
        current.syncState === "awaiting_persist" ||
        action.preserveLive === true ||
        detailIsInFlight
      const keepAllLiveBuffers =
        action.preserveLive === true || detailIsInFlight

      const nextSession: ConversationRuntimeSession = {
        ...current,
        detail: action.detail,
        detailLoading: false,
        detailError: null,
        externalId: nextExternalId ?? current.externalId,
        sessionStats: action.detail.session_stats ?? current.sessionStats,
        ...(isActivelyInteracting
          ? keepAllLiveBuffers
            ? {}
            : { localTurns: [] }
          : { localTurns: [], optimisticTurns: [], liveMessage: null }),
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

      // Promote: optimisticTurns + streamingTurns → localTurns. Dedup by turn
      // id (keep the latest copy) so a re-promotion of an already-promoted turn
      // doesn't leave two same-id turns in `localTurns`. This happens when the
      // background `turn_complete` listener races the panel's own promotion
      // after the same liveMessage was re-bridged: the first COMPLETE_TURN puts
      // a snapshot into localTurns, the live turn re-streams under the same id,
      // and a second COMPLETE_TURN would append it again. Identical ids mean the
      // same underlying turn, so the later (most complete) copy supersedes.
      const promotedRaw = [
        ...current.localTurns,
        ...current.optimisticTurns,
        ...streamingTurns,
      ]
      const promotedLastIndexById = new Map<string, number>()
      promotedRaw.forEach((turn, i) => promotedLastIndexById.set(turn.id, i))
      const promoted =
        promotedLastIndexById.size === promotedRaw.length
          ? promotedRaw
          : promotedRaw.filter(
              (turn, i) => promotedLastIndexById.get(turn.id) === i
            )

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

    case "REMOVE_OPTIMISTIC_TURN": {
      const current = state.byConversationId.get(action.conversationId)
      if (!current) return state
      const remaining = current.optimisticTurns.filter(
        (t) => t.id !== action.id
      )
      // Not found → no-op (avoid a needless re-render / identity change).
      if (remaining.length === current.optimisticTurns.length) return state
      return updateSessionInState(state, action.conversationId, (s) => ({
        ...s,
        optimisticTurns: remaining,
        // Drop back to idle once the last in-flight optimistic turn is rolled
        // back, so the `awaiting_persist` set on append doesn't linger and
        // suppress the next detail reconciliation. Concurrent optimistic turns
        // (if any) keep us awaiting_persist.
        syncState: remaining.length === 0 ? "idle" : s.syncState,
      }))
    }

    case "APPEND_VIEWER_USER_TURN": {
      const current =
        state.byConversationId.get(action.conversationId) ??
        createEmptySession(action.conversationId)
      const id = action.turn.id
      // EXACT-id dedup (not a heuristic): the sender's OWN optimistic turn
      // shares this id — the UI threaded its optimistic turn id to the backend,
      // which echoed it as the `user_message` message_id — so the sender drops
      // its own echo here. Also covers an already-promoted turn (localTurns) and
      // a snapshot re-deliver. Keyed on exact id so an UNRELATED optimistic turn
      // on a co-controlling client never suppresses a DIFFERENT sender's prompt.
      //
      // `detail.turns` is checked too: while a turn is in flight the detail
      // endpoint stamps the persisted in-flight user turn with this same
      // broadcast id (see `apply_in_flight_message_id` in the backend), so the
      // synthesized copy defers to the persisted turn in its correct position.
      // This covers OpenCode and Gemini, whose transcript tail mid-stream is
      // `[.., user X, partial assistant Y]` rather than ending at the user turn —
      // the content guard below (which only matches a trailing user turn) can't
      // see X, but the backend stamp (matched by content + turn-start recency)
      // makes this id match X directly.
      //
      // Role-scoped to USER turns: every legitimate match (the sender's optimistic
      // turn, a promoted local turn, the stamped persisted prompt) is a user turn.
      // Requiring the role guards against an id collision — an unrelated ASSISTANT
      // turn that happens to share this id (only reachable via a client id that
      // slipped into another namespace) must never suppress the new prompt.
      if (
        current.optimisticTurns.some((t) => t.id === id && t.role === "user") ||
        current.localTurns.some((t) => t.id === id && t.role === "user") ||
        (current.detail?.turns.some((t) => t.id === id && t.role === "user") ??
          false)
      ) {
        return state
      }
      // CONTENT dedup against persisted history. The exact-id guard above is
      // blind to the prompt once the agent has written it to its JSONL
      // transcript and it has been reloaded into `detail.turns`: the parser
      // assigns it an unrelated id there, so the synthesized turn (keyed by the
      // broadcast message_id) and the persisted turn never share an id. Without
      // this, a viewer that attaches mid-stream after the prompt was persisted
      // renders the user message twice.
      //
      // Suppress ONLY when the synthesized prompt equals the LAST persisted turn
      // AND that turn is a user turn — i.e. the transcript currently ends exactly
      // at the in-flight prompt, its reply still streaming in `liveMessage` and
      // not yet written (the normal mid-stream shape for Claude/Codex, whose
      // assistant turn is appended to the JSONL only on completion). We must NOT
      // look past a trailing assistant turn: a PREVIOUS, already-answered user
      // turn with identical text (e.g. a repeated "continue") ends with its
      // completed assistant reply, so doing so would wrongly suppress a genuinely
      // new prompt the transcript hasn't captured yet. When in doubt we keep the
      // synthesized turn visible — a transient duplicate is recoverable, a hidden
      // prompt is not. (Agents that persist a PARTIAL assistant turn mid-stream —
      // OpenCode and Gemini — end with that partial rather than the user turn, so
      // they fall through this content guard; the backend instead stamps their
      // persisted user turn with this broadcast id, handled by the exact-id guard
      // above.)
      //
      // Invariant: a trailing persisted user turn is the in-flight prompt. If a
      // prior run instead left a bare trailing user turn (crash/cancel before any
      // reply) and the user re-sends identical text, this self-corrects — the new
      // prompt is written to the transcript near-instantly, becoming the trailing
      // turn, at which point suppression of the (now redundant) synthesized copy
      // is correct. The only-suppress-on-exact-trailing-match keeps the worst case
      // a sub-second transient, never a stuck hidden prompt.
      const persistedTurns = current.detail?.turns
      const lastPersisted = persistedTurns?.[persistedTurns.length - 1]
      if (
        lastPersisted?.role === "user" &&
        userTurnContentKey(lastPersisted) === userTurnContentKey(action.turn)
      ) {
        return state
      }
      // Append as an optimistic turn so it flows through the EXISTING promotion
      // (COMPLETE_TURN → localTurns) and reset-on-fetch machinery, identical to
      // the owner's own user turn. Deliberately does NOT set
      // `syncState: "awaiting_persist"` — the viewer didn't send, so a later
      // detail fetch should cleanly replace the synthesized turn with persisted
      // truth (awaiting_persist would preserve it and risk a duplicate).
      return updateSessionInState(state, action.conversationId, (s) => ({
        ...s,
        optimisticTurns: [...s.optimisticTurns, action.turn],
      }))
    }

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
        liveOwnsActiveTurn: to.liveOwnsActiveTurn || from.liveOwnsActiveTurn,
        delegationKickoffText:
          to.delegationKickoffText ?? from.delegationKickoffText,
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

    case "SET_LIVE_OWNS_ACTIVE_TURN": {
      const current = state.byConversationId.get(action.conversationId)
      // No-op (don't materialize a session) when clearing an absent one with
      // no kickoff text to record.
      if (!current && !action.value && action.kickoffText == null) return state
      // `undefined` kickoffText leaves the stored value untouched.
      const nextKickoff =
        action.kickoffText !== undefined
          ? action.kickoffText
          : (current?.delegationKickoffText ?? null)
      if (
        current &&
        current.liveOwnsActiveTurn === action.value &&
        current.delegationKickoffText === nextKickoff
      ) {
        return state
      }
      return updateSessionInState(state, action.conversationId, (s) => ({
        ...s,
        liveOwnsActiveTurn: action.value,
        delegationKickoffText: nextKickoff,
      }))
    }

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
  /**
   * Re-fetch persisted detail, bypassing the active-data guard.
   * `options.preserveLive` (default false) keeps the current `liveMessage`,
   * `localTurns`, and `optimisticTurns` alive across the detail load — used by
   * the sub-agent dialog when fetching while the child is mid-stream, so the
   * bridged live reply survives and the just-fetched detail (which may include
   * a partial in-progress assistant turn from the DB) is rendered through the
   * `liveOwnsActiveTurn` filter instead of being blindly overwritten.
   */
  refetchDetail: (
    conversationId: number,
    options?: { preserveLive?: boolean }
  ) => void
  completeTurn: (
    conversationId: number,
    liveMessage?: LiveMessage | null
  ) => void
  appendOptimisticTurn: (
    conversationId: number,
    turn: MessageTurn,
    turnToken: string
  ) => void
  /** Roll back an optimistic user turn that never reached the backend (e.g. a
   *  send rejected as "turn in progress", whose draft is being re-queued). */
  removeOptimisticTurn: (conversationId: number, id: string) => void
  /** Cross-client VIEWER: synthesize the sender's user turn from a broadcast
   *  `user_message` / snapshot. No-op on the sender (sender-guarded + idempotent
   *  in the reducer). */
  appendViewerUserTurn: (conversationId: number, turn: MessageTurn) => void
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
  /**
   * Mark this session's reply as live-owned (true = the sub-agent dialog is
   * viewing a child that owns its reply via the live bridge / localTurns).
   * While true, `getTimelineTurns` strips the persisted copy of the reply so
   * the live/local reply is shown exactly once. The optional `kickoffText`
   * records the parent's `delegate_to_agent` task so the kickoff user turn can
   * be synthesized while the async transcript lags (pass `undefined` to leave
   * any stored kickoff untouched).
   */
  setLiveOwnsActiveTurn: (
    conversationId: number,
    value: boolean,
    kickoffText?: string | null
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

      // Phase 1: DB historical turns.
      // When liveOwnsActiveTurn is set (sub-agent dialog), the live/local reply
      // is authoritative for the child's current (only) reply. Strip any
      // persisted assistant turns while there's a live or just-promoted local
      // reply in this session — only the kickoff prefix (everything before the
      // first assistant turn) is shown from the DB. This eliminates the
      // partial-plus-live duplicate for all timing scenarios, including a
      // connection-id-null open where we can't read the live store during fetch.
      //
      // Delegation children are SINGLE-REPLY (one-shot): stripping from the
      // first assistant turn onward removes exactly the persisted copy of that
      // one reply. (A hypothetical multi-turn child would have earlier replies
      // hidden during the live/grace window — not a case the viewer supports.)
      const rawPersistedTurns = session.detail?.turns ?? []
      const hasLiveOrLocalReply =
        session.liveOwnsActiveTurn &&
        (session.liveMessage !== null || session.localTurns.length > 0)
      const firstAssistantIdx = hasLiveOrLocalReply
        ? rawPersistedTurns.findIndex((t) => t.role === "assistant")
        : -1
      const persistedTurns =
        hasLiveOrLocalReply && firstAssistantIdx !== -1
          ? rawPersistedTurns.slice(0, firstAssistantIdx)
          : rawPersistedTurns

      // Suppress the persisted PARTIAL in-flight reply for a non-delegation
      // cross-client viewer. While a reply is streaming, some agents (OpenCode,
      // Gemini) persist a partial assistant turn for it under a parser id; loaded
      // into `detail` it sits beside the live reply (a separate assistant turn
      // under a `live-…` id), and `mergeConsecutiveAssistantTurns` concatenates
      // the two — so the already-persisted head (e.g. the first reasoning block)
      // renders twice. Hide that persisted partial, but ONLY while `liveMessage`
      // is in hand: the live stream carries the full reply (the attach snapshot is
      // built atomically and includes it), so this only ever hides from render
      // what the live stream is concurrently showing — never dropping a reply we
      // can't re-show. The moment the turn ends, `liveMessage` clears and the
      // persisted copy (now complete) renders normally; the brief promote→refetch
      // grace window can show a transient visible duplicate, never a hidden turn.
      //
      // The in-flight prompt is identified authoritatively by the backend, which
      // reports the id of the persisted user turn it stamped as the in-flight one
      // (`detail.in_flight_user_turn_id`). This is robust where a frontend anchor
      // is not: the viewer's synthesized prompt may be suppressed (the persisted
      // copy already carries the broadcast id), and `liveMessage.startedAt` is the
      // client clock on the streaming path — neither can locate the prompt across
      // machines. When the new prompt isn't persisted yet the backend reports no
      // id, so an earlier completed round's reply is never mistaken for a partial.
      const inFlightPromptId = session.detail?.in_flight_user_turn_id ?? null
      const inFlightPromptIdx =
        !hasLiveOrLocalReply &&
        session.liveMessage !== null &&
        inFlightPromptId !== null
          ? persistedTurns.findIndex(
              (t) => t.role === "user" && t.id === inFlightPromptId
            )
          : -1
      const visiblePersistedTurns =
        inFlightPromptIdx === -1
          ? persistedTurns
          : persistedTurns.filter(
              (t, i) => i <= inFlightPromptIdx || t.role !== "assistant"
            )

      const persisted: ConversationTimelineTurn[] = visiblePersistedTurns.map(
        (turn, index) => ({
          key: `persisted-${conversationId}-${turn.id}-${index}`,
          turn,
          phase: "persisted" as const,
        })
      )

      // Synthetic delegation kickoff. The child agent CLI writes its JSONL
      // transcript asynchronously, so the persisted detail can lag the live
      // stream by up to seconds — during which `persistedTurns` carries no user
      // turn and the dialog would show the streaming reply with no kickoff above
      // it. When this is a delegation-child viewer (`liveOwnsActiveTurn`) and no
      // persisted user turn has surfaced yet, synthesize the kickoff from the
      // known parent task text so it shows immediately. The moment the real
      // persisted user turn lands, this condition turns off and the authentic
      // turn is used instead — no duplicate, no cleanup needed.
      if (
        session.liveOwnsActiveTurn &&
        session.delegationKickoffText &&
        !persistedTurns.some((t) => t.role === "user")
      ) {
        persisted.unshift({
          key: `kickoff-${conversationId}`,
          turn: {
            id: `kickoff-${conversationId}`,
            role: "user",
            blocks: [{ type: "text", text: session.delegationKickoffText }],
            // Best-effort timestamp: the persisted summary (once loaded) or the
            // live reply's start; falls back to "" only in the brief window
            // before either exists. Consumers in the render path tolerate "";
            // the fallbacks keep date formatters off an empty string in the
            // common case.
            timestamp:
              session.detail?.summary.created_at ??
              (session.liveMessage
                ? new Date(session.liveMessage.startedAt).toISOString()
                : ""),
          },
          phase: "persisted",
        })
      }

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

      // Invariant: the timeline never contains two turns with the same id. A
      // premature/duplicate COMPLETE_TURN (e.g. the background `turn_complete`
      // listener in ConversationDetailPanel racing the panel's own promotion)
      // can leave the in-flight turn in BOTH `localTurns` (a promoted snapshot)
      // and the still-streaming `liveMessage`, or — after a final re-promotion
      // once the same liveMessage was re-bridged — twice in `localTurns`. All
      // copies are built by `buildStreamingTurnsFromLiveMessage` from that one
      // liveMessage, so they share `live-<cid>-<liveMessageId>[-i]` ids.
      // Rendering both duplicates the whole assistant turn (visible doubling +
      // React duplicate-key warnings once `mergeConsecutiveAssistantTurns`
      // flat-maps their parts).
      //
      // Retain rule is role-aware (all entries sharing an id are the same
      // underlying turn, so the role is unambiguous):
      //   - ASSISTANT (and any non-user): keep the LAST occurrence. The live
      //     streaming copy (appended last) wins over an earlier promoted
      //     snapshot, and a re-promoted local turn wins over its stale copy.
      //   - USER: keep the FIRST occurrence. When the detail endpoint stamps the
      //     persisted in-flight user turn with the broadcast id, that persisted
      //     copy is emitted first, in its correct position before any partial
      //     assistant reply; a same-id optimistic/synthesized copy is appended
      //     later (and, for the sender, survives a mid-turn `awaiting_persist`
      //     refetch). Keeping the persisted copy preserves ordering — otherwise
      //     the prompt would render after its own streaming reply.
      // Real turns always have distinct ids (liveMessage.id is minted fresh per
      // prompt cycle, DB turn ids are unique), so a normal multi-turn timeline
      // has no collisions and is returned untouched.
      //
      // The key includes the role, not just the id, so the merge only ever
      // collapses entries that are genuinely the same turn (same id AND role).
      // Should two DIFFERENT-role turns ever share an id — only reachable via a
      // client id that collided into another namespace — they are kept separately
      // (a recoverable visible duplicate) instead of one silently overwriting the
      // other, which could hide a user prompt.
      const retainKey = (turn: MessageTurn) =>
        JSON.stringify([turn.role, turn.id])
      const retainIndexByKey = new Map<string, number>()
      result.forEach((entry, i) => {
        const key = retainKey(entry.turn)
        const existing = retainIndexByKey.get(key)
        // First sighting always records; later sightings overwrite only for
        // non-user turns (keep-last). User turns keep their first index.
        if (existing === undefined || entry.turn.role !== "user") {
          retainIndexByKey.set(key, i)
        }
      })
      const deduped =
        retainIndexByKey.size === result.length
          ? result
          : result.filter(
              (entry, i) => retainIndexByKey.get(retainKey(entry.turn)) === i
            )

      timelineCacheRef.current.set(session, deduped)
      return deduped
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
    (conversationId: number, options?: { preserveLive?: boolean }) => {
      const generation = bumpFetchGeneration(conversationId)
      dispatch({ type: "FETCH_DETAIL_START", conversationId })
      getFolderConversation(conversationId)
        .then((detail) => {
          if (!isLatestGeneration(conversationId, generation)) return
          dispatch({
            type: "FETCH_DETAIL_SUCCESS",
            conversationId,
            detail,
            preserveLive: options?.preserveLive ?? false,
          })
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

  const removeOptimisticTurn = useCallback(
    (conversationId: number, id: string) => {
      dispatch({ type: "REMOVE_OPTIMISTIC_TURN", conversationId, id })
    },
    []
  )

  const appendViewerUserTurn = useCallback(
    (conversationId: number, turn: MessageTurn) => {
      dispatch({ type: "APPEND_VIEWER_USER_TURN", conversationId, turn })
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

  const setLiveOwnsActiveTurn = useCallback(
    (conversationId: number, value: boolean, kickoffText?: string | null) => {
      dispatch({
        type: "SET_LIVE_OWNS_ACTIVE_TURN",
        conversationId,
        value,
        kickoffText,
      })
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
      removeOptimisticTurn,
      appendViewerUserTurn,
      setLiveMessage,
      setExternalId,
      setSyncState,
      migrateConversation,
      setPendingCleanup,
      setAcpLoadError,
      setLiveOwnsActiveTurn,
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
      removeOptimisticTurn,
      appendViewerUserTurn,
      setLiveMessage,
      setExternalId,
      setSyncState,
      migrateConversation,
      setPendingCleanup,
      setAcpLoadError,
      setLiveOwnsActiveTurn,
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
