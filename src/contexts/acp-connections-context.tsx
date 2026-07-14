"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react"
import { useTranslations } from "next-intl"
import { subscribe, getEventStream } from "@/lib/platform"
import type {
  AttachHandlers,
  EventStreamSubscription,
} from "@/lib/transport/types"
import { randomUUID } from "@/lib/utils"
import { inferLiveToolName } from "@/lib/tool-call-normalization"
import {
  acpConnect,
  acpGetAgentStatus,
  acpPrompt,
  acpSetMode,
  acpSetConfigOption,
  acpCancel,
  acpRespondPermission,
  acpAnswerQuestion,
  acpDisconnect,
  acpTouchConnection,
  acpGetSessionSnapshot,
  acpFindConnectionForConversation,
} from "@/lib/api"
import { denormalizeSnapshot } from "@/lib/snapshot-denormalize"
import { buildDelegationSeedEnvelopes } from "@/lib/delegation-seed"
import {
  getConversationIdByExternalIdFromStore,
  useConversationRuntimeStore,
} from "@/stores/conversation-runtime-store"
import type {
  AgentType,
  AcpAgentStatus,
  AcpEvent,
  ActiveDelegationState,
  AvailableCommandInfo,
  ConfigStaleKind,
  ConnectionStatus,
  ConversationConnectionInfo,
  EventEnvelope,
  PlanEntryInfo,
  PermissionOptionInfo,
  PendingQuestionState,
  QuestionAnswer,
  SessionConfigOptionInfo,
  SessionModeStateInfo,
  SessionUsageUpdateInfo,
  PromptCapabilitiesInfo,
  PromptInputBlock,
  ToolCallImageWire,
  UserMessageBlock,
} from "@/lib/types"
import { AGENT_LABELS } from "@/lib/types"
import {
  CONNECTION_IDLE_TIMEOUT_MS,
  CONNECTION_KEEPALIVE_INTERVAL_MS,
  IDLE_SWEEP_INTERVAL_MS,
} from "@/lib/constants"
import { sendSystemNotification } from "@/lib/notification"
import {
  getSavedPrefsForConnect,
  saveModePreference,
  saveConfigPreference,
} from "@/lib/selector-prefs-storage"
import { useAlertContext, type AlertAction } from "@/contexts/alert-context"
import { useActiveFolder } from "@/contexts/active-folder-context"

// ── Shared types (re-exported for consumers) ──

/** ACP extensibility metadata attached to tool calls. */
export type ToolCallMeta = Record<string, unknown> | null

/**
 * An image attached to a tool call (e.g. codex-acp v0.14+ image generation).
 * Re-exports the wire-level `ToolCallImageWire` from `@/lib/types` so that
 * snapshot, live `tool_call(_update)` events, and `ToolCallInfo` share one
 * shape. `data` is base64 (potentially multi-MB), `mime_type` defaults to
 * `image/png` when the agent omits it, `uri` is the on-disk path when the
 * agent persisted the asset (e.g. codex's `~/.codex/generated_images/...`).
 */
export type ToolCallImage = ToolCallImageWire

export interface ToolCallInfo {
  tool_call_id: string
  title: string
  kind: string
  status: string
  content: string | null
  raw_input: string | null
  raw_output_chunks: string[]
  raw_output_total_bytes: number
  locations: unknown
  meta: ToolCallMeta
  /**
   * Replace-on-update: a fresh ToolCallUpdate carrying images replaces this
   * vec; an absent images field preserves the prior value. Empty array
   * means "no images on this tool call". Persisted via snapshot so a
   * frontend reconnecting mid-turn or after refresh sees the same image.
   */
  images: ToolCallImage[]
}

export interface PendingPermission {
  request_id: string
  tool_call: unknown
  options: PermissionOptionInfo[]
}

/** In-flight user prompt carried on a connection (from a `user_message` event
 *  or a snapshot's `pending_user_message`). Mirrored into the runtime as a
 *  synthesized user turn for cross-client VIEWERS so they see the sender's
 *  message (the sender renders its own optimistic turn and ignores it). */
export interface PendingUserMessage {
  messageId: string
  blocks: UserMessageBlock[]
}

export interface PendingQuestion {
  tool_call_id: string
  question: string
}

export interface ClaudeApiRetryState {
  sessionId: string
  attempt: number | null
  maxRetries: number | null
  error: string | null
  errorStatus: number | null
  retryDelayMs: number | null
}

export type LiveContentBlock =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "plan"; entries: PlanEntryInfo[] }
  | { type: "tool_call"; info: ToolCallInfo }

export interface LiveMessage {
  id: string
  role: "assistant" | "tool"
  content: LiveContentBlock[]
  startedAt: number
}

// ── Per-connection state ──

export interface ConnectionState {
  connectionId: string
  contextKey: string
  agentType: AgentType
  workingDir: string | null
  status: ConnectionStatus
  promptCapabilities: PromptCapabilitiesInfo
  supportsFork: boolean
  selectorsReady: boolean
  sessionId: string | null
  modes: SessionModeStateInfo | null
  configOptions: SessionConfigOptionInfo[] | null
  availableCommands: AvailableCommandInfo[] | null
  usage: SessionUsageUpdateInfo | null
  liveMessage: LiveMessage | null
  pendingPermission: PendingPermission | null
  /** In-flight user prompt for the current turn — set from a `user_message`
   *  event or a snapshot's `pending_user_message`. A VIEWER mirrors this into
   *  the runtime as a synthesized user turn; `null` outside an active turn. */
  pendingUserMessage: PendingUserMessage | null
  pendingQuestion: PendingQuestion | null
  /** Awaiting-answer multiple-choice `ask_user_question` (the codeg-mcp blocking
   *  tool). Set from a `question_request` event or a snapshot's
   *  `pending_question`; cleared on `question_resolved` or turn end. Distinct
   *  from the free-text `pendingQuestion` above. */
  pendingAskQuestion: PendingQuestionState | null
  claudeApiRetry: ClaudeApiRetryState | null
  error: string | null
  /**
   * Set when the agent rejected `session/load` non-recoverably (currently
   * only `Resource not found` for an expired/missing historical session).
   * Distinct from `error` because the UI surfaces it inline in the message
   * list with reload / new-conversation actions, instead of as a toast.
   * Cleared on the next CONNECTION_CREATED for the same key, or by
   * CLEAR_ACP_LOAD_ERROR (Reload button).
   */
  loadError: string | null
  /**
   * Highest envelope.seq applied to this connection. Used to dedup the
   * live `acp://event` stream against the snapshot endpoint: a
   * HYDRATE_FROM_SNAPSHOT sets this to snapshot.event_seq, and incoming
   * envelopes with seq <= lastAppliedSeq are dropped as duplicates.
   * Phase 3b initialises to 0 on CONNECTION_CREATED.
   */
  lastAppliedSeq: number
  /**
   * True when this entry was synthesized for a backend connection that
   * was spawned by the delegation broker (not via a user-driven
   * `connect()`). Such entries piggy-back on the same reducer pipeline
   * as real connections so the child's live message, tool calls, and
   * permission requests reach the UI, but they MUST be hidden from any
   * user-facing connection list / picker, and they MUST NOT be reaped
   * by the idle sweep — their lifetime is governed by the parent's
   * delegation_started / delegation_completed events.
   */
  isDelegationChild: boolean
  /**
   * For delegation-child entries: the parent's `tool_use_id` that owns
   * this child. The DelegatedSubThread component uses this to resolve
   * the child connection state from its parent-side identifier. Null
   * for non-delegation connections.
   */
  parentToolUseId: string | null
  /**
   * For delegation-child entries: the parent connection that spawned
   * this child. Carried for diagnostic / cascade-cancel purposes; not
   * required for the rendering path. Null for non-delegation
   * connections.
   */
  parentConnectionId: string | null
  /**
   * True when this client did NOT spawn the backend connection but attached to
   * one another client already owns (cross-client live streaming, discovered
   * via `acp_find_connection_for_conversation`). A viewer is a NON-OWNING,
   * co-controlling client: it streams the same turn and MAY drive the shared
   * agent (sendPrompt/cancel target the owner's connection, serialized
   * server-side by its prompt_lock; turn-level concurrency rejection is a
   * tracked follow-up). The one hard invariant: on teardown a viewer MUST
   * detach (drop its attach subscription / reverse-map entry) and MUST NOT
   * `acpDisconnect` — that would kill the agent for the owner. Like
   * `isDelegationChild`, viewers are skipped by the idle sweep's disconnect
   * path. Distinct from `isDelegationChild` (broker-owned child bookkeeping);
   * a plain viewer is the lighter cousin with no delegation state.
   */
  isViewer: boolean
  /**
   * True when the agent's effective settings changed after this session was
   * spawned, so the running process is still on its launch-time config (env
   * vars / model provider / native config). Set from a `session_config_stale`
   * event or a hydrated snapshot; cleared when the user reverts the setting or
   * restarts the session via `reapplyConfig`. Drives the per-conversation
   * "restart to apply" banner.
   */
  configStale: boolean
  /** Which settings surface drifted, for the banner's wording. `null` when not stale. */
  configStaleKind: ConfigStaleKind | null
  /**
   * Launched-but-unresolved background tasks (async sub-agents / background
   * shells) on this connection, mirrored from `background_activity` events
   * (authoritative accounting lives in the backend transcript watcher).
   * Non-zero exempts the connection from the frontend idle sweep — killing
   * the connection kills the agent CLI and the background work with it —
   * and drives the "background tasks running" chip.
   */
  backgroundOutstanding: number
  /**
   * Epoch ms of the most recent `background_activity` event that settled a
   * task, cleared when the follow-up overlay turns start arriving. Bridges
   * the otherwise-blank gap between "N tasks running" disappearing and the
   * agent's reaction to the results surfacing (model time-to-first-block +
   * the transcript's record granularity — typically 3–15s): the chip shows a
   * "syncing results" state while this is set. Client-local UI state (not in
   * the snapshot); the chip additionally expires it from display after a
   * fixed window so a killed CLI can't strand the indicator.
   */
  backgroundSettleSyncingSince: number | null
  /**
   * Tool-call context observed OUT-OF-TURN (status !== "prompting"), kept
   * ONLY so a background permission request can still render its command/
   * diff details. Out-of-turn wire tool events are barred from `liveMessage`
   * (the transcript overlay renders that content), but the permission dialog
   * enriches from the live tool registry — this small bounded map
   * (`OUT_OF_TURN_TOOL_CALL_CAP` newest entries) is that registry's
   * out-of-turn stand-in. Cleared when the next prompting turn starts.
   * `null` when empty (the common case allocates nothing).
   */
  outOfTurnToolCalls: ReadonlyMap<string, ToolCallInfo> | null
  /**
   * Client-local: the user dismissed (X) the stale banner for the CURRENT
   * drift. Hides the banner without touching the underlying `configStale`
   * state. Reset to `false` whenever a fresh `session_config_stale` arrives (a
   * new change re-shows the banner) and on a new connection. Never sourced from
   * the snapshot — dismissal is per-client UI state.
   */
  configStaleDismissed: boolean
}

type ConnectRequest = {
  agentType: AgentType
  workingDir?: string
  sessionId?: string
  // Persisted conversation id (when known) — drives the cross-client viewer
  // discovery gate in connect(). Not part of `sameConnectRequest` equality
  // (sessionId already distinguishes), but carried so a re-fired pending
  // request still runs discovery.
  conversationId?: number
}

function sameConnectRequest(a: ConnectRequest, b: ConnectRequest) {
  return (
    a.agentType === b.agentType &&
    (a.workingDir ?? null) === (b.workingDir ?? null) &&
    (a.sessionId ?? null) === (b.sessionId ?? null)
  )
}

// ── Reducer actions ──

type Action =
  | {
      type: "CONNECTION_CREATED"
      contextKey: string
      connectionId: string
      agentType: AgentType
      workingDir: string | null
      // Set when attaching to a connection another client owns (viewer).
      // Defaults to false (owner) when omitted.
      isViewer?: boolean
    }
  | {
      type: "HYDRATE_FROM_SNAPSHOT"
      contextKey: string
      patch: import("@/lib/snapshot-denormalize").SnapshotPatch
    }
  | { type: "CONNECTION_REMOVED"; contextKey: string }
  | { type: "REMOVE_ALL" }
  | { type: "REKEY_CONNECTION"; fromKey: string; toKey: string }
  | {
      type: "STATUS_CHANGED"
      contextKey: string
      status: ConnectionStatus
    }
  | {
      // Mirror of a `background_activity` event onto the connection: the
      // `outstanding` count (the backend transcript watcher's authoritative
      // accounting) plus whether this event settled tasks / carried overlay
      // turns, which drive the settle-syncing bridge state. No-op when
      // nothing it mirrors changed, so repeat events don't re-render
      // connection consumers. `outOfTurnSettleCount` counts only settles whose
      // reply arrives OUT OF TURN (a separate overlay turn) — i.e. NOT
      // wire-visible; those are the ones the "syncing results" hint bridges.
      type: "SET_BACKGROUND_OUTSTANDING"
      contextKey: string
      outstanding: number
      outOfTurnSettleCount: number
      turnsCount: number
    }
  | StreamingAction
  | { type: "STREAM_BATCH"; actions: StreamingAction[] }
  | {
      type: "TOOL_CALL"
      contextKey: string
      tool_call_id: string
      title: string
      kind: string
      status: string
      content: string | null
      raw_input: string | null
      raw_output: string | null
      locations: unknown
      meta: ToolCallMeta
      /** `null` when the wire event omitted the field (no images). */
      images: ToolCallImage[] | null
    }
  | {
      type: "TOOL_CALL_UPDATE"
      contextKey: string
      tool_call_id: string
      title: string | null
      fallback_title: string
      fallback_kind: string
      status: string | null
      content: string | null
      raw_input: string | null
      raw_output: string | null
      raw_output_append?: boolean
      locations: unknown
      meta: ToolCallMeta
      /**
       * `null` when the wire event omitted the field — preserve prior images.
       * `[]` (empty array) when the agent explicitly cleared images.
       * `[a, b]` to replace.
       */
      images: ToolCallImage[] | null
    }
  | {
      type: "BATCH_TOOL_CALL_UPDATES"
      actions: Array<{
        contextKey: string
        tool_call_id: string
        title: string | null
        fallback_title: string
        fallback_kind: string
        status: string | null
        content: string | null
        raw_input: string | null
        raw_output: string | null
        raw_output_append?: boolean
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        locations: any | null
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        meta: any | null
        images: ToolCallImage[] | null
      }>
    }
  | {
      type: "PERMISSION_REQUEST"
      contextKey: string
      request_id: string
      tool_call: unknown
      fallback_title: string
      fallback_kind: string
      options: PermissionOptionInfo[]
    }
  | {
      type: "PERMISSION_CLEARED"
      contextKey: string
      /**
       * When present, only clear if the current pendingPermission's request_id
       * matches. Guards against a late `permission_resolved` event wiping out a
       * fresh permission that was raised between resolve and dispatch.
       * Omit for unconditional clears (e.g. cancel paths).
       */
      requestId?: string
    }
  | {
      type: "SET_PENDING_QUESTION"
      contextKey: string
      pendingQuestion: PendingQuestion
    }
  | { type: "CLEAR_PENDING_QUESTION"; contextKey: string }
  | {
      type: "SET_ASK_QUESTION"
      contextKey: string
      pendingAskQuestion: PendingQuestionState
    }
  | {
      type: "CLEAR_ASK_QUESTION"
      contextKey: string
      /** When present, only clear if the current question_id matches (guards a
       *  late `question_resolved` from wiping a freshly-raised question). */
      questionId?: string
    }
  | { type: "SESSION_STARTED"; contextKey: string; sessionId: string }
  | {
      type: "SESSION_MODES"
      contextKey: string
      modes: SessionModeStateInfo
    }
  | {
      type: "SESSION_CONFIG_OPTIONS"
      contextKey: string
      configOptions: SessionConfigOptionInfo[]
    }
  | {
      type: "CONFIG_STALE_CHANGED"
      contextKey: string
      stale: boolean
      kind: ConfigStaleKind
    }
  | {
      type: "DISMISS_CONFIG_STALE"
      contextKey: string
    }
  | {
      type: "SELECTORS_READY"
      contextKey: string
    }
  | {
      type: "PROMPT_CAPABILITIES"
      contextKey: string
      promptCapabilities: PromptCapabilitiesInfo
    }
  | {
      type: "FORK_SUPPORTED"
      contextKey: string
      supported: boolean
    }
  | { type: "MODE_CHANGED"; contextKey: string; modeId: string }
  | {
      type: "CONFIG_OPTION_CHANGED"
      contextKey: string
      configId: string
      valueId: string
    }
  | {
      type: "PLAN_UPDATE"
      contextKey: string
      entries: PlanEntryInfo[]
    }
  | {
      type: "CLAUDE_API_RETRY"
      contextKey: string
      retry: ClaudeApiRetryState | null
    }
  | { type: "ERROR"; contextKey: string; message: string }
  | { type: "ACP_LOAD_ERROR"; contextKey: string; message: string }
  | { type: "CLEAR_ACP_LOAD_ERROR"; contextKey: string }
  | {
      type: "AVAILABLE_COMMANDS"
      contextKey: string
      commands: AvailableCommandInfo[]
    }
  | {
      type: "USAGE_UPDATE"
      contextKey: string
      usage: SessionUsageUpdateInfo
    }
  | {
      type: "EVENT_APPLIED"
      contextKey: string
      seq: number
    }
  | {
      /**
       * Synthesize a ConnectionState for a delegation-spawned child so
       * its acp://event stream lands in the reducer the same way a
       * user-driven connect() does. contextKey == connectionId for these
       * entries — the child has no user-facing tab to anchor a separate
       * key against.
       */
      type: "DELEGATION_CHILD_ATTACH"
      contextKey: string
      connectionId: string
      agentType: AgentType
      parentConnectionId: string
      parentToolUseId: string
    }
  | {
      /**
       * Remove the synthetic child entry once the delegation has wound
       * down (delegation_completed) and any grace window has elapsed.
       * No-op when the entry is already gone.
       */
      type: "DELEGATION_CHILD_DETACH"
      contextKey: string
    }

type StreamingAction =
  | { type: "CONTENT_DELTA"; contextKey: string; text: string }
  | { type: "THINKING"; contextKey: string; text: string }

type ConnectionsMap = Map<string, ConnectionState>
const MAX_LIVE_TOOL_RAW_OUTPUT_CHARS = 200_000
const MAX_BUFFERED_UNMAPPED_EVENTS_PER_CONNECTION = 64
const MAX_BUFFERED_UNMAPPED_CONNECTIONS = 128

// Per-agentType cache for selectors (modes / configOptions).
// Populated when real data arrives from the backend.
// Used as UI-layer fallback when the connection hasn't received real data yet.
const selectorsCache = new Map<
  string,
  {
    modes: SessionModeStateInfo | null
    configOptions: SessionConfigOptionInfo[] | null
  }
>()

export function getCachedSelectors(agentType: string) {
  return selectorsCache.get(agentType) ?? null
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null
  }
  return value as Record<string, unknown>
}

const PERMISSION_TOOL_INPUT_KEYS = [
  "rawInput",
  "raw_input",
  "input",
  "arguments",
  "params",
  "payload",
] as const

function asFiniteNumber(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value
  }
  if (typeof value === "string" && value.trim().length > 0) {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function parseClaudeApiRetryEvent(
  event: Extract<AcpEvent, { type: "claude_sdk_message" }>
): ClaudeApiRetryState | null {
  const message = asRecord(event.message)
  if (!message) return null
  if (message.type !== "system" || message.subtype !== "api_retry") return null

  return {
    sessionId:
      typeof message.session_id === "string"
        ? message.session_id
        : event.session_id,
    attempt: asFiniteNumber(message.attempt),
    maxRetries: asFiniteNumber(message.max_retries),
    error: typeof message.error === "string" ? message.error : null,
    errorStatus: asFiniteNumber(message.error_status),
    retryDelayMs: asFiniteNumber(message.retry_delay_ms),
  }
}

function extractPermissionToolCallId(toolCall: unknown): string | null {
  const record = asRecord(toolCall)
  if (!record) return null
  const candidates = [
    record.call_id,
    record.callId,
    record.tool_call_id,
    record.toolCallId,
    record.id,
  ]
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim().length > 0) {
      return candidate
    }
  }
  return null
}

function pickPermissionToolInput(record: Record<string, unknown>): unknown {
  for (const key of PERMISSION_TOOL_INPUT_KEYS) {
    const value = record[key]
    if (value === undefined || value === null) continue
    if (typeof value === "string" && value.trim().length === 0) continue
    return value
  }
  return null
}

function serializePermissionInput(value: unknown): string | null {
  if (value === undefined || value === null) return null
  if (typeof value === "string") {
    return value.trim().length > 0 ? value : null
  }
  try {
    return JSON.stringify(value)
  } catch {
    return null
  }
}

function serializePermissionToolCall(toolCall: unknown): string | null {
  const record = asRecord(toolCall)
  if (!record) return null
  try {
    // Extract the actual tool input rather than serializing the entire
    // permission wrapper (which includes internal fields like kind/status/id).
    const nestedInput = pickPermissionToolInput(record)
    const serializedNestedInput = serializePermissionInput(nestedInput)
    if (serializedNestedInput) return serializedNestedInput

    // Fallback: strip wrapper-only fields to avoid rendering internal
    // permission structure as raw text.
    const wrapperKeys = new Set([
      "content",
      "kind",
      "status",
      "title",
      "toolCallId",
      "tool_call_id",
      "callId",
      "call_id",
      ...PERMISSION_TOOL_INPUT_KEYS,
    ])
    const rest: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(record)) {
      if (!wrapperKeys.has(k)) rest[k] = v
    }
    return Object.keys(rest).length > 0 ? JSON.stringify(rest) : null
  } catch {
    return null
  }
}

function findLiveToolCallInfo(
  content: LiveContentBlock[],
  toolCallId: string | null
): ToolCallInfo | null {
  if (!toolCallId) return null
  const block = content.find(
    (item) => item.type === "tool_call" && item.info.tool_call_id === toolCallId
  )
  return block?.type === "tool_call" ? block.info : null
}

function mergePermissionToolCallWithLiveInfo(
  toolCall: unknown,
  liveInfo: ToolCallInfo | null
): unknown {
  if (!liveInfo) return toolCall

  const rawInput = serializePermissionInput(liveInfo.raw_input)
  const record = asRecord(toolCall)
  if (!record) {
    if (!rawInput) return toolCall
    return {
      toolCallId: liveInfo.tool_call_id,
      title: liveInfo.title,
      kind: liveInfo.kind,
      rawInput,
    }
  }

  const next = { ...record }
  let changed = false
  const existingInput = serializePermissionInput(pickPermissionToolInput(next))
  if (!existingInput && rawInput) {
    next.rawInput = rawInput
    changed = true
  }
  if (typeof next.title !== "string" || next.title.trim().length === 0) {
    next.title = liveInfo.title
    changed = true
  }
  if (typeof next.kind !== "string" || next.kind.trim().length === 0) {
    next.kind = liveInfo.kind
    changed = true
  }
  if (!extractPermissionToolCallId(next)) {
    next.toolCallId = liveInfo.tool_call_id
    changed = true
  }
  return changed ? next : toolCall
}

function mergePendingPermissionWithLiveInfo(
  pendingPermission: PendingPermission | null,
  liveInfo: ToolCallInfo | null
): PendingPermission | null {
  if (!pendingPermission || !liveInfo) return pendingPermission
  const permissionCallId = extractPermissionToolCallId(
    pendingPermission.tool_call
  )
  if (permissionCallId !== liveInfo.tool_call_id) return pendingPermission

  const toolCall = mergePermissionToolCallWithLiveInfo(
    pendingPermission.tool_call,
    liveInfo
  )
  if (toolCall === pendingPermission.tool_call) return pendingPermission
  return {
    ...pendingPermission,
    tool_call: toolCall,
  }
}

function mergePendingPermissionWithLiveMessage(
  pendingPermission: PendingPermission | null,
  liveMessage: LiveMessage | null
): PendingPermission | null {
  const permissionCallId = extractPermissionToolCallId(
    pendingPermission?.tool_call
  )
  const liveInfo = liveMessage
    ? findLiveToolCallInfo(liveMessage.content, permissionCallId)
    : null
  return mergePendingPermissionWithLiveInfo(pendingPermission, liveInfo)
}

function extractPermissionToolTitle(toolCall: unknown): string | null {
  const record = asRecord(toolCall)
  if (!record) return null
  const candidates = [record.title, record.tool_name, record.name, record.type]
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim().length > 0) {
      return candidate
    }
  }
  return null
}

function extractPermissionToolKind(toolCall: unknown): string | null {
  const record = asRecord(toolCall)
  if (!record) return null
  const candidates = [record.kind, record.tool_name, record.name, record.type]
  for (const candidate of candidates) {
    if (typeof candidate === "string" && candidate.trim().length > 0) {
      return candidate
    }
  }
  return null
}

/**
 * Extract the free-text question for the LEGACY `QuestionDialog` from a tool
 * call's raw input — gated on a singular `question` STRING field. Exported so a
 * regression test can prove the new multiple-choice `ask_user_question` tool
 * (whose input is `{ questions: [...] }`, plural array) never trips this legacy
 * path even though tool-name normalization classifies it as "question".
 */
export function extractQuestionText(rawInput: string | null): string | null {
  if (!rawInput) return null
  try {
    const parsed = JSON.parse(rawInput)
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof parsed.question === "string"
    ) {
      return parsed.question
    }
  } catch {
    // not JSON, try using rawInput as-is if it looks like a question
  }
  return null
}

function sameModes(
  a: SessionModeStateInfo | null,
  b: SessionModeStateInfo
): boolean {
  if (a === b) return true
  if (!a) return false
  if (a.current_mode_id !== b.current_mode_id) return false
  if (a.available_modes.length !== b.available_modes.length) return false
  for (let i = 0; i < a.available_modes.length; i += 1) {
    const left = a.available_modes[i]
    const right = b.available_modes[i]
    if (
      left.id !== right.id ||
      left.name !== right.name ||
      left.description !== right.description
    ) {
      return false
    }
  }
  return true
}

function samePromptCapabilities(
  a: PromptCapabilitiesInfo,
  b: PromptCapabilitiesInfo
): boolean {
  return (
    a.image === b.image &&
    a.audio === b.audio &&
    a.embedded_context === b.embedded_context
  )
}

function samePlanEntries(a: PlanEntryInfo[], b: PlanEntryInfo[]): boolean {
  if (a === b) return true
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) {
    if (
      a[i].content !== b[i].content ||
      a[i].priority !== b[i].priority ||
      a[i].status !== b[i].status
    ) {
      return false
    }
  }
  return true
}

function sameConfigOptions(
  a: SessionConfigOptionInfo[] | null,
  b: SessionConfigOptionInfo[]
): boolean {
  if (a === b) return true
  if (!a) return false
  if (a.length !== b.length) return false

  for (let i = 0; i < a.length; i += 1) {
    const left = a[i]
    const right = b[i]
    if (
      left.id !== right.id ||
      left.name !== right.name ||
      left.description !== right.description ||
      left.category !== right.category
    ) {
      return false
    }

    const leftKind = left.kind
    const rightKind = right.kind
    if (leftKind.type !== rightKind.type) return false

    if (leftKind.type === "select") {
      if (leftKind.current_value !== rightKind.current_value) return false
      if (leftKind.options.length !== rightKind.options.length) return false
      if (leftKind.groups.length !== rightKind.groups.length) return false

      for (let j = 0; j < leftKind.options.length; j += 1) {
        const lo = leftKind.options[j]
        const ro = rightKind.options[j]
        if (
          lo.value !== ro.value ||
          lo.name !== ro.name ||
          lo.description !== ro.description
        ) {
          return false
        }
      }

      for (let j = 0; j < leftKind.groups.length; j += 1) {
        const lg = leftKind.groups[j]
        const rg = rightKind.groups[j]
        if (lg.group !== rg.group || lg.name !== rg.name) return false
        if (lg.options.length !== rg.options.length) return false
        for (let k = 0; k < lg.options.length; k += 1) {
          const lgo = lg.options[k]
          const rgo = rg.options[k]
          if (
            lgo.value !== rgo.value ||
            lgo.name !== rgo.name ||
            lgo.description !== rgo.description
          ) {
            return false
          }
        }
      }
    }
  }
  return true
}

function sameCommands(
  a: AvailableCommandInfo[] | null,
  b: AvailableCommandInfo[]
): boolean {
  if (a === b) return true
  if (!a) return false
  if (a.length !== b.length) return false
  for (let i = 0; i < a.length; i += 1) {
    if (
      a[i].name !== b[i].name ||
      a[i].description !== b[i].description ||
      a[i].input_hint !== b[i].input_hint
    ) {
      return false
    }
  }
  return true
}

function dedupeCommandsByName(
  commands: AvailableCommandInfo[]
): AvailableCommandInfo[] {
  const seen = new Set<string>()
  let deduped: AvailableCommandInfo[] | null = null

  for (let i = 0; i < commands.length; i += 1) {
    const command = commands[i]
    if (seen.has(command.name)) {
      deduped ??= commands.slice(0, i)
      continue
    }

    seen.add(command.name)
    deduped?.push(command)
  }

  return deduped ?? commands
}

/**
 * Lazy-create a `LiveMessage` shell mirroring the backend's
 * `ensure_live_message` semantic. Required because the backend only
 * initializes `session_state.live_message` when the first `ContentDelta` /
 * `Thinking` / `ToolCall` / `PlanUpdate` arrives — there's a window between
 * `StatusChanged(Prompting)` and the first content event in which the
 * snapshot reports `live_message: null`. After a browser refresh inside
 * that window, the live `STATUS_CHANGED(prompting)` event won't re-fire
 * (status is already prompting in the snapshot), so without this fallback
 * the reducer would drop every subsequent delta / tool call / plan update.
 */
function ensureLiveMessage(prev: LiveMessage | null): LiveMessage {
  if (prev) return prev
  return {
    id: randomUUID(),
    role: "assistant",
    content: [],
    startedAt: Date.now(),
  }
}

/** Last time an out-of-turn drop was logged — module-level sampling clock. */
let lastOutOfTurnDropLogAt = 0

function applyStreamingAction(
  conn: ConnectionState,
  action: StreamingAction
): ConnectionState | null {
  // OUT-OF-TURN guard: the backend's idle loop forwards session/updates that
  // arrive BETWEEN turns (background sub-agent completions, the agent's
  // continued autonomous work). Appending those here would graft them onto
  // the previous turn's completed liveMessage — the historical "background
  // results render garbled/incomplete" bug. The transcript watcher's
  // `background_activity` overlay is the single render path for out-of-turn
  // content, so wire deltas outside a prompting turn are dropped. Ordering is
  // safe: the backend emits StatusChanged(prompting) before any turn content,
  // and turn_complete flushes queued deltas before flipping status back.
  if (conn.status !== "prompting") {
    // Sampled: an autonomous (cron//loop) turn streams the ENTIRE wire
    // out-of-turn — logging every dropped delta would spam the console and
    // allocate per token for minutes at a time.
    const now = Date.now()
    if (now - lastOutOfTurnDropLogAt > 5_000) {
      lastOutOfTurnDropLogAt = now
      console.debug(
        "[acp] dropping out-of-turn streaming deltas (transcript overlay renders them)",
        { contextKey: conn.contextKey, type: action.type }
      )
    }
    return null
  }
  // CONTENT_DELTA with empty text is a true no-op. THINKING with empty text
  // is allowed to create the initial placeholder block so the UI can show
  // a "Thinking..." indicator immediately (and for newer Claude models that
  // redact thinking text entirely, keeping the empty block as the signal).
  if (action.type === "CONTENT_DELTA" && action.text.length === 0) return null

  const prev = ensureLiveMessage(conn.liveMessage)
  const lastBlock = prev.content[prev.content.length - 1]
  let newContent: LiveContentBlock[] | null = null

  if (action.type === "CONTENT_DELTA") {
    if (lastBlock?.type === "text") {
      newContent = [
        ...prev.content.slice(0, -1),
        { type: "text", text: lastBlock.text + action.text },
      ]
    } else {
      newContent = [...prev.content, { type: "text", text: action.text }]
    }
  } else {
    if (action.text.length === 0 && lastBlock?.type === "thinking") {
      // Already have a thinking block; an empty follow-up event is a no-op.
      return null
    }
    if (lastBlock?.type === "thinking") {
      newContent = [
        ...prev.content.slice(0, -1),
        { type: "thinking", text: lastBlock.text + action.text },
      ]
    } else {
      newContent = [...prev.content, { type: "thinking", text: action.text }]
    }
  }

  if (!newContent) return null
  return {
    ...conn,
    liveMessage: { ...prev, content: newContent },
    // Streaming content implies the SDK has recovered from any in-flight
    // Claude API retry, so hide the retry banner immediately instead of
    // waiting for the prompt cycle to end.
    claudeApiRetry: null,
  }
}

/** Newest out-of-turn tool-call contexts kept per connection (see
 *  `ConnectionState.outOfTurnToolCalls`). Permission enrichment only ever
 *  needs the last few. */
const OUT_OF_TURN_TOOL_CALL_CAP = 8

/**
 * Overlay-fold refetch: once a conversation's background overlay exceeds this
 * many turns, fold them into persisted turns via a detail refetch (the
 * watermark rule retires covered entries). Keeps a day-long cron//loop
 * session's overlay — which never settles, so nothing else refetches —
 * bounded. Sized so the fold runs every few dozen autonomous turns, not per
 * turn.
 */
const OVERLAY_FOLD_THRESHOLD = 60
/** Floor between overlay-fold refetches per conversation, so a failing
 *  backend (refetch errors, overlay keeps growing) can't escalate into a
 *  refetch per background event. */
const OVERLAY_FOLD_MIN_INTERVAL_MS = 30_000
/** conversationId → epoch ms of the last overlay-fold refetch. Module-level:
 *  survives provider re-renders; a few entries only (conversations with
 *  active background overlay). */
const overlayFoldRefetchAt = new Map<number, number>()

/** Upsert one out-of-turn tool-call info into the bounded registry,
 *  evicting the oldest entry past the cap. Returns a fresh map. */
function recordOutOfTurnToolCall(
  existing: ReadonlyMap<string, ToolCallInfo> | null,
  info: ToolCallInfo
): ReadonlyMap<string, ToolCallInfo> {
  const next = new Map(existing ?? [])
  next.delete(info.tool_call_id)
  next.set(info.tool_call_id, info)
  while (next.size > OUT_OF_TURN_TOOL_CALL_CAP) {
    const oldest = next.keys().next().value
    if (oldest === undefined) break
    next.delete(oldest)
  }
  return next
}

function connectionsReducer(
  state: ConnectionsMap,
  action: Action
): ConnectionsMap {
  switch (action.type) {
    case "CONNECTION_CREATED": {
      const next = new Map(state)
      next.set(action.contextKey, {
        connectionId: action.connectionId,
        contextKey: action.contextKey,
        agentType: action.agentType,
        workingDir: action.workingDir,
        status: "connecting",
        promptCapabilities: {
          image: false,
          audio: false,
          embedded_context: false,
        },
        supportsFork: false,
        selectorsReady: false,
        sessionId: null,
        modes: null,
        configOptions: null,
        availableCommands: null,
        usage: null,
        liveMessage: null,
        pendingPermission: null,
        pendingUserMessage: null,
        pendingQuestion: null,
        pendingAskQuestion: null,
        claudeApiRetry: null,
        error: null,
        loadError: null,
        lastAppliedSeq: 0,
        isDelegationChild: false,
        parentToolUseId: null,
        parentConnectionId: null,
        isViewer: action.isViewer ?? false,
        configStale: false,
        configStaleKind: null,
        configStaleDismissed: false,
        backgroundOutstanding: 0,
        backgroundSettleSyncingSince: null,
        outOfTurnToolCalls: null,
      })
      return next
    }

    case "DELEGATION_CHILD_ATTACH": {
      // Idempotent: if an entry already exists for this key with the
      // same connectionId, leave it untouched so a duplicate
      // delegation_started (e.g. replayed from snapshot hydration after
      // a refresh) doesn't blow away the live stream that has already
      // populated. If the connectionId differs we replace, since a new
      // spawn won the race.
      const existing = state.get(action.contextKey)
      if (existing && existing.connectionId === action.connectionId) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        connectionId: action.connectionId,
        contextKey: action.contextKey,
        agentType: action.agentType,
        workingDir: null,
        // The child is already alive in the backend by the time
        // delegation_started fires; treat it as connected so any UI
        // surface that gates on status reflects reality.
        status: "connected",
        promptCapabilities: {
          image: false,
          audio: false,
          embedded_context: false,
        },
        supportsFork: false,
        selectorsReady: true,
        sessionId: null,
        modes: null,
        configOptions: null,
        availableCommands: null,
        usage: null,
        liveMessage: null,
        pendingPermission: null,
        pendingUserMessage: null,
        pendingQuestion: null,
        pendingAskQuestion: null,
        claudeApiRetry: null,
        error: null,
        loadError: null,
        lastAppliedSeq: 0,
        isDelegationChild: true,
        parentToolUseId: action.parentToolUseId,
        parentConnectionId: action.parentConnectionId,
        isViewer: false,
        configStale: false,
        configStaleKind: null,
        configStaleDismissed: false,
        backgroundOutstanding: 0,
        backgroundSettleSyncingSince: null,
        outOfTurnToolCalls: null,
      })
      return next
    }

    case "DELEGATION_CHILD_DETACH": {
      const existing = state.get(action.contextKey)
      if (!existing || !existing.isDelegationChild) return state
      const next = new Map(state)
      next.delete(action.contextKey)
      return next
    }

    case "HYDRATE_FROM_SNAPSHOT": {
      const current = state.get(action.contextKey)
      if (!current) return state
      // Identity guard: the connection at this contextKey may have been
      // disconnected and replaced between the snapshot fetch firing and
      // its async response. eventSeq alone is not enough — a stale snapshot
      // from connection A (high seq) would otherwise overwrite a fresh
      // connection B (lastAppliedSeq=0) at the same contextKey.
      if (current.connectionId !== action.patch.connectionId) return state

      // Latched-once / fill-null fields are always safe to merge, even when
      // the snapshot is stale by event_seq. Their producing events
      // (`selectors_ready`, `fork_supported`, `session_modes`,
      // `session_config_options`, `available_commands`, `prompt_capabilities`)
      // typically fire only once during the initial handshake, so the
      // snapshot is the only recovery path after a refresh that missed the
      // original live event. Without this, a mid-stream browser refresh
      // races the snapshot fetch against new content_delta events: the
      // deltas advance lastAppliedSeq past the snapshot's event_seq, the
      // outer guard rejects the patch, and `selectorsReady` never recovers
      // — leaving the bottom status bar stuck on "正在初始化 xxx 会话".
      const mergedSelectorsReady =
        action.patch.selectorsReady || current.selectorsReady
      const mergedSupportsFork =
        action.patch.supportsFork || current.supportsFork
      const mergedModes = current.modes ?? action.patch.modes
      const mergedConfigOptions =
        current.configOptions ?? action.patch.configOptions
      const mergedAvailableCommands =
        current.availableCommands ?? action.patch.availableCommands
      const mergedPromptCapabilities =
        action.patch.promptCapabilities ?? current.promptCapabilities

      // Race guard: the snapshot may have been generated BEFORE events
      // that have since arrived and been applied to in-memory state.
      // Mutable fields (status, sessionId, liveMessage, pendingPermission,
      // usage, error) are fresher in memory than in the snapshot and must NOT
      // be overwritten — but the latched/fill-null fields above are still
      // applied so the once-per-lifetime bits can recover. `error` in
      // particular is cleared on a new prompt (STATUS_CHANGED → prompting), so
      // folding a stale snapshot's `lastError` back in here would resurrect an
      // error the current turn already cleared; it is recovered on the fresh
      // path below instead.
      if (action.patch.eventSeq <= current.lastAppliedSeq) {
        if (
          mergedSelectorsReady === current.selectorsReady &&
          mergedSupportsFork === current.supportsFork &&
          mergedModes === current.modes &&
          mergedConfigOptions === current.configOptions &&
          mergedAvailableCommands === current.availableCommands &&
          mergedPromptCapabilities === current.promptCapabilities
        ) {
          return state
        }
        const next = new Map(state)
        next.set(action.contextKey, {
          ...current,
          modes: mergedModes,
          configOptions: mergedConfigOptions,
          availableCommands: mergedAvailableCommands,
          promptCapabilities: mergedPromptCapabilities,
          selectorsReady: mergedSelectorsReady,
          supportsFork: mergedSupportsFork,
        })
        return next
      }

      const hydratedLiveMessage = action.patch.liveMessage
      const hydratedPendingPermission = mergePendingPermissionWithLiveMessage(
        action.patch.pendingPermission,
        hydratedLiveMessage ?? current.liveMessage
      )
      const next = new Map(state)
      next.set(action.contextKey, {
        ...current,
        status: action.patch.status,
        sessionId: action.patch.sessionId,
        modes: action.patch.modes,
        configOptions: action.patch.configOptions,
        availableCommands: action.patch.availableCommands,
        usage: action.patch.usage,
        liveMessage: hydratedLiveMessage,
        pendingPermission: hydratedPendingPermission,
        pendingAskQuestion: action.patch.pendingAskQuestion,
        pendingUserMessage: action.patch.pendingUserMessage,
        promptCapabilities: mergedPromptCapabilities,
        selectorsReady: mergedSelectorsReady,
        supportsFork: mergedSupportsFork,
        // Staleness is a current-state field (like status): apply the snapshot's
        // value on the fresh path. `configStaleDismissed` is client-local and
        // preserved via `...current`.
        configStale: action.patch.configStale,
        configStaleKind: action.patch.configStaleKind,
        // Current-state field like `status`: a client attaching mid-episode
        // recovers the pending-background count the one-shot events won't
        // replay for it (sweep exemption + chip).
        backgroundOutstanding: action.patch.backgroundOutstanding,
        error: action.patch.lastError,
        lastAppliedSeq: action.patch.eventSeq,
      })
      return next
    }

    case "EVENT_APPLIED": {
      const current = state.get(action.contextKey)
      if (!current) return state
      // Idempotent: only advances if the new seq is strictly higher.
      if (action.seq <= current.lastAppliedSeq) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...current,
        lastAppliedSeq: action.seq,
      })
      return next
    }

    case "CONNECTION_REMOVED": {
      const next = new Map(state)
      next.delete(action.contextKey)
      return next
    }

    case "REMOVE_ALL":
      return new Map()

    case "REKEY_CONNECTION": {
      const conn = state.get(action.fromKey)
      if (!conn) return state
      // Defensive: if toKey already has an entry, do not clobber it.
      if (state.has(action.toKey)) return state
      const next = new Map(state)
      next.delete(action.fromKey)
      next.set(action.toKey, { ...conn, contextKey: action.toKey })
      return next
    }

    case "STATUS_CHANGED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      const updated = { ...conn, status: action.status }
      if (action.status === "prompting") {
        updated.liveMessage = {
          id: randomUUID(),
          role: "assistant",
          content: [],
          startedAt: Date.now(),
        }
        updated.pendingQuestion = null
        updated.claudeApiRetry = null
        updated.error = null
        // The out-of-turn window ended: its tool-call contexts (kept only for
        // background permission enrichment) are stale for the new turn.
        updated.outOfTurnToolCalls = null
      } else if (conn.status === "prompting") {
        // Prompt cycle ended: clear in-flight Claude API retry banner.
        updated.claudeApiRetry = null
        // A blocked ask_user_question can't outlive its turn. The normal path
        // clears it via `question_resolved`; this is the safety net for a turn
        // that ended without one (agent error / abandoned block).
        updated.pendingAskQuestion = null
      }
      next.set(action.contextKey, updated)
      return next
    }

    case "SET_BACKGROUND_OUTSTANDING": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      // Settle-syncing bridge: a settlement whose reply arrives OUT OF TURN
      // (as a separate overlay turn) means that reply is being generated — arm
      // the "syncing results" hint to fill the gap until it surfaces. The
      // backend classifies this per settle via `wire_visible` (folded into
      // `outOfTurnSettleCount` by the handler): under claude-agent-acp #870 the
      // launching turn is held OPEN and the reply streams LIVE as its tail —
      // already on screen, no gap to bridge — so those are excluded. Arming for
      // a held settle would STRAND the hint: its reply never arrives as an
      // overlay `turns` event, so nothing disarms it and it sits until the 30s
      // cap (the "结果都出来了还显示 Syncing" bug). Using the backend flag rather
      // than the connection's current status is deliberate — it's correct even
      // when the watcher reads the settlement after the turn already fell back
      // to `connected`. The first genuinely out-of-turn `turns` event disarms.
      const syncingSince =
        action.outOfTurnSettleCount > 0
          ? Date.now()
          : action.turnsCount > 0
            ? null
            : conn.backgroundSettleSyncingSince
      if (
        conn.backgroundOutstanding === action.outstanding &&
        conn.backgroundSettleSyncingSince === syncingSince
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        backgroundOutstanding: action.outstanding,
        backgroundSettleSyncingSince: syncingSince,
      })
      return next
    }

    case "CONTENT_DELTA":
    case "THINKING": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const updated = applyStreamingAction(conn, action)
      if (!updated) return state
      const next = new Map(state)
      next.set(action.contextKey, updated)
      return next
    }

    case "STREAM_BATCH": {
      if (action.actions.length === 0) return state
      const grouped = new Map<string, StreamingAction[]>()
      for (const streamAction of action.actions) {
        const list = grouped.get(streamAction.contextKey)
        if (list) {
          list.push(streamAction)
        } else {
          grouped.set(streamAction.contextKey, [streamAction])
        }
      }

      let next: ConnectionsMap | null = null

      for (const [contextKey, streamActions] of grouped) {
        const source = next ?? state
        const conn = source.get(contextKey)
        if (!conn) continue

        let updatedConn = conn
        let hasChange = false
        for (const streamAction of streamActions) {
          const updated = applyStreamingAction(updatedConn, streamAction)
          if (!updated) continue
          updatedConn = updated
          hasChange = true
        }
        if (!hasChange) continue

        if (!next) {
          next = new Map(state)
        }
        next.set(contextKey, updatedConn)
      }

      return next ?? state
    }

    case "TOOL_CALL": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      // Out-of-turn wire tool activity stays OUT of `liveMessage` (the
      // transcript overlay renders that content — grafting it here recreated
      // the garbled-timeline bug), but its context is still recorded so a
      // background permission request can render command/diff details.
      if (conn.status !== "prompting") {
        const next = new Map(state)
        next.set(action.contextKey, {
          ...conn,
          outOfTurnToolCalls: recordOutOfTurnToolCall(conn.outOfTurnToolCalls, {
            tool_call_id: action.tool_call_id,
            title: action.title,
            kind: action.kind,
            status: action.status,
            content: action.content,
            raw_input: action.raw_input,
            raw_output_chunks:
              action.raw_output !== null ? [action.raw_output] : [],
            raw_output_total_bytes: action.raw_output?.length ?? 0,
            locations: action.locations,
            meta: action.meta,
            images: action.images ?? [],
          }),
        })
        return next
      }
      const prev = ensureLiveMessage(conn.liveMessage)
      const existingIndex = prev.content.findIndex(
        (b) =>
          b.type === "tool_call" && b.info.tool_call_id === action.tool_call_id
      )
      let newContent: LiveContentBlock[]
      if (existingIndex !== -1) {
        const block = prev.content[existingIndex]
        if (block.type === "tool_call") {
          newContent = [
            ...prev.content.slice(0, existingIndex),
            {
              type: "tool_call",
              info: {
                ...block.info,
                title: action.title ?? block.info.title,
                kind: action.kind ?? block.info.kind,
                status: action.status ?? block.info.status,
                content: action.content ?? block.info.content,
                raw_input: action.raw_input ?? block.info.raw_input,
                raw_output_chunks:
                  action.raw_output !== null
                    ? [action.raw_output]
                    : block.info.raw_output_chunks,
                raw_output_total_bytes:
                  action.raw_output !== null
                    ? action.raw_output.length
                    : block.info.raw_output_total_bytes,
                images:
                  action.images !== null ? action.images : block.info.images,
              },
            },
            ...prev.content.slice(existingIndex + 1),
          ]
        } else {
          newContent = prev.content
        }
      } else {
        newContent = [
          ...prev.content,
          {
            type: "tool_call",
            info: {
              tool_call_id: action.tool_call_id,
              title: action.title,
              kind: action.kind,
              status: action.status,
              content: action.content,
              raw_input: action.raw_input,
              raw_output_chunks:
                action.raw_output !== null ? [action.raw_output] : [],
              raw_output_total_bytes: action.raw_output?.length ?? 0,
              locations: action.locations ?? null,
              meta: action.meta ?? null,
              images: action.images ?? [],
            },
          },
        ]
      }
      const nextLiveMessage = { ...prev, content: newContent }
      const nextInfo = findLiveToolCallInfo(newContent, action.tool_call_id)
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: nextLiveMessage,
        pendingPermission: mergePendingPermissionWithLiveInfo(
          conn.pendingPermission,
          nextInfo
        ),
        claudeApiRetry: null,
      })
      return next
    }

    case "TOOL_CALL_UPDATE": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      // Out-of-turn: stay out of `liveMessage` (see TOOL_CALL), but merge the
      // registry entry and backfill an open permission dialog waiting for
      // this tool's input — a background permission must still show its
      // command/diff details. In-turn ordering is safe: the panel flushes
      // pending tool-call updates at turn_complete BEFORE status flips back.
      if (conn.status !== "prompting") {
        const existing = conn.outOfTurnToolCalls?.get(action.tool_call_id)
        const merged: ToolCallInfo = existing
          ? {
              ...existing,
              title: action.title ?? existing.title,
              status: action.status ?? existing.status,
              content: action.content ?? existing.content,
              raw_input: action.raw_input ?? existing.raw_input,
              locations: action.locations ?? existing.locations,
              meta: action.meta ?? existing.meta,
              images: action.images !== null ? action.images : existing.images,
            }
          : {
              tool_call_id: action.tool_call_id,
              title: action.title ?? action.fallback_title,
              kind: action.fallback_kind,
              status: action.status ?? "pending",
              content: action.content,
              raw_input: action.raw_input,
              raw_output_chunks: [],
              raw_output_total_bytes: 0,
              locations: action.locations,
              meta: action.meta,
              images: action.images ?? [],
            }
        const next = new Map(state)
        next.set(action.contextKey, {
          ...conn,
          outOfTurnToolCalls: recordOutOfTurnToolCall(
            conn.outOfTurnToolCalls,
            merged
          ),
          pendingPermission: mergePendingPermissionWithLiveInfo(
            conn.pendingPermission,
            merged
          ),
        })
        return next
      }
      const prev = ensureLiveMessage(conn.liveMessage)
      const existingIndex = prev.content.findIndex(
        (b) =>
          b.type === "tool_call" && b.info.tool_call_id === action.tool_call_id
      )
      let newContent: LiveContentBlock[]

      if (existingIndex === -1) {
        const initialChunks =
          action.raw_output !== null ? [action.raw_output] : []
        const initialBytes = action.raw_output?.length ?? 0
        newContent = [
          ...prev.content,
          {
            type: "tool_call",
            info: {
              tool_call_id: action.tool_call_id,
              title: action.title ?? action.fallback_title,
              kind: action.fallback_kind,
              status:
                action.status ??
                (initialChunks.length > 0 ? "in_progress" : "pending"),
              content: action.content,
              raw_input: action.raw_input,
              raw_output_chunks: initialChunks,
              raw_output_total_bytes: initialBytes,
              locations: action.locations ?? null,
              meta: action.meta ?? null,
              images: action.images ?? [],
            },
          },
        ]
      } else {
        const block = prev.content[existingIndex]
        if (block.type !== "tool_call") return state

        let newChunks: string[]
        let newTotalBytes: number

        if (action.raw_output === null) {
          newChunks = block.info.raw_output_chunks
          newTotalBytes = block.info.raw_output_total_bytes
        } else if (action.raw_output_append) {
          newChunks = [...block.info.raw_output_chunks, action.raw_output]
          newTotalBytes =
            block.info.raw_output_total_bytes + action.raw_output.length

          // 超限时从头部批量移除 chunks（单次 slice 替代循环 shift）
          if (
            newTotalBytes > MAX_LIVE_TOOL_RAW_OUTPUT_CHARS &&
            newChunks.length > 1
          ) {
            let evictCount = 0
            let evictedBytes = 0
            while (
              evictCount < newChunks.length - 1 &&
              newTotalBytes - evictedBytes > MAX_LIVE_TOOL_RAW_OUTPUT_CHARS
            ) {
              evictedBytes += newChunks[evictCount].length
              evictCount++
            }
            if (evictCount > 0) {
              newChunks = newChunks.slice(evictCount)
              newTotalBytes -= evictedBytes
            }
          }
        } else {
          // 非 append 模式（替换）
          newChunks = [action.raw_output]
          newTotalBytes = action.raw_output.length
        }

        newContent = [
          ...prev.content.slice(0, existingIndex),
          {
            type: "tool_call" as const,
            info: {
              ...block.info,
              title: action.title ?? block.info.title,
              status: action.status ?? block.info.status,
              content: action.content ?? block.info.content,
              raw_input: action.raw_input ?? block.info.raw_input,
              raw_output_chunks: newChunks,
              locations: action.locations ?? block.info.locations,
              meta: action.meta ?? block.info.meta,
              raw_output_total_bytes: newTotalBytes,
              images:
                action.images !== null ? action.images : block.info.images,
            },
          },
          ...prev.content.slice(existingIndex + 1),
        ]
      }

      const nextLiveMessage = { ...prev, content: newContent }
      const nextInfo = findLiveToolCallInfo(newContent, action.tool_call_id)
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: nextLiveMessage,
        pendingPermission: mergePendingPermissionWithLiveInfo(
          conn.pendingPermission,
          nextInfo
        ),
        claudeApiRetry: null,
      })
      return next
    }

    case "BATCH_TOOL_CALL_UPDATES": {
      let current = state
      for (const sub of action.actions) {
        current = connectionsReducer(current, {
          type: "TOOL_CALL_UPDATE",
          ...sub,
        })
      }
      return current
    }

    case "PERMISSION_REQUEST": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      let updatedLiveMessage = conn.liveMessage
      const permissionCallId = extractPermissionToolCallId(action.tool_call)
      // Live tool context first; for an OUT-OF-TURN permission (background
      // sub-agent work — liveMessage intentionally untouched) fall back to
      // the out-of-turn registry so the dialog still shows command/diff.
      const existingInfo =
        (updatedLiveMessage
          ? findLiveToolCallInfo(updatedLiveMessage.content, permissionCallId)
          : null) ??
        (permissionCallId
          ? (conn.outOfTurnToolCalls?.get(permissionCallId) ?? null)
          : null)
      const permissionToolCall = mergePermissionToolCallWithLiveInfo(
        action.tool_call,
        existingInfo
      )
      const permissionToolInput =
        serializePermissionToolCall(permissionToolCall)
      if (
        updatedLiveMessage &&
        permissionCallId &&
        typeof permissionToolInput === "string"
      ) {
        const existingIndex = updatedLiveMessage.content.findIndex(
          (block) =>
            block.type === "tool_call" &&
            block.info.tool_call_id === permissionCallId
        )
        if (existingIndex !== -1) {
          const block = updatedLiveMessage.content[existingIndex]
          if (block.type === "tool_call") {
            const nextContent: LiveContentBlock[] = [
              ...updatedLiveMessage.content.slice(0, existingIndex),
              {
                type: "tool_call",
                info: {
                  ...block.info,
                  raw_input:
                    block.info.raw_input && block.info.raw_input.length > 0
                      ? block.info.raw_input
                      : permissionToolInput,
                },
              },
              ...updatedLiveMessage.content.slice(existingIndex + 1),
            ]
            updatedLiveMessage = {
              ...updatedLiveMessage,
              content: nextContent,
            }
          }
        } else {
          updatedLiveMessage = {
            ...updatedLiveMessage,
            content: [
              ...updatedLiveMessage.content,
              {
                type: "tool_call",
                info: {
                  tool_call_id: permissionCallId,
                  title:
                    extractPermissionToolTitle(action.tool_call) ??
                    action.fallback_title,
                  kind:
                    extractPermissionToolKind(action.tool_call) ??
                    action.fallback_kind,
                  status: "pending",
                  content: null,
                  raw_input: permissionToolInput,
                  raw_output_chunks: [],
                  raw_output_total_bytes: 0,
                  locations: null,
                  meta: null,
                  images: [],
                },
              },
            ],
          }
        }
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: updatedLiveMessage,
        pendingPermission: {
          request_id: action.request_id,
          tool_call: permissionToolCall,
          options: action.options,
        },
      })
      return next
    }

    case "PERMISSION_CLEARED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (
        action.requestId !== undefined &&
        conn.pendingPermission?.request_id !== action.requestId
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingPermission: null,
      })
      return next
    }

    case "SET_PENDING_QUESTION": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingQuestion: action.pendingQuestion,
      })
      return next
    }

    case "CLEAR_PENDING_QUESTION": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingQuestion: null,
      })
      return next
    }

    case "SET_ASK_QUESTION": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingAskQuestion: action.pendingAskQuestion,
      })
      return next
    }

    case "CLEAR_ASK_QUESTION": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (
        action.questionId !== undefined &&
        conn.pendingAskQuestion?.question_id !== action.questionId
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingAskQuestion: null,
      })
      return next
    }

    case "SESSION_STARTED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        sessionId: action.sessionId,
      })
      return next
    }

    case "SESSION_MODES": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (sameModes(conn.modes, action.modes)) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        modes: action.modes,
      })
      return next
    }

    case "SESSION_CONFIG_OPTIONS": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (sameConfigOptions(conn.configOptions, action.configOptions)) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        configOptions: action.configOptions,
      })
      return next
    }

    case "CONFIG_STALE_CHANGED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const kind = action.stale ? action.kind : null
      // A fresh stale=true is a NEW drift → un-dismiss so the banner reappears
      // even if the user had dismissed a previous one. stale=false clears it.
      const dismissed = action.stale ? false : conn.configStaleDismissed
      if (
        conn.configStale === action.stale &&
        conn.configStaleKind === kind &&
        conn.configStaleDismissed === dismissed
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        configStale: action.stale,
        configStaleKind: kind,
        configStaleDismissed: dismissed,
      })
      return next
    }

    case "DISMISS_CONFIG_STALE": {
      const conn = state.get(action.contextKey)
      if (!conn || conn.configStaleDismissed) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        configStaleDismissed: true,
      })
      return next
    }

    case "SELECTORS_READY": {
      const conn = state.get(action.contextKey)
      if (!conn || conn.selectorsReady) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        selectorsReady: true,
      })
      return next
    }

    case "PROMPT_CAPABILITIES": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (
        samePromptCapabilities(
          conn.promptCapabilities,
          action.promptCapabilities
        )
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        promptCapabilities: action.promptCapabilities,
      })
      return next
    }

    case "FORK_SUPPORTED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (conn.supportsFork === action.supported) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        supportsFork: action.supported,
      })
      return next
    }

    case "MODE_CHANGED": {
      const conn = state.get(action.contextKey)
      if (!conn?.modes) return state
      if (conn.modes.current_mode_id === action.modeId) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        modes: {
          ...conn.modes,
          current_mode_id: action.modeId,
        },
      })
      return next
    }

    case "CONFIG_OPTION_CHANGED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const options =
        conn.configOptions ??
        selectorsCache.get(conn.agentType)?.configOptions ??
        null
      if (!options) return state
      const idx = options.findIndex((o) => o.id === action.configId)
      if (idx === -1) return state
      const opt = options[idx]
      if (
        opt.kind.type !== "select" ||
        opt.kind.current_value === action.valueId
      ) {
        return state
      }
      const updated = [...options]
      updated[idx] = {
        ...opt,
        kind: { ...opt.kind, current_value: action.valueId },
      }
      const next = new Map(state)
      next.set(action.contextKey, { ...conn, configOptions: updated })
      return next
    }

    case "PLAN_UPDATE": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      // Same out-of-turn guard as TOOL_CALL / streaming deltas.
      if (conn.status !== "prompting") return state
      const prev = ensureLiveMessage(conn.liveMessage)
      const nonPlanContent = prev.content.filter(
        (block) => block.type !== "plan"
      )
      const currentPlan = [...prev.content]
        .reverse()
        .find((block): block is { type: "plan"; entries: PlanEntryInfo[] } => {
          return block.type === "plan"
        })

      if (
        action.entries.length === 0 &&
        currentPlan === undefined &&
        nonPlanContent.length === prev.content.length
      ) {
        return state
      }

      const isAlreadyCanonicalPlan =
        currentPlan !== undefined &&
        samePlanEntries(currentPlan.entries, action.entries) &&
        prev.content.length === nonPlanContent.length + 1 &&
        prev.content[prev.content.length - 1]?.type === "plan"

      if (isAlreadyCanonicalPlan) return state

      const newContent =
        action.entries.length === 0
          ? nonPlanContent
          : [
              ...nonPlanContent,
              { type: "plan" as const, entries: action.entries },
            ]

      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: { ...prev, content: newContent },
        claudeApiRetry: null,
      })
      return next
    }

    case "CLAUDE_API_RETRY": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        claudeApiRetry: action.retry,
      })
      return next
    }

    case "ERROR": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        claudeApiRetry: null,
        error: action.message,
      })
      return next
    }

    case "ACP_LOAD_ERROR": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        loadError: action.message,
      })
      return next
    }

    case "CLEAR_ACP_LOAD_ERROR": {
      const conn = state.get(action.contextKey)
      if (!conn || conn.loadError === null) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        loadError: null,
      })
      return next
    }

    case "AVAILABLE_COMMANDS": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const commands = dedupeCommandsByName(action.commands)
      if (sameCommands(conn.availableCommands, commands)) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        availableCommands: commands,
      })
      return next
    }

    case "USAGE_UPDATE": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      // Ignore usage updates that reset used to 0 when we already have
      // valid data — these come from synthetic responses for local commands
      // like /context and would overwrite the real context window usage.
      if (action.usage.used === 0 && conn.usage && conn.usage.used > 0) {
        return state
      }
      if (
        conn.usage?.used === action.usage.used &&
        conn.usage?.size === action.usage.size
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        usage: action.usage,
      })
      return next
    }

    default:
      return state
  }
}

// ── Ref-based store (replaces useReducer + Context) ──

interface InternalStore {
  connections: ConnectionsMap
  activeKey: string | null
  keyListeners: Map<string, Set<() => void>>
  activeKeyListeners: Set<() => void>
}

// ── Store API for consumers ──

export interface ConnectionStoreApi {
  getConnection(key: string): ConnectionState | undefined
  getActiveKey(): string | null
  subscribeKey(key: string, cb: () => void): () => void
  subscribeActiveKey(cb: () => void): () => void
}

const ConnectionStoreContext = createContext<ConnectionStoreApi | null>(null)

export function useConnectionStore(): ConnectionStoreApi {
  const ctx = useContext(ConnectionStoreContext)
  if (!ctx) {
    throw new Error(
      "useConnectionStore must be used within AcpConnectionsProvider"
    )
  }
  return ctx
}

// ── Actions context (unchanged interface) ──

/**
 * Sink that mirrors a connection's `liveMessage` into the conversation-runtime
 * store OUTSIDE React. Registered per `contextKey` by the conversation panel and
 * invoked synchronously from `dispatch` whenever that connection's `liveMessage`
 * reference changes (streaming deltas, tool updates, the prompt-start reset).
 * Moving this write out of a React effect lets the keep-alive panel stop
 * re-rendering on every streaming token — only the runtime-store subscriber (the
 * message list) re-renders. `isLive` is `status === "prompting"`, which the
 * runtime reducer uses to bypass its stale-reconnect-replay guard.
 */
export type LiveMessageSink = (
  liveMessage: LiveMessage,
  isLive: boolean
) => void

export interface AcpActionsValue {
  connect(
    contextKey: string,
    agentType: AgentType,
    workingDir?: string,
    sessionId?: string,
    conversationId?: number
  ): Promise<void>
  disconnect(contextKey: string): Promise<void>
  disconnectAll(): Promise<void>
  sendPrompt(
    contextKey: string,
    blocks: PromptInputBlock[],
    opts?: {
      folderId?: number | null
      conversationId?: number | null
      clientMessageId?: string | null
    }
  ): Promise<void>
  setMode(contextKey: string, modeId: string): Promise<void>
  setConfigOption(
    contextKey: string,
    configId: string,
    valueId: string
  ): Promise<void>
  cancel(contextKey: string): Promise<void>
  respondPermission(
    contextKey: string,
    requestId: string,
    optionId: string
  ): Promise<void>
  answerQuestion(
    contextKey: string,
    questionId: string,
    answer: QuestionAnswer
  ): Promise<void>
  setActiveKey(key: string | null): void
  touchActivity(contextKey: string): void
  registerOpenTabKeys(keys: Set<string>): void
  /**
   * Register a sink that mirrors this contextKey's `liveMessage` into the
   * conversation-runtime store from `dispatch` (outside React), replacing the
   * panel's per-token mirror effect. Returns an unregister fn (idempotent —
   * only removes the entry if it still points at this sink). See
   * `LiveMessageSink`.
   */
  registerLiveMessageSink(contextKey: string, sink: LiveMessageSink): () => void
  /**
   * Clear `loadError` set by a `session/load` failure so the next auto-connect
   * attempt isn't gated by stale failure state. Wired to the Reload button in
   * the conversation detail panel.
   */
  clearAcpLoadError(contextKey: string): void
  /**
   * Register a delegation-spawned child connection so its acp://event
   * stream lands in the reducer (live message, tool calls, permission
   * requests). The child connection is already alive on the backend —
   * this is a frontend-only attach. Idempotent on connectionId.
   *
   * Routing:
   *   * Tauri: registers the connectionId in the global event router
   *     and drains any envelopes that arrived before registration.
   *   * Web/remote: opens a per-connection WS attach so the snapshot +
   *     replay + live events arrive on a dedicated stream.
   */
  attachDelegationChild(args: {
    connectionId: string
    parentConnectionId: string
    parentToolUseId: string
    agentType: AgentType
  }): void
  /**
   * Tear down a previously-attached delegation child. Releases the
   * synthetic ConnectionState and any per-connection WS attach. Does
   * NOT call acpDisconnect — the broker owns the child's backend
   * lifecycle. No-op when the child isn't attached.
   */
  detachDelegationChild(connectionId: string): void
  /**
   * Restart the session at `contextKey` so it picks up the latest agent/model
   * settings: disconnect the running process, then reconnect with the same
   * `sessionId` (the agent resumes the conversation — history is preserved).
   * The freshly spawned process reads current config, so its recomputed
   * fingerprint matches and `configStale` clears. Wired to the "restart to
   * apply" banner button. Returns `true` if it actually restarted, `false` if
   * it was a no-op (no connection, or a viewer / delegation child that doesn't
   * own the backend process) — callers gate their "applied" confirmation on it.
   */
  reapplyConfig(contextKey: string): Promise<boolean>
  /**
   * Dismiss the "restart to apply" banner for the current drift WITHOUT
   * restarting (client-local; the underlying `configStale` is untouched). A
   * subsequent settings change re-shows it. Wired to the banner's X button.
   */
  dismissConfigStale(contextKey: string): void
}

const AcpActionsContext = createContext<AcpActionsValue | null>(null)

export function useAcpActions(): AcpActionsValue {
  const ctx = useContext(AcpActionsContext)
  if (!ctx) {
    throw new Error("useAcpActions must be used within AcpConnectionsProvider")
  }
  return ctx
}

// ── Event subscriber context ──
//
// JS-level fanout of `acp://event` envelopes. The provider owns the single
// physical Tauri/WebSocket subscription; consumers register callbacks here
// instead of opening a second listener. See `useAcpEvent` below.

type EventSubscriberHandler = (envelope: EventEnvelope) => void
type EventSubscriberRef = { current: EventSubscriberHandler }

interface AcpEventSubscriberApi {
  subscribers: Set<EventSubscriberRef>
}

const AcpEventSubscriberContext = createContext<AcpEventSubscriberApi | null>(
  null
)

/**
 * Subscribe to `acp://event` envelopes via the provider's primary listener.
 *
 * The handler is invoked AFTER the context's reducer has dispatched its own
 * actions for that envelope (state is consistent at fire time). It also
 * inherits the provider's `seq` dedup — duplicates the primary listener
 * would skip are skipped here too. Unmapped events (no `contextKey`) do
 * NOT fan out.
 *
 * Stability: the latest `handler` is stored in a ref each render, so callers
 * may pass an inline function. There is no need for caller-side refs to keep
 * the subscription stable across renders.
 *
 * Errors thrown by `handler` are caught and logged so a single buggy
 * subscriber cannot break the central listener.
 */
export function useAcpEvent(handler: EventSubscriberHandler): void {
  const ctx = useContext(AcpEventSubscriberContext)
  if (!ctx) {
    throw new Error("useAcpEvent must be used within AcpConnectionsProvider")
  }
  const handlerRef = useRef(handler)
  // Re-sync each render so the latest closure is used at fire time.
  useEffect(() => {
    handlerRef.current = handler
  })
  // Register / unregister exactly once. Set-of-refs (not Set-of-functions)
  // so unmount cleanup matches the original entry even though `handler`
  // identity may change between renders.
  useEffect(() => {
    const ref = handlerRef
    ctx.subscribers.add(ref)
    return () => {
      ctx.subscribers.delete(ref)
    }
  }, [ctx])
}

// ── Helper: extract affected key from action ──

function getAffectedKey(action: Action): string | null {
  if (action.type === "REMOVE_ALL") return null // special: all keys
  if (action.type === "STREAM_BATCH") return null
  if ("contextKey" in action) return action.contextKey
  return null
}

function normalizeErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}

type AlertedError = Error & { alerted: true }

function createAlertedError(message: string): AlertedError {
  const error = new Error(message) as AlertedError
  error.alerted = true
  return error
}

function isAlertedError(error: unknown): error is AlertedError {
  if (!error || typeof error !== "object") return false
  return (error as { alerted?: unknown }).alerted === true
}

// ── Provider ──

export function AcpConnectionsProvider({ children }: { children: ReactNode }) {
  const t = useTranslations("Folder.chat.acpConnections")
  const tChat = useTranslations("Folder.chat")
  const { pushAlert } = useAlertContext()
  const { activeFolder: folder } = useActiveFolder()
  const folderNameRef = useRef(folder?.name)
  useEffect(() => {
    folderNameRef.current = folder?.name
  }, [folder?.name])
  const pushAlertRef = useRef(pushAlert)
  useEffect(() => {
    pushAlertRef.current = pushAlert
  }, [pushAlert])

  // Ref-based store — mutations don't trigger React state updates
  const storeRef = useRef<InternalStore>({
    connections: new Map(),
    activeKey: null,
    keyListeners: new Map(),
    activeKeyListeners: new Set(),
  })

  // connectionId → contextKey reverse mapping. Used by the legacy global
  // `acp://event` listener path. Attach-protocol connections (web mode)
  // bypass this entirely — their events are routed by the per-subscription
  // handlers registered in `attachSubscriptionsRef`.
  const reverseMapRef = useRef(new Map<string, string>())

  // contextKey → active EventStream subscription handle. Populated only for
  // connections established via the Subscribe-with-Snapshot attach
  // protocol (web + remote-desktop). Used to (a) detach on disconnect /
  // tab close, and (b) re-attach with the current cursor when a connection
  // is rekeyed (orphan rescue) so handlers reference the new contextKey.
  const attachSubscriptionsRef = useRef(
    new Map<string, EventStreamSubscription>()
  )

  // Open tab keys — updated by child TabProvider via registerOpenTabKeys
  const openTabKeysRef = useRef(new Set<string>())

  // Guard against concurrent connect() calls
  const connectingKeysRef = useRef(new Set<string>())
  const pendingConnectRequestsRef = useRef(new Map<string, ConnectRequest>())
  // Keys whose disconnect was requested while connect was still in flight
  const abandonedKeysRef = useRef(new Set<string>())
  const connectRef = useRef<AcpActionsValue["connect"] | null>(null)

  type ConnectBlockState =
    | { kind: "none"; reason: "" }
    | {
        kind: "missing_config" | "disabled" | "unavailable" | "sdk_missing"
        reason: string
      }

  const buildOpenAgentsSettingsAction = useCallback(
    (agentType?: AgentType): AlertAction => {
      const payload =
        typeof agentType === "string"
          ? JSON.stringify({
              section: "agents",
              agentType,
            })
          : "agents"
      return {
        label: t("actions.openAgentsSettings"),
        kind: "open_agents_settings",
        payload,
      }
    },
    [t]
  )

  const resolveConnectBlockState = useCallback(
    (agent: AcpAgentStatus | null): ConnectBlockState => {
      if (!agent) {
        return { kind: "missing_config", reason: t("blocked.missingConfig") }
      }

      const agentLabel = AGENT_LABELS[agent.agent_type]
      if (!agent.enabled) {
        return {
          kind: "disabled",
          reason: t("blocked.disabled", { agent: agentLabel }),
        }
      }

      if (!agent.available) {
        return {
          kind: "unavailable",
          reason: t("blocked.unavailable", { agent: agentLabel }),
        }
      }

      if (agent.installed_version) {
        return { kind: "none", reason: "" }
      }

      return {
        kind: "sdk_missing",
        reason: t("blocked.sdkMissing", { agent: agentLabel }),
      }
    },
    [t]
  )

  // Per-contextKey liveMessage sinks. Fired synchronously from `dispatch` when a
  // connection's liveMessage reference changes, mirroring it into the runtime
  // store outside React (see `LiveMessageSink`). A ref → no re-renders.
  const liveMessageSinksRef = useRef(new Map<string, LiveMessageSink>())

  // Activity tracking (no re-renders)
  const lastActivityRef = useRef(new Map<string, number>())
  const streamingQueueRef = useRef<StreamingAction[]>([])
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pendingUnmappedEventsRef = useRef(new Map<string, EventEnvelope[]>())
  const listenerReadyRef = useRef(false)
  const listenerReadyWaitersRef = useRef<Array<() => void>>([])
  // Set of refs (not callbacks) so unmount cleanup matches the original
  // registration even when caller-side handler identity changes per render.
  // Populated by the `useAcpEvent` hook; read by the primary `acp://event`
  // listener and the buffered-events replay loop.
  const eventSubscribersRef = useRef<Set<EventSubscriberRef>>(new Set())

  // ── Notify helpers ──

  const notifyKeyListeners = useCallback((key: string) => {
    const listeners = storeRef.current.keyListeners.get(key)
    if (listeners) {
      for (const cb of listeners) cb()
    }
  }, [])

  const notifyAllKeyListeners = useCallback(() => {
    for (const [, listeners] of storeRef.current.keyListeners) {
      for (const cb of listeners) cb()
    }
  }, [])

  const notifyActiveKeyListeners = useCallback(() => {
    for (const cb of storeRef.current.activeKeyListeners) cb()
  }, [])

  // ── Dispatch (replaces useReducer dispatch) ──

  const dispatch = useCallback(
    (action: Action) => {
      const prev = storeRef.current.connections
      const next = connectionsReducer(prev, action)
      if (next === prev) return // no change

      storeRef.current.connections = next

      // Mirror a changed liveMessage into the runtime store OUTSIDE React, so
      // the keep-alive conversation panel no longer has to re-render per
      // streaming token just to run a mirror effect. Fires only when the
      // reference actually changed and a sink is registered for the key; writes
      // non-null values (turn-end clearing is owned by COMPLETE_TURN, unmount by
      // removeConversation). `isLive = status === "prompting"`.
      //
      // Ordering: mirror BEFORE notifying the connection's key listeners, so the
      // runtime store is updated before React observes the connection change —
      // the panel and the runtime-store-driven message list then re-render off a
      // consistent snapshot (not relying on React batching to reconcile the two).
      const mirrorLiveMessage = (key: string) => {
        const sink = liveMessageSinksRef.current.get(key)
        if (!sink) return
        const nextConn = next.get(key)
        if (!nextConn || nextConn.liveMessage == null) return
        if (nextConn.liveMessage === prev.get(key)?.liveMessage) return
        sink(nextConn.liveMessage, nextConn.status === "prompting")
      }

      if (action.type === "REMOVE_ALL") {
        notifyAllKeyListeners()
      } else if (action.type === "STREAM_BATCH") {
        const keys = new Set(action.actions.map((item) => item.contextKey))
        for (const key of keys) {
          mirrorLiveMessage(key)
          notifyKeyListeners(key)
        }
      } else if (action.type === "BATCH_TOOL_CALL_UPDATES") {
        const keys = new Set(action.actions.map((item) => item.contextKey))
        for (const key of keys) {
          mirrorLiveMessage(key)
          notifyKeyListeners(key)
        }
      } else if (action.type === "REKEY_CONNECTION") {
        // The connection (with its in-flight liveMessage) moved to toKey; sync
        // it BEFORE notifying so a close+reopen mid-turn doesn't drop the stream.
        mirrorLiveMessage(action.toKey)
        notifyKeyListeners(action.fromKey)
        notifyKeyListeners(action.toKey)
      } else {
        const key = getAffectedKey(action)
        if (key) {
          mirrorLiveMessage(key)
          notifyKeyListeners(key)
        }
      }
    },
    [notifyKeyListeners, notifyAllKeyListeners]
  )

  // ── setActiveKey ──

  const setActiveKey = useCallback(
    (key: string | null) => {
      if (storeRef.current.activeKey === key) return
      storeRef.current.activeKey = key
      notifyActiveKeyListeners()
    },
    [notifyActiveKeyListeners]
  )

  // ── Store API (stable object — never recreated) ──

  const storeApi = useMemo<ConnectionStoreApi>(() => {
    return {
      getConnection(key: string) {
        return storeRef.current.connections.get(key)
      },
      getActiveKey() {
        return storeRef.current.activeKey
      },
      subscribeKey(key: string, cb: () => void) {
        const { keyListeners } = storeRef.current
        let set = keyListeners.get(key)
        if (!set) {
          set = new Set()
          keyListeners.set(key, set)
        }
        set.add(cb)
        return () => {
          set!.delete(cb)
          if (set!.size === 0) keyListeners.delete(key)
        }
      },
      subscribeActiveKey(cb: () => void) {
        storeRef.current.activeKeyListeners.add(cb)
        return () => {
          storeRef.current.activeKeyListeners.delete(cb)
        }
      },
    }
  }, [])

  const touchActivity = useCallback((contextKey: string) => {
    lastActivityRef.current.set(contextKey, Date.now())
  }, [])

  const registerOpenTabKeys = useCallback((keys: Set<string>) => {
    openTabKeysRef.current = keys
  }, [])

  const registerLiveMessageSink = useCallback(
    (contextKey: string, sink: LiveMessageSink) => {
      liveMessageSinksRef.current.set(contextKey, sink)
      // Replay the CURRENT liveMessage immediately, matching the removed mirror
      // effect's setup write. A panel can mount/remount over a connection that
      // already holds a non-null liveMessage — connection reuse, a viewer
      // attaching mid-turn, or close+reopen mid-turn — and if the stream is
      // paused (e.g. blocked on a permission/question) no further delta would
      // arrive to trigger the sink. Without this replay the runtime store, and
      // thus the message list, would stay blank/stale until the next change.
      const conn = storeRef.current.connections.get(contextKey)
      if (conn?.liveMessage != null) {
        sink(conn.liveMessage, conn.status === "prompting")
      }
      return () => {
        // Idempotent: only drop the entry if it still points at this sink (a
        // remount may have already replaced it).
        if (liveMessageSinksRef.current.get(contextKey) === sink) {
          liveMessageSinksRef.current.delete(contextKey)
        }
      }
    },
    []
  )

  const clearAcpLoadError = useCallback(
    (contextKey: string) => {
      dispatch({ type: "CLEAR_ACP_LOAD_ERROR", contextKey })
    },
    [dispatch]
  )

  const flushStreamingQueue = useCallback(() => {
    flushTimerRef.current = null
    const queued = streamingQueueRef.current
    if (queued.length === 0) return
    streamingQueueRef.current = []

    // Merge adjacent deltas by connection key (per-key order preserved),
    // reducing reducer work and string copies under high-frequency streams.
    const grouped = new Map<string, StreamingAction[]>()
    for (const action of queued) {
      const list = grouped.get(action.contextKey)
      if (!list) {
        grouped.set(action.contextKey, [{ ...action }])
        continue
      }
      const last = list[list.length - 1]
      if (last && last.type === action.type) {
        last.text += action.text
      } else {
        list.push({ ...action })
      }
    }

    const compacted = Array.from(grouped.values()).flat()
    dispatch({ type: "STREAM_BATCH", actions: compacted })
  }, [dispatch])

  const enqueueStreamingAction = useCallback(
    (action: StreamingAction) => {
      streamingQueueRef.current.push(action)
      if (streamingQueueRef.current.length >= 256) {
        if (flushTimerRef.current !== null) {
          clearTimeout(flushTimerRef.current)
          flushTimerRef.current = null
        }
        flushStreamingQueue()
        return
      }
      if (flushTimerRef.current === null) {
        flushTimerRef.current = setTimeout(flushStreamingQueue, 16)
      }
    },
    [flushStreamingQueue]
  )

  const resolveListenerReadyWaiters = useCallback(() => {
    if (listenerReadyWaitersRef.current.length === 0) return
    const waiters = listenerReadyWaitersRef.current
    listenerReadyWaitersRef.current = []
    for (const resolve of waiters) resolve()
  }, [])

  const waitForListenerReady = useCallback(async () => {
    if (listenerReadyRef.current) return
    await new Promise<void>((resolve) => {
      listenerReadyWaitersRef.current.push(resolve)
    })
  }, [])

  const bufferUnmappedEvent = useCallback((event: EventEnvelope) => {
    const connectionId = event.connection_id
    const buffered = pendingUnmappedEventsRef.current.get(connectionId) ?? []
    if (buffered.length >= MAX_BUFFERED_UNMAPPED_EVENTS_PER_CONNECTION) {
      buffered.shift()
    }
    buffered.push(event)
    pendingUnmappedEventsRef.current.set(connectionId, buffered)

    if (
      pendingUnmappedEventsRef.current.size > MAX_BUFFERED_UNMAPPED_CONNECTIONS
    ) {
      const oldest = pendingUnmappedEventsRef.current.keys().next().value
      if (oldest) {
        pendingUnmappedEventsRef.current.delete(oldest)
      }
    }
  }, [])

  const consumeBufferedEvents = useCallback(
    (connectionId: string): EventEnvelope[] => {
      const buffered = pendingUnmappedEventsRef.current.get(connectionId)
      if (!buffered || buffered.length === 0) return []
      pendingUnmappedEventsRef.current.delete(connectionId)
      return buffered
    },
    []
  )

  // ── RAF batching for tool_call_update events ──
  const pendingToolCallUpdates = useRef<
    Array<{
      contextKey: string
      tool_call_id: string
      title: string | null
      fallback_title: string
      fallback_kind: string
      status: string | null
      content: string | null
      raw_input: string | null
      raw_output: string | null
      raw_output_append?: boolean
      locations: unknown
      meta: ToolCallMeta
      images: ToolCallImage[] | null
    }>
  >([])
  const toolCallUpdateRafId = useRef<number | null>(null)

  const flushPendingToolCallUpdates = useCallback(() => {
    if (pendingToolCallUpdates.current.length === 0) return
    if (toolCallUpdateRafId.current !== null) {
      cancelAnimationFrame(toolCallUpdateRafId.current)
      toolCallUpdateRafId.current = null
    }
    const batch = pendingToolCallUpdates.current
    pendingToolCallUpdates.current = []
    dispatch({ type: "BATCH_TOOL_CALL_UPDATES", actions: batch })
  }, [dispatch])

  const scheduleToolCallUpdateFlush = useCallback(() => {
    if (toolCallUpdateRafId.current !== null) return
    toolCallUpdateRafId.current = requestAnimationFrame(() => {
      toolCallUpdateRafId.current = null
      flushPendingToolCallUpdates()
    })
  }, [flushPendingToolCallUpdates])

  useEffect(() => {
    return () => {
      if (toolCallUpdateRafId.current !== null) {
        cancelAnimationFrame(toolCallUpdateRafId.current)
      }
    }
  }, [])

  const handleMappedEvent = useCallback(
    (contextKey: string, e: EventEnvelope) => {
      switch (e.type) {
        case "status_changed":
          flushStreamingQueue()
          dispatch({ type: "STATUS_CHANGED", contextKey, status: e.status })
          break
        case "content_delta":
          enqueueStreamingAction({
            type: "CONTENT_DELTA",
            contextKey,
            text: e.text,
          })
          break
        case "thinking":
          enqueueStreamingAction({ type: "THINKING", contextKey, text: e.text })
          break
        case "claude_sdk_message":
          flushStreamingQueue()
          dispatch({
            type: "CLAUDE_API_RETRY",
            contextKey,
            retry: parseClaudeApiRetryEvent(e),
          })
          break
        case "tool_call":
          flushStreamingQueue()
          dispatch({
            type: "TOOL_CALL",
            contextKey,
            tool_call_id: e.tool_call_id,
            title: e.title,
            kind: e.kind,
            status: e.status,
            content: e.content,
            raw_input: e.raw_input,
            raw_output: e.raw_output,
            locations: e.locations ?? null,
            meta: (e.meta as ToolCallMeta) ?? null,
            images: e.images ?? null,
          })
          break
        case "tool_call_update":
          flushStreamingQueue()
          pendingToolCallUpdates.current.push({
            contextKey,
            tool_call_id: e.tool_call_id,
            title: e.title,
            fallback_title: t("toolFallbackTitle"),
            fallback_kind: "tool",
            status: e.status,
            content: e.content,
            raw_input: e.raw_input,
            raw_output: e.raw_output,
            raw_output_append: e.raw_output_append,
            locations: e.locations ?? null,
            meta: (e.meta as ToolCallMeta) ?? null,
            images: e.images ?? null,
          })
          scheduleToolCallUpdateFlush()
          break
        case "permission_resolved":
          // Backend signals a permission was answered (this window's local
          // respondPermission, a sibling window, a server-mode peer, or
          // chat-channel auto-approve). The local-respond path already
          // dispatched PERMISSION_CLEARED synchronously, so this is a no-op
          // there; the other three paths rely on this branch to retire the
          // dialog without waiting for TurnComplete. Matched by request_id so
          // a stale event can't wipe a fresh permission.
          dispatch({
            type: "PERMISSION_CLEARED",
            contextKey,
            requestId: e.request_id,
          })
          break
        case "question_request":
          // Agent called the blocking `ask_user_question` MCP tool. Flush any
          // queued streaming so the card renders against current content, then
          // raise the interactive multiple-choice card above the input box.
          flushStreamingQueue()
          dispatch({
            type: "SET_ASK_QUESTION",
            contextKey,
            pendingAskQuestion: {
              question_id: e.question_id,
              questions: e.questions,
              created_at: new Date().toISOString(),
            },
          })
          break
        case "question_resolved":
          // The question was answered (this or another window) or canceled.
          // Matched by question_id so a stale event can't wipe a fresh one.
          dispatch({
            type: "CLEAR_ASK_QUESTION",
            contextKey,
            questionId: e.question_id,
          })
          break
        case "background_activity": {
          // Out-of-turn transcript activity from the backend watcher: async
          // task completions, the agent's continued work after them, cron//
          // loop turns. Three consumers:
          // 1. the outstanding mirror (idle-sweep exemption + chip) plus the
          //    settle-syncing bridge inputs;
          dispatch({
            type: "SET_BACKGROUND_OUTSTANDING",
            contextKey,
            outstanding: e.outstanding,
            // Only settles whose reply arrives out of turn (NOT wire-visible)
            // warrant the syncing hint; a #870-held settle's reply is already
            // live on screen (see the reducer + BackgroundSettledInfo).
            outOfTurnSettleCount:
              e.settled?.filter((s) => !s.wire_visible).length ?? 0,
            turnsCount: e.turns?.length ?? 0,
          })
          // 2. overlay turns → the conversation runtime store (resolved via
          //    the external-id index; unresolved = this conversation was never
          //    opened in this client, and its cold detail fetch covers it);
          if (e.turns && e.turns.length > 0) {
            const conversationId = getConversationIdByExternalIdFromStore(
              e.session_id
            )
            if (conversationId != null) {
              const runtime = useConversationRuntimeStore.getState()
              runtime.actions.applyBackgroundActivity(
                conversationId,
                e.turns,
                e.watermark
              )
              // Self-healing bound: cron//loop turns never settle, so nothing
              // else would ever refetch — the overlay would grow for as long
              // as the tab stays open. Past the threshold, fold what's
              // accumulated into persisted turns (the watermark rule retires
              // covered entries). Guarded by the in-flight flag and a
              // per-conversation interval so a failing backend can't turn
              // this into a 1Hz fetch loop.
              const session = useConversationRuntimeStore
                .getState()
                .byConversationId.get(conversationId)
              const now = Date.now()
              const lastAt = overlayFoldRefetchAt.get(conversationId) ?? 0
              if (
                session &&
                session.backgroundTurns.length > OVERLAY_FOLD_THRESHOLD &&
                !session.detailLoading &&
                now - lastAt > OVERLAY_FOLD_MIN_INTERVAL_MS
              ) {
                overlayFoldRefetchAt.set(conversationId, now)
                const oc = storeRef.current.connections.get(contextKey)
                runtime.actions.refetchDetail(conversationId, {
                  preserveLive: oc?.status === "prompting",
                })
              }
            }
          }
          // 3. one OS notification per settled task (matches the permission
          //    notification's shape; `document.hidden` gating lives inside
          //    sendSystemNotification).
          if (e.settled && e.settled.length > 0) {
            const nc = storeRef.current.connections.get(contextKey)
            const agentLabel = nc ? AGENT_LABELS[nc.agentType] : "Agent"
            const fn = folderNameRef.current
            const title = fn ? `${fn} - Codeg` : "Codeg"
            for (const settled of e.settled) {
              const body =
                settled.summary ??
                tChat("backgroundTasks.settledFallback", {
                  status: settled.status,
                })
              sendSystemNotification(title, `${agentLabel}: ${body}`).catch(
                () => {}
              )
            }
            // 4. flip each async sub-agent's launch card to its terminal
            //    (completed + result) state IN-MEMORY, by rewriting the
            //    launching tool call's `[[codeg-background-task]]` marker from
            //    the settle payload's own `tool_use_id`/`status`/`result`. This
            //    deliberately replaces the `refetchDetail` this used to do: that
            //    refetch re-parsed the still-open transcript mid-#870-hold,
            //    double-rendering the held turn AND racing the file's last
            //    write. Entries with
            //    no `tool_use_id` (background shells) have no marker card and are
            //    skipped. The store queues a settlement whose launch turn hasn't
            //    promoted yet and applies it at COMPLETE_TURN.
            const conversationId = getConversationIdByExternalIdFromStore(
              e.session_id
            )
            if (conversationId != null) {
              const runtimeActions =
                useConversationRuntimeStore.getState().actions
              for (const settled of e.settled) {
                if (!settled.tool_use_id) continue
                runtimeActions.resolveBackgroundTask(conversationId, {
                  toolUseId: settled.tool_use_id,
                  taskId: settled.task_id,
                  status: settled.status,
                  summary: settled.summary ?? null,
                  result: settled.result ?? null,
                })
              }
            }
          }
          break
        }
        case "permission_request":
          flushStreamingQueue()
          flushPendingToolCallUpdates()
          dispatch({
            type: "PERMISSION_REQUEST",
            contextKey,
            request_id: e.request_id,
            tool_call: e.tool_call,
            fallback_title: t("toolFallbackTitle"),
            fallback_kind: "tool",
            options: e.options,
          })
          // Send OS notification when permission approval is needed
          {
            const nc = storeRef.current.connections.get(contextKey)
            if (nc) {
              const agentLabel = AGENT_LABELS[nc.agentType]
              const fn = folderNameRef.current
              const title = fn ? `${fn} - Codeg` : "Codeg"
              sendSystemNotification(
                title,
                `${agentLabel}: ${tChat("permissionDialog.subtitle")}`
              ).catch(() => {})
            }
          }
          break
        case "session_started":
          flushStreamingQueue()
          dispatch({
            type: "SESSION_STARTED",
            contextKey,
            sessionId: e.session_id,
          })
          break
        case "conversation_linked":
          // Backend just bound (or reaffirmed) the connection's DB conversation
          // row. Phase 3a frontend pre-creates rows for new-tab sends so this
          // event is mostly a confirmation; we log it for visibility. Phase 3b
          // will use this to drive UI mapping when the frontend stops creating
          // rows itself.
          console.log("[acp-context] conversation_linked", {
            contextKey,
            connectionId: e.connection_id,
            conversationId: e.conversation_id,
            folderId: e.folder_id,
          })
          break
        case "session_modes": {
          flushStreamingQueue()
          // Preferences are applied on the backend during connect (see
          // `getSavedPrefsForConnect` + `acp_connect`), so `e.modes` already
          // carries the user's preferred `current_mode_id` — no client-side
          // override or sync-back needed.
          dispatch({
            type: "SESSION_MODES",
            contextKey,
            modes: e.modes,
          })
          const modeConn = storeRef.current.connections.get(contextKey)
          if (modeConn) {
            const entry = selectorsCache.get(modeConn.agentType) ?? {
              modes: null,
              configOptions: null,
            }
            entry.modes = e.modes
            selectorsCache.set(modeConn.agentType, entry)
          }
          break
        }
        case "session_config_options": {
          flushStreamingQueue()
          // Same as `session_modes`: backend already merged saved prefs
          // into `current_value` before emitting.
          dispatch({
            type: "SESSION_CONFIG_OPTIONS",
            contextKey,
            configOptions: e.config_options,
          })
          const cfgConn = storeRef.current.connections.get(contextKey)
          if (cfgConn) {
            const entry = selectorsCache.get(cfgConn.agentType) ?? {
              modes: null,
              configOptions: null,
            }
            entry.configOptions = e.config_options
            selectorsCache.set(cfgConn.agentType, entry)
          }
          break
        }
        case "session_config_stale": {
          flushStreamingQueue()
          dispatch({
            type: "CONFIG_STALE_CHANGED",
            contextKey,
            stale: e.stale,
            kind: e.kind,
          })
          break
        }
        case "selectors_ready": {
          flushStreamingQueue()
          dispatch({
            type: "SELECTORS_READY",
            contextKey,
          })
          // Cache for agent types that may not emit session_modes /
          // session_config_options at all (no selectors).
          const rdyConn = storeRef.current.connections.get(contextKey)
          if (rdyConn && !selectorsCache.has(rdyConn.agentType)) {
            selectorsCache.set(rdyConn.agentType, {
              modes: rdyConn.modes,
              configOptions: rdyConn.configOptions,
            })
          }
          break
        }
        case "prompt_capabilities":
          flushStreamingQueue()
          dispatch({
            type: "PROMPT_CAPABILITIES",
            contextKey,
            promptCapabilities: e.prompt_capabilities,
          })
          break
        case "fork_supported":
          flushStreamingQueue()
          dispatch({
            type: "FORK_SUPPORTED",
            contextKey,
            supported: e.supported,
          })
          break
        case "mode_changed":
          flushStreamingQueue()
          dispatch({
            type: "MODE_CHANGED",
            contextKey,
            modeId: e.mode_id,
          })
          break
        case "plan_update":
          flushStreamingQueue()
          dispatch({
            type: "PLAN_UPDATE",
            contextKey,
            entries: e.entries,
          })
          break
        case "turn_complete": {
          flushStreamingQueue()
          flushPendingToolCallUpdates()
          dispatch({
            type: "STATUS_CHANGED",
            contextKey,
            status: "connected",
          })
          // Detect pending question from tool calls in the completed turn
          const turnConn = storeRef.current.connections.get(contextKey)
          if (turnConn?.liveMessage) {
            const blocks = turnConn.liveMessage.content
            for (let i = blocks.length - 1; i >= 0; i--) {
              const block = blocks[i]
              if (block.type !== "tool_call") continue
              const normalized = inferLiveToolName({
                title: block.info.title,
                kind: block.info.kind,
                rawInput: block.info.raw_input,
                meta: block.info.meta,
              })
              if (normalized === "question") {
                const questionText = extractQuestionText(block.info.raw_input)
                if (questionText) {
                  dispatch({
                    type: "SET_PENDING_QUESTION",
                    contextKey,
                    pendingQuestion: {
                      tool_call_id: block.info.tool_call_id,
                      question: questionText,
                    },
                  })
                }
                break
              }
            }
          }
          // Send OS notification when window is not focused
          {
            const nc = storeRef.current.connections.get(contextKey)
            if (nc) {
              const agentLabel = AGENT_LABELS[nc.agentType]
              const fn = folderNameRef.current
              const title = fn ? `${fn} - Codeg` : "Codeg"
              sendSystemNotification(
                title,
                t("notificationTurnComplete", { agent: agentLabel })
              ).catch(() => {})
            }
          }
          break
        }
        case "error": {
          flushStreamingQueue()
          const nc = storeRef.current.connections.get(contextKey)
          const agentLabel = nc
            ? AGENT_LABELS[nc.agentType]
            : (e.agent_type as string)

          // Localize backend errors via their stable `code` identifier.
          // Unknown codes fall back to the raw English message so we
          // never swallow a useful stack trace.
          const localizedMessage = (() => {
            switch (e.code) {
              case "initialize_timeout":
                return t("backendErrors.initializeTimeout", {
                  agent: agentLabel,
                })
              case "sdk_not_installed":
                return t("blocked.sdkMissing", { agent: agentLabel })
              case "platform_not_supported":
                return t("blocked.unavailable", { agent: agentLabel })
              case "process_exited":
                return t("backendErrors.processExited", { agent: agentLabel })
              case "spawn_failed":
                return t("backendErrors.spawnFailed", {
                  agent: agentLabel,
                  message: e.message,
                })
              case "download_failed":
                return t("backendErrors.downloadFailed", {
                  agent: agentLabel,
                  message: e.message,
                })
              case "turn_failed_refusal":
                return t("backendErrors.turnFailedRefusal", {
                  agent: agentLabel,
                })
              case "turn_failed_max_tokens":
                return t("backendErrors.turnFailedMaxTokens", {
                  agent: agentLabel,
                })
              case "turn_failed_max_turn_requests":
                return t("backendErrors.turnFailedMaxTurnRequests", {
                  agent: agentLabel,
                })
              case "turn_failed_unknown":
                return t("backendErrors.turnFailedUnknown", {
                  agent: agentLabel,
                })
              case "turn_failed_empty":
                return t("backendErrors.turnFailedEmpty", {
                  agent: agentLabel,
                })
              case "grok_model_switch_incompatible_agent":
                return t("backendErrors.grokModelSwitchIncompatibleAgent", {
                  agent: agentLabel,
                })
              default:
                return e.message
            }
          })()

          dispatch({ type: "ERROR", contextKey, message: localizedMessage })
          pushAlertRef.current("error", t("eventErrorTitle"), localizedMessage)
          // Send OS notification for agent errors
          if (nc) {
            const fn = folderNameRef.current
            const title = fn ? `${fn} - Codeg` : "Codeg"
            sendSystemNotification(
              title,
              t("notificationError", {
                agent: agentLabel,
                message: localizedMessage,
              })
            ).catch(() => {})
          }
          break
        }
        case "session_load_failed": {
          flushStreamingQueue()
          // Localize via the stable `code` field (currently only
          // "resource_not_found" — JSON-RPC -32002). Fall back to the raw
          // agent message so an unknown future code still surfaces something
          // intelligible rather than getting swallowed.
          const nc = storeRef.current.connections.get(contextKey)
          const agentLabel = nc ? AGENT_LABELS[nc.agentType] : ""
          const localizedMessage = (() => {
            switch (e.code) {
              case "resource_not_found":
                return t("backendErrors.sessionLoadResourceNotFound", {
                  agent: agentLabel,
                })
              case "session_unavailable":
                return t("backendErrors.sessionLoadUnavailable", {
                  agent: agentLabel,
                })
              default:
                return e.message
            }
          })()
          dispatch({
            type: "ACP_LOAD_ERROR",
            contextKey,
            message: localizedMessage,
          })
          break
        }
        case "available_commands":
          flushStreamingQueue()
          dispatch({
            type: "AVAILABLE_COMMANDS",
            contextKey,
            commands: e.commands,
          })
          break
        case "usage_update":
          flushStreamingQueue()
          dispatch({
            type: "USAGE_UPDATE",
            contextKey,
            usage: {
              used: e.used,
              size: e.size,
            },
          })
          break
      }
    },
    [
      dispatch,
      enqueueStreamingAction,
      flushPendingToolCallUpdates,
      flushStreamingQueue,
      scheduleToolCallUpdateFlush,
      t,
      tChat,
    ]
  )

  // Apply a single envelope to the store. Shared by the legacy global
  // listener and the attach-protocol per-subscription handlers so dedup +
  // dispatch ordering + JS subscriber fan-out stays identical between
  // the two paths.
  const applyMappedEnvelope = useCallback(
    (contextKey: string, envelope: EventEnvelope) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (conn && envelope.seq <= conn.lastAppliedSeq) return
      lastActivityRef.current.set(contextKey, Date.now())
      handleMappedEvent(contextKey, envelope)
      dispatch({ type: "EVENT_APPLIED", contextKey, seq: envelope.seq })
      for (const ref of eventSubscribersRef.current) {
        try {
          ref.current(envelope)
        } catch (err) {
          console.error("[acp-context] subscriber threw:", err)
        }
      }
    },
    [dispatch, handleMappedEvent]
  )

  // Re-seed `DelegationProvider` bindings from a snapshot's active_delegations.
  // `delegation_started` / `delegation_completed` are transient — they mutate
  // no SessionState field, so they are NOT in `to_snapshot()` and (on the
  // snapshot attach path) are never replayed. Without this, a web/server client
  // that cold-attaches, re-attaches after a broadcast lag, or refreshes
  // mid-delegation never establishes the live binding: the card shows a
  // premature "completed" and no "查看会话" until the child finally finishes.
  // We synthesize the same envelopes the broker emits live and fan them ONLY to
  // the JS event subscribers (DelegationProvider), bypassing applyMappedEnvelope
  // so we neither run the store reducer (which has no case for these) nor touch
  // `lastAppliedSeq` / trip the seq-dedup. Idempotent with any live/replayed
  // event for the same `parent_tool_use_id` (DelegationProvider overwrites the
  // binding and `attachDelegationChild` early-returns when already attached).
  const seedDelegationsFromSnapshot = useCallback(
    (
      connectionId: string,
      activeDelegations: ActiveDelegationState[],
      eventSeq: number
    ) => {
      const envelopes = buildDelegationSeedEnvelopes(
        connectionId,
        activeDelegations,
        eventSeq
      )
      for (const envelope of envelopes) {
        for (const ref of eventSubscribersRef.current) {
          try {
            ref.current(envelope)
          } catch (err) {
            console.error(
              "[acp-context] delegation seed subscriber threw:",
              err
            )
          }
        }
      }
    },
    []
  )

  // Open a Subscribe-with-Snapshot stream for `connectionId` and route its
  // frames into the store under `contextKey`. Returns the subscription
  // handle for cleanup, or `null` when the active transport doesn't
  // implement the attach protocol (caller falls back to the legacy
  // snapshot-fetch + global-listener flow).
  //
  // The subscription survives WS reconnects automatically — see
  // `WebEventStream.reattachAll`. Detach reasons are handled here:
  //   - lagged / server_shutdown: re-attach with current cursor so the
  //     consumer doesn't have to think about transient disconnects
  //   - connection_gone: terminal; clean up store entry and let the next
  //     user interaction surface the failure
  const setupAttachSubscription = useCallback(
    (
      contextKey: string,
      connectionId: string,
      sinceSeq: number | undefined
    ): EventStreamSubscription | null => {
      const stream = getEventStream()
      if (!stream) return null

      let activeSub: EventStreamSubscription | null = null
      const handlers: AttachHandlers = {
        onSnapshot: (snapshot) => {
          const patch = denormalizeSnapshot(snapshot)
          dispatch({ type: "HYDRATE_FROM_SNAPSHOT", contextKey, patch })
          lastActivityRef.current.set(contextKey, Date.now())
          // Recover delegation bindings the snapshot carries but the transient
          // events don't (the load-bearing fix for the web-only "running shows
          // completed / no 查看会话" bug). Uses the snapshot's own connection_id
          // as the parent id.
          seedDelegationsFromSnapshot(
            patch.connectionId,
            patch.activeDelegations,
            patch.eventSeq
          )
        },
        onReplay: (events) => {
          for (const envelope of events) {
            applyMappedEnvelope(contextKey, envelope)
          }
        },
        onEvent: (envelope) => {
          applyMappedEnvelope(contextKey, envelope)
        },
        onDetached: (reason) => {
          if (reason === "lagged" || reason === "server_shutdown") {
            // Transient: re-attach with the latest cursor so we either
            // replay the gap (small) or hydrate fresh (large). For
            // server_shutdown the WS is closed, so the new attach frame
            // queues until reconnect; for lagged the WS is still open.
            const conn = storeRef.current.connections.get(contextKey)
            const newSinceSeq = conn?.lastAppliedSeq
            const newSub = stream.attach(
              connectionId,
              { sinceSeq: newSinceSeq },
              handlers
            )
            activeSub = newSub
            attachSubscriptionsRef.current.set(contextKey, newSub)
            return
          }
          // connection_gone: backend GC'd the connection. Mirror to UI
          // so the user sees the conversation tab go away rather than
          // staring at stale state forever.
          attachSubscriptionsRef.current.delete(contextKey)
          dispatch({ type: "CONNECTION_REMOVED", contextKey })
        },
      }

      activeSub = stream.attach(connectionId, { sinceSeq }, handlers)
      attachSubscriptionsRef.current.set(contextKey, activeSub)
      return activeSub
    },
    [applyMappedEnvelope, dispatch, seedDelegationsFromSnapshot]
  )

  // Tear down an attach subscription: detach the WS subscription so the
  // server-side forwarder task exits, and clear the local handle.
  // Idempotent — safe to call from disconnect, idle sweep, REKEY, and
  // REMOVE_ALL paths without checking whether a sub exists. No-op for
  // legacy (Tauri) connections that never went through
  // `setupAttachSubscription`.
  const teardownAttachSubscription = useCallback((contextKey: string) => {
    const sub = attachSubscriptionsRef.current.get(contextKey)
    if (!sub) return
    attachSubscriptionsRef.current.delete(contextKey)
    try {
      sub.detach()
    } catch (err) {
      console.warn("[acp-context] attach detach threw:", err)
    }
  }, [])

  // Single global event listener
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | null = null

    // Web / remote-desktop transports: the backend no longer fans ACP
    // events through the WS firehose (Phase 5 dropped the `acp://event`
    // channel; per-connection attach streams are the sole delivery path).
    // Skip the legacy listener entirely — keeping it would register a
    // dead WebSocket subscription and waste a slot on every reconnect.
    // `waitForListenerReady` becomes an immediate no-op since the path
    // it was guarding (Tauri's app.emit handshake) doesn't exist here.
    if (getEventStream() !== null) {
      listenerReadyRef.current = true
      resolveListenerReadyWaiters()
      return
    }

    listenerReadyRef.current = false

    subscribe<EventEnvelope>("acp://event", (envelope) => {
      // Tauri webview path: the desktop frontend receives ACP events here
      // via `app.emit("acp://event", ...)`. Web / remote-desktop transports
      // skipped this useEffect above and route ACP events solely via the
      // per-connection attach streams.
      const contextKey = reverseMapRef.current.get(envelope.connection_id)
      if (!contextKey) {
        bufferUnmappedEvent(envelope)
        return
      }

      // Seq dedup: skip envelopes already accounted for by a snapshot or a
      // prior delivery. snapshot.event_seq sets the lower bound; subsequent
      // envelopes with seq <= lastAppliedSeq are no-op duplicates.
      const conn = storeRef.current.connections.get(contextKey)
      if (conn && envelope.seq <= conn.lastAppliedSeq) {
        return
      }

      // Touch activity on every incoming event
      lastActivityRef.current.set(contextKey, Date.now())
      handleMappedEvent(contextKey, envelope)

      // Advance lastAppliedSeq after the event's effects have dispatched.
      // EVENT_APPLIED is idempotent (only advances if higher).
      dispatch({
        type: "EVENT_APPLIED",
        contextKey,
        seq: envelope.seq,
      })

      // Fan out to JS-level subscribers (e.g. ConversationDetailPanel's
      // background turn_complete handler). Runs AFTER the reducer dispatches
      // and AFTER seq dedup, so subscribers see a consistent, deduped stream.
      // Unmapped events return early above and never reach here. One bad
      // subscriber must not kill the others — wrap each call in try/catch.
      for (const ref of eventSubscribersRef.current) {
        try {
          ref.current(envelope)
        } catch (err) {
          console.error("[acp-context] subscriber threw:", err)
        }
      }
    })
      .then((fn) => {
        if (cancelled) {
          fn()
        } else {
          unlisten = fn
          listenerReadyRef.current = true
          resolveListenerReadyWaiters()
        }
      })
      .catch(() => {
        listenerReadyRef.current = true
        resolveListenerReadyWaiters()
      })

    return () => {
      cancelled = true
      listenerReadyRef.current = false
      resolveListenerReadyWaiters()
      if (flushTimerRef.current !== null) {
        clearTimeout(flushTimerRef.current)
        flushTimerRef.current = null
      }
      unlisten?.()
    }
  }, [
    bufferUnmappedEvent,
    dispatch,
    handleMappedEvent,
    resolveListenerReadyWaiters,
  ])

  // ── Backend keepalive timer ──
  // Frontend is the only side that knows which conversation tabs the
  // user has open. Without this, the backend's idle sweep
  // (CODEG_ACP_IDLE_TIMEOUT_SECS, default 180s) would reap connections
  // backing visible tabs whenever the user was just reading without
  // sending — forcing them to re-spawn the agent on next message.
  // Touching only bumps last_activity_at; it does not emit any event.
  useEffect(() => {
    const timer = setInterval(() => {
      const currentActiveKey = storeRef.current.activeKey
      const currentOpenTabKeys = openTabKeysRef.current
      const seen = new Set<string>()
      const toTouch: string[] = []
      const consider = (contextKey: string) => {
        if (seen.has(contextKey)) return
        seen.add(contextKey)
        const conn = storeRef.current.connections.get(contextKey)
        if (!conn) return
        // Prompting is already sweep-protected on the backend; touching
        // is harmless but redundant. Connecting hasn't reached the
        // sweep-eligible state yet. Only Connected matters.
        if (conn.status !== "connected") return
        toTouch.push(conn.connectionId)
      }
      if (currentActiveKey) consider(currentActiveKey)
      for (const contextKey of currentOpenTabKeys) consider(contextKey)
      for (const connectionId of toTouch) {
        acpTouchConnection(connectionId).catch(() => {})
      }
    }, CONNECTION_KEEPALIVE_INTERVAL_MS)

    return () => clearInterval(timer)
  }, [])

  // ── Idle sweep timer ──
  // Complements the backend keepalive: this sweep targets connections
  // that are NOT in `openTabKeys ∪ {activeKey}` — i.e. connections the
  // frontend opened but is no longer surfacing to the user (panel
  // dismissed, navigated away). The backend's own idle sweep would
  // reap them on its 60s cadence regardless; doing it here too keeps
  // the React store free of stale entries and triggers an explicit
  // disconnect rather than waiting for the backend's own timeout.
  // Connections backing currently-open tabs are never reaped here —
  // those are kept alive by the keepalive loop above.
  useEffect(() => {
    const timer = setInterval(() => {
      const now = Date.now()
      const currentActiveKey = storeRef.current.activeKey

      const currentOpenTabKeys = openTabKeysRef.current
      const toDisconnect: { contextKey: string; connectionId: string }[] = []
      for (const [contextKey, conn] of storeRef.current.connections) {
        if (contextKey === currentActiveKey) continue
        if (currentOpenTabKeys.has(contextKey)) continue
        if (conn.status === "prompting" || conn.status === "connecting") {
          continue
        }
        if (conn.status !== "connected") continue
        // Delegation children are owned by the broker — the
        // delegation_completed event is the only signal that should
        // tear them down (via detachDelegationChild). The idle sweep
        // would otherwise call acpDisconnect on a backend connection
        // still mid-prompt for the parent's tool_use.
        if (conn.isDelegationChild) continue
        // Viewers don't own their backend connection — acpDisconnect here
        // would kill another client's agent. The viewer is torn down when its
        // tab unmounts (disconnect's isViewer branch detaches it).
        if (conn.isViewer) continue
        // Launched-but-unresolved background work (async sub-agent /
        // background shell): disconnecting would kill the agent CLI and the
        // background task with it. The backend watcher settles or max-age
        // expires the accounting and emits `outstanding: 0`, which re-arms
        // this sweep for the connection.
        if (conn.backgroundOutstanding > 0) continue
        const lastActive = lastActivityRef.current.get(contextKey) ?? 0
        if (now - lastActive > CONNECTION_IDLE_TIMEOUT_MS) {
          toDisconnect.push({
            contextKey,
            connectionId: conn.connectionId,
          })
        }
      }

      for (const { contextKey, connectionId } of toDisconnect) {
        acpDisconnect(connectionId).catch(() => {})
        reverseMapRef.current.delete(connectionId)
        teardownAttachSubscription(contextKey)
        lastActivityRef.current.delete(contextKey)
        pendingUnmappedEventsRef.current.delete(connectionId)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
      }
    }, IDLE_SWEEP_INTERVAL_MS)

    return () => clearInterval(timer)
  }, [dispatch, teardownAttachSubscription])

  // Disconnect all on unmount
  useEffect(() => {
    const reverseMap = reverseMapRef.current
    const attachSubs = attachSubscriptionsRef.current
    // Capture the store ref at effect-setup time so the cleanup
    // function doesn't read a moving target (`storeRef.current` is the
    // same object across renders by design, but the lint rule
    // `react-hooks/exhaustive-deps` flags reading it inside cleanup
    // because in the general case a ref's `.current` can be replaced).
    const store = storeRef.current
    return () => {
      for (const [connectionId, contextKey] of reverseMap) {
        // Delegation-child entries are not real user-facing
        // connections — the broker owns their backend lifecycle and
        // will tear them down when the parent's delegation resolves.
        // Calling acpDisconnect on them here would race the broker's
        // own one-shot teardown and emit a benign-but-noisy "unknown
        // connection" error from the backend.
        const conn = store.connections.get(contextKey)
        if (conn?.isDelegationChild) continue
        // Viewers attach to a connection another client owns — never
        // acpDisconnect it on our unmount. The attach-sub detach loop below
        // releases our read-only subscription cleanly.
        if (conn?.isViewer) continue
        acpDisconnect(connectionId).catch(() => {})
      }
      for (const [, sub] of attachSubs) {
        try {
          sub.detach()
        } catch {
          // best-effort during teardown
        }
      }
    }
  }, [])

  // True when this client already owns (or is already viewing) the given
  // backend connection: some store entry references it, or the desktop
  // firehose already routes it via the reverse-map. Guards the discovery gate
  // from demoting an owner to a viewer on a re-render — a viewer never
  // `acpDisconnect`s, so a mis-tagged owner would leak its agent process.
  const isConnectionOwnedLocally = useCallback((connectionId: string) => {
    if (reverseMapRef.current.has(connectionId)) return true
    for (const conn of storeRef.current.connections.values()) {
      if (conn.connectionId === connectionId) return true
    }
    return false
  }, [])

  // Attach this client to a backend connection ANOTHER client owns
  // (cross-client live streaming). The viewer is a NON-OWNING, co-controlling
  // client: it streams the same turn and may also drive the shared agent
  // (sendPrompt/cancel go to the owner's connection, serialized server-side by
  // its prompt_lock; turn-level concurrency rejection is tracked as a
  // follow-up). The one hard invariant: a viewer's teardown DETACHES, never
  // `acpDisconnect`s — that would kill the owner's agent. Generalizes
  // `attachDelegationChild`: Subscribe-with-Snapshot attach on web, snapshot-
  // hydrate + firehose reverse-map on desktop.
  //
  // ALWAYS a COLD attach (no `sinceSeq`): the viewer has applied no prior
  // events, so it must receive a full snapshot of the in-flight turn — passing
  // the discovered `event_seq` as a cursor could yield only a post-cursor
  // replay and miss all earlier live state. Reconnects re-attach with the
  // running `lastAppliedSeq` (see `setupAttachSubscription.onDetached`).
  const connectAsViewer = useCallback(
    async (
      contextKey: string,
      connectionId: string,
      agentType: AgentType,
      workingDir: string | null
    ) => {
      dispatch({
        type: "CONNECTION_CREATED",
        contextKey,
        connectionId,
        agentType,
        workingDir,
        isViewer: true,
      })
      lastActivityRef.current.set(contextKey, Date.now())

      const stream = getEventStream()
      if (stream) {
        // Web / remote: the per-connection WS attach delivers snapshot +
        // replay + live events atomically over the same socket.
        setupAttachSubscription(contextKey, connectionId, undefined)
        return
      }

      // Desktop firehose: the global `acp://event` stream only carries FUTURE
      // events, so fetch a snapshot to backfill the in-flight turn, then route
      // this connection's events via the reverse-map and drain anything that
      // arrived while the snapshot was in flight. Mirrors the legacy owner
      // path in `connect()`.
      let patch: import("@/lib/snapshot-denormalize").SnapshotPatch | null =
        null
      try {
        const snapshot = await acpGetSessionSnapshot(connectionId)
        if (snapshot) patch = denormalizeSnapshot(snapshot)
      } catch (e) {
        console.warn(
          "[acp-context] viewer snapshot fetch failed for",
          connectionId,
          e
        )
      }
      // Detach race: the tab may have disconnected (disconnect() removed the
      // entry) while the snapshot fetch was in flight. Re-check the store still
      // holds THIS viewer connection BEFORE applying the snapshot, seeding
      // delegations, or installing firehose routing — otherwise we'd hydrate /
      // seed child streams / route for a viewer no one is watching anymore.
      if (
        storeRef.current.connections.get(contextKey)?.connectionId !==
        connectionId
      ) {
        return
      }
      if (patch) {
        dispatch({ type: "HYDRATE_FROM_SNAPSHOT", contextKey, patch })
        seedDelegationsFromSnapshot(
          patch.connectionId,
          patch.activeDelegations,
          patch.eventSeq
        )
      }
      reverseMapRef.current.set(connectionId, contextKey)
      for (const env of consumeBufferedEvents(connectionId)) {
        applyMappedEnvelope(contextKey, env)
      }
    },
    [
      applyMappedEnvelope,
      consumeBufferedEvents,
      dispatch,
      seedDelegationsFromSnapshot,
      setupAttachSubscription,
    ]
  )

  const connect = useCallback(
    async (
      contextKey: string,
      agentType: AgentType,
      workingDir?: string,
      sessionId?: string,
      conversationId?: number
    ) => {
      const request: ConnectRequest = {
        agentType,
        workingDir,
        sessionId,
        conversationId,
      }
      if (connectingKeysRef.current.has(contextKey)) {
        pendingConnectRequestsRef.current.set(contextKey, request)
        return
      }
      connectingKeysRef.current.add(contextKey)

      try {
        // Preflight: read agent status and block if the SDK / binary is
        // not installed. The session page must never trigger a download
        // or install — if the agent is not ready, prompt the user to
        // install it from Agent Settings instead.
        let configuredAgent: AcpAgentStatus | null = null
        try {
          configuredAgent = await acpGetAgentStatus(agentType)
        } catch (error) {
          const reason = t("unableReadAgentConfig", {
            message: normalizeErrorMessage(error),
          })
          const failedTitle = t("connectFailedTitle", {
            agent: AGENT_LABELS[agentType],
          })
          pushAlertRef.current(
            "error",
            failedTitle,
            `${reason}\n${t("agentsSetupHint")}`,
            [buildOpenAgentsSettingsAction(agentType)]
          )
          throw createAlertedError(reason)
        }

        const blocked = resolveConnectBlockState(configuredAgent)
        if (blocked.kind !== "none") {
          const failedTitle = t("connectFailedTitle", {
            agent: AGENT_LABELS[agentType],
          })
          const detail =
            blocked.kind === "sdk_missing"
              ? t("withSetupHint", {
                  message: blocked.reason,
                  hint: t("agentsSetupHint"),
                })
              : `${blocked.reason}\n${t("agentsSetupHint")}`
          pushAlertRef.current(
            "error",
            blocked.kind === "sdk_missing" ? blocked.reason : failedTitle,
            detail,
            [buildOpenAgentsSettingsAction(agentType)]
          )
          throw createAlertedError(blocked.reason)
        }

        const nextWorkingDir = workingDir ?? null
        const existing = storeRef.current.connections.get(contextKey)
        if (existing) {
          if (
            existing.agentType === agentType &&
            existing.workingDir === nextWorkingDir &&
            existing.status !== "disconnected" &&
            existing.status !== "error"
          ) {
            return
          }
          if (
            existing.status !== "disconnected" &&
            existing.status !== "error"
          ) {
            // A viewer doesn't own the backend connection — detach only, never
            // acpDisconnect (that would kill the owner's agent). Owners are
            // disconnected normally before re-spawning under new params.
            if (!existing.isViewer) {
              await acpDisconnect(existing.connectionId).catch(() => {})
            }
            reverseMapRef.current.delete(existing.connectionId)
            teardownAttachSubscription(contextKey)
            lastActivityRef.current.delete(contextKey)
            pendingUnmappedEventsRef.current.delete(existing.connectionId)
          }
        }

        // Orphan rescue: when no entry exists at this contextKey but an
        // alive connection with the same sessionId exists at another
        // contextKey, rekey instead of creating a fresh backend connection.
        // This handles tab close+reopen for newly-created conversations:
        // the original tab's contextKey (e.g. "new-XXXX") differs from
        // the canonical sidebar-reopen contextKey (e.g. "conv-{folderId}-
        // {agent}-{convId}"), and the orphaned connection holds the
        // in-flight live state (live_message, pending_permission, etc.)
        // that we want to preserve across the remount.
        if (!existing && sessionId) {
          let orphanKey: string | null = null
          let orphanConn: ConnectionState | null = null
          for (const [key, conn] of storeRef.current.connections) {
            if (key === contextKey) continue
            if (
              conn.sessionId === sessionId &&
              conn.agentType === agentType &&
              conn.workingDir === nextWorkingDir &&
              conn.status !== "disconnected" &&
              conn.status !== "error"
            ) {
              orphanKey = key
              orphanConn = conn
              break
            }
          }
          if (orphanKey && orphanConn) {
            reverseMapRef.current.set(orphanConn.connectionId, contextKey)
            const lastActivity = lastActivityRef.current.get(orphanKey)
            lastActivityRef.current.delete(orphanKey)
            lastActivityRef.current.set(contextKey, lastActivity ?? Date.now())
            if (storeRef.current.activeKey === orphanKey) {
              setActiveKey(contextKey)
            }
            // Migrate any active attach subscription from the orphan key to
            // the new key. The handlers' contextKey was captured by closure
            // at attach time, so a simple Map rename would leave events
            // dispatching to the (now-removed) orphan key. Detach + re-attach
            // with the current cursor is correct: the attach response is
            // either a (possibly empty) replay or a fresh snapshot, both
            // converge on the same state.
            const orphanCursor = orphanConn.lastAppliedSeq
            teardownAttachSubscription(orphanKey)
            dispatch({
              type: "REKEY_CONNECTION",
              fromKey: orphanKey,
              toKey: contextKey,
            })
            setupAttachSubscription(
              contextKey,
              orphanConn.connectionId,
              orphanCursor
            )
            return
          }
        }

        // Cross-client viewer attach. Before spawning a NEW backend agent, ask
        // whether another client already holds a LIVE connection for this
        // persisted conversation; if so, attach to it as a (co-controlling)
        // both clients stream the same in-flight turn (fixes desktop→browser
        // streaming). Only for real persisted conversations (id > 0) — a
        // brand-new conversation has no live owner yet, so we spawn + own.
        // Best-effort: a discovery failure falls through to the owner spawn.
        if (conversationId != null && conversationId > 0) {
          let discovered: ConversationConnectionInfo | null = null
          try {
            // Pass sessionId so discovery can fall back to external_id when the
            // live owner hasn't bound its conversation_id yet (pre-first-prompt
            // window) — without it a second client would reuse the owner's
            // connection as a mis-tagged owner and kill it on tab close. The
            // external_id fallback is matched WITH agentType (external_id is
            // unique only per agent).
            discovered = await acpFindConnectionForConversation(
              conversationId,
              sessionId,
              agentType
            )
          } catch (e) {
            console.warn(
              "[acp-context] connection discovery failed for conversation",
              conversationId,
              e
            )
          }
          // Discovery awaited: re-check the abandon/supersede guards in case a
          // disconnect() or a newer connect() for this key landed meanwhile
          // (mirrors the post-acpConnect guards below). The finally block
          // clears connectingKeys/abandoned, so a bare return is safe.
          if (abandonedKeysRef.current.has(contextKey)) {
            return
          }
          const pendingAfterDiscovery =
            pendingConnectRequestsRef.current.get(contextKey)
          if (
            pendingAfterDiscovery &&
            !sameConnectRequest(pendingAfterDiscovery, request)
          ) {
            return
          }
          if (
            discovered &&
            !isConnectionOwnedLocally(discovered.connection_id)
          ) {
            await connectAsViewer(
              contextKey,
              discovered.connection_id,
              agentType,
              nextWorkingDir
            )
            return
          }
        }

        // Wait for the legacy global listener to register so Tauri's drain
        // path picks up any events emitted between acpConnect returning
        // and reverseMap.set below. Web/remote use attach which doesn't
        // need this gate, but the wait is a fast no-op once the initial
        // subscribe resolves.
        await waitForListenerReady()
        // Ship the user's saved selector preferences (mode + per-config
        // values, persisted per agentType in localStorage) up to the backend
        // at connect time. The backend applies them on the freshly-attached
        // session before emitting `session_modes` / `session_config_options`,
        // so by the time the frontend sees those events (or a snapshot frame
        // on the Subscribe-with-Snapshot attach), `current_mode_id` and
        // `current_value` already reflect the user's preferences. This
        // eliminates the prior "intercept event → overwrite locally → sync
        // back to agent" path, which fixed new-conversation flow but quietly
        // regressed when the snapshot path replaced the event path on tab
        // re-open (the snapshot frame doesn't carry a `session_modes` event,
        // so the apply-on-event hook never fired).
        const savedPrefs = getSavedPrefsForConnect(agentType)
        const connectionId = await acpConnect(
          agentType,
          workingDir,
          sessionId,
          savedPrefs.modeId,
          savedPrefs.configValues
        )

        // If disconnect was requested while connect was in flight,
        // tear down immediately instead of registering the connection.
        if (abandonedKeysRef.current.delete(contextKey)) {
          acpDisconnect(connectionId).catch(() => {})
          return
        }
        const pendingRequest = pendingConnectRequestsRef.current.get(contextKey)
        if (pendingRequest && !sameConnectRequest(pendingRequest, request)) {
          acpDisconnect(connectionId).catch(() => {})
          return
        }

        lastActivityRef.current.set(contextKey, Date.now())
        dispatch({
          type: "CONNECTION_CREATED",
          contextKey,
          connectionId,
          agentType,
          workingDir: nextWorkingDir,
        })

        // Subscribe-with-Snapshot path. When the active transport supports
        // the attach protocol (currently web mode), the per-connection WS
        // stream delivers snapshot + replay + live events atomically — no
        // separate snapshot HTTP fetch, no reverse-map, no unmapped buffer.
        // Returns null on transports without attach support; we fall
        // through to the legacy snapshot+global-listener path below.
        const attachSub = setupAttachSubscription(
          contextKey,
          connectionId,
          undefined
        )
        if (attachSub) {
          // Done — the EventStream handles snapshot, replay, live events,
          // and reconnect entirely in-band over the same WS.
        } else {
          // Legacy path (Tauri desktop, RemoteDesktop): same flow as
          // before Phase 3. Awaits snapshot HTTP first, then registers
          // reverseMap, then drains any envelopes that arrived on the
          // global listener while the snapshot was in flight.
          let snapshotPatch:
            | import("@/lib/snapshot-denormalize").SnapshotPatch
            | null = null
          try {
            const snapshot = await acpGetSessionSnapshot(connectionId)
            if (snapshot) {
              snapshotPatch = denormalizeSnapshot(snapshot)
            }
          } catch (e: unknown) {
            console.warn(
              "[acp-context] snapshot fetch failed for",
              connectionId,
              e
            )
          }

          if (snapshotPatch) {
            dispatch({
              type: "HYDRATE_FROM_SNAPSHOT",
              contextKey,
              patch: snapshotPatch,
            })
            // Recover delegation bindings from the snapshot here too. On
            // Tauri the firehose also delivers the events (so this is an
            // idempotent no-op), but it keeps RemoteDesktop and the legacy
            // path symmetric with the attach path above.
            seedDelegationsFromSnapshot(
              snapshotPatch.connectionId,
              snapshotPatch.activeDelegations,
              snapshotPatch.eventSeq
            )
          }

          reverseMapRef.current.set(connectionId, contextKey)

          const buffered = consumeBufferedEvents(connectionId)
          if (buffered.length > 0) {
            for (const event of buffered) {
              applyMappedEnvelope(contextKey, event)
            }
          }
        }
      } catch (err) {
        const pendingRequest = pendingConnectRequestsRef.current.get(contextKey)
        const superseded =
          pendingRequest != null && !sameConnectRequest(pendingRequest, request)
        if (!superseded && !isAlertedError(err)) {
          const message = normalizeErrorMessage(err)
          const agentLabel = AGENT_LABELS[agentType]
          // Backend safety net: if the agent turned out to be not
          // installed (e.g. the binary was removed between preflight
          // and spawn), surface the same install prompt with a direct
          // "Open Agent Settings" action. Title is localized via the
          // same i18n key the preflight path uses.
          //
          // INVARIANT: `AcpError::SdkNotInstalled` renders its payload
          // unchanged, and both producers
          // (`src-tauri/src/commands/acp.rs::verify_agent_installed`
          // and `src-tauri/src/acp/connection.rs::build_agent` Binary
          // branch) format the message with the literal English
          // substring "is not installed". Do NOT translate those two
          // format strings — this branch matches on them as a stable
          // identifier, since `AcpError::Serialize` flattens to a bare
          // message string and does not expose the error `code` for
          // synchronous Tauri command rejections.
          if (message.includes("is not installed")) {
            pushAlertRef.current(
              "error",
              t("blocked.sdkMissing", { agent: agentLabel }),
              t("agentsSetupHint"),
              [buildOpenAgentsSettingsAction(agentType)]
            )
          } else {
            pushAlertRef.current(
              "error",
              t("connectFailedTitle", { agent: agentLabel }),
              message
            )
          }
        }
        if (!superseded) {
          throw err
        }
      } finally {
        connectingKeysRef.current.delete(contextKey)
        abandonedKeysRef.current.delete(contextKey)
        const pendingRequest = pendingConnectRequestsRef.current.get(contextKey)
        if (pendingRequest) {
          pendingConnectRequestsRef.current.delete(contextKey)
          if (!sameConnectRequest(pendingRequest, request)) {
            queueMicrotask(() => {
              connectRef
                .current?.(
                  contextKey,
                  pendingRequest.agentType,
                  pendingRequest.workingDir,
                  pendingRequest.sessionId,
                  pendingRequest.conversationId
                )
                .catch(() => {})
            })
          }
        }
      }
    },
    [
      applyMappedEnvelope,
      buildOpenAgentsSettingsAction,
      connectAsViewer,
      consumeBufferedEvents,
      dispatch,
      isConnectionOwnedLocally,
      resolveConnectBlockState,
      seedDelegationsFromSnapshot,
      setActiveKey,
      setupAttachSubscription,
      t,
      teardownAttachSubscription,
      waitForListenerReady,
    ]
  )
  connectRef.current = connect

  const disconnect = useCallback(
    async (contextKey: string) => {
      pendingConnectRequestsRef.current.delete(contextKey)
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        // connect() is still in flight — mark as abandoned so it
        // tears down immediately when acpConnect returns.
        if (connectingKeysRef.current.has(contextKey)) {
          abandonedKeysRef.current.add(contextKey)
        }
        return
      }
      if (conn.isViewer) {
        // Viewer teardown: drop our read-only attachment WITHOUT
        // `acpDisconnect` — the backend connection belongs to another client,
        // and disconnecting it would kill the owner's agent mid-turn. Mirrors
        // detachDelegationChild. The owner's own disconnect / the idle sweep
        // governs the connection's real lifetime.
        teardownAttachSubscription(contextKey)
        reverseMapRef.current.delete(conn.connectionId)
        pendingUnmappedEventsRef.current.delete(conn.connectionId)
        lastActivityRef.current.delete(contextKey)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
        return
      }
      await acpDisconnect(conn.connectionId)
      reverseMapRef.current.delete(conn.connectionId)
      teardownAttachSubscription(contextKey)
      lastActivityRef.current.delete(contextKey)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
      dispatch({ type: "CONNECTION_REMOVED", contextKey })
    },
    [dispatch, teardownAttachSubscription]
  )

  const reapplyConfig = useCallback(
    async (contextKey: string): Promise<boolean> => {
      const conn = storeRef.current.connections.get(contextKey)
      // Viewers / delegation children don't own the backend process — restarting
      // would kill another client's (or the broker's) agent. The banner hides
      // its restart button for them, but guard here too. Return false so the
      // caller doesn't show a false "applied" confirmation on this no-op.
      if (!conn || conn.isViewer || conn.isDelegationChild) return false
      // Capture identity BEFORE teardown. `sessionId` is what makes the new
      // process resume this conversation (session/load) rather than start fresh.
      const { agentType, workingDir, sessionId } = conn
      await disconnect(contextKey)
      await connect(
        contextKey,
        agentType,
        workingDir ?? undefined,
        sessionId ?? undefined
      )
      return true
    },
    [connect, disconnect]
  )

  const dismissConfigStale = useCallback(
    (contextKey: string) => {
      dispatch({ type: "DISMISS_CONFIG_STALE", contextKey })
    },
    [dispatch]
  )

  const disconnectAll = useCallback(async () => {
    const promises: Promise<void>[] = []
    pendingConnectRequestsRef.current.clear()
    for (const [contextKey, conn] of storeRef.current.connections) {
      // Viewers attach to a connection another client owns — detach our
      // read-only subscription but never acpDisconnect (that would kill the
      // owner's agent). Owners are torn down normally.
      if (!conn.isViewer) {
        promises.push(acpDisconnect(conn.connectionId).catch(() => {}))
      }
      reverseMapRef.current.delete(conn.connectionId)
      teardownAttachSubscription(contextKey)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
    }
    lastActivityRef.current.clear()
    await Promise.all(promises)
    dispatch({ type: "REMOVE_ALL" })
  }, [dispatch, teardownAttachSubscription])

  const sendPrompt = useCallback(
    async (
      contextKey: string,
      blocks: PromptInputBlock[],
      opts?: {
        folderId?: number | null
        conversationId?: number | null
        clientMessageId?: string | null
      }
    ) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) return
      lastActivityRef.current.set(contextKey, Date.now())
      await acpPrompt(
        conn.connectionId,
        blocks,
        opts?.folderId ?? null,
        opts?.conversationId ?? null,
        opts?.clientMessageId ?? null
      )
    },
    []
  )

  const setMode = useCallback(async (contextKey: string, modeId: string) => {
    const conn = storeRef.current.connections.get(contextKey)
    if (!conn) return
    // Persist user's mode selection to localStorage
    const modes =
      conn.modes ?? selectorsCache.get(conn.agentType)?.modes ?? null
    if (modes) {
      saveModePreference(conn.agentType, {
        ...modes,
        current_mode_id: modeId,
      })
    }
    lastActivityRef.current.set(contextKey, Date.now())
    await acpSetMode(conn.connectionId, modeId)
  }, [])

  const setConfigOption = useCallback(
    async (contextKey: string, configId: string, valueId: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) return
      dispatch({
        type: "CONFIG_OPTION_CHANGED",
        contextKey,
        configId,
        valueId,
      })
      // Persist user selection to localStorage so the next `acp_connect`
      // can ship it back to the backend as a preferred config value.
      saveConfigPreference(conn.agentType, configId, valueId)
      lastActivityRef.current.set(contextKey, Date.now())
      await acpSetConfigOption(conn.connectionId, configId, valueId)
    },
    [dispatch]
  )

  const cancel = useCallback(async (contextKey: string) => {
    const conn = storeRef.current.connections.get(contextKey)
    if (!conn) return
    await acpCancel(conn.connectionId)
  }, [])

  const respondPermission = useCallback(
    async (contextKey: string, requestId: string, optionId: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        console.error(
          "[AcpConnections] respondPermission: no connection for",
          contextKey
        )
        return
      }
      try {
        lastActivityRef.current.set(contextKey, Date.now())
        await acpRespondPermission(conn.connectionId, requestId, optionId)
        dispatch({ type: "PERMISSION_CLEARED", contextKey, requestId })
      } catch (e) {
        console.error("[AcpConnections] respondPermission failed:", e)
        throw e
      }
    },
    [dispatch]
  )

  const answerQuestion = useCallback(
    async (contextKey: string, questionId: string, answer: QuestionAnswer) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        // Throw, don't silently return: AskQuestionCard awaits this and holds a
        // disabled in-flight state (spinner) until it resolves, only re-enabling
        // on rejection. A silent resolve here would leave the card stuck. The
        // throw routes to the card's retryable inline error instead.
        throw new Error(
          `[AcpConnections] answerQuestion: no connection for ${contextKey}`
        )
      }
      try {
        lastActivityRef.current.set(contextKey, Date.now())
        await acpAnswerQuestion(conn.connectionId, questionId, answer)
        // Optimistically clear; the backend also broadcasts question_resolved
        // (idempotent on the matched id).
        dispatch({ type: "CLEAR_ASK_QUESTION", contextKey, questionId })
      } catch (e) {
        console.error("[AcpConnections] answerQuestion failed:", e)
        throw e
      }
    },
    [dispatch]
  )

  const attachDelegationChild = useCallback(
    (args: {
      connectionId: string
      parentConnectionId: string
      parentToolUseId: string
      agentType: AgentType
    }) => {
      const { connectionId, parentConnectionId, parentToolUseId, agentType } =
        args
      const existing = storeRef.current.connections.get(connectionId)
      if (
        existing &&
        existing.isDelegationChild &&
        existing.connectionId === connectionId
      ) {
        // Already attached; just refresh activity so the idle sweep
        // doesn't trip on a duplicate delegation_started event.
        lastActivityRef.current.set(connectionId, Date.now())
        return
      }
      dispatch({
        type: "DELEGATION_CHILD_ATTACH",
        contextKey: connectionId,
        connectionId,
        agentType,
        parentConnectionId,
        parentToolUseId,
      })
      lastActivityRef.current.set(connectionId, Date.now())

      const stream = getEventStream()
      if (stream) {
        // Web / remote transport: open a per-connection attach so the
        // child's snapshot + replay + live events flow through the
        // standard handlers. This is independent of any user-driven
        // tab attach because contextKey == connectionId for children.
        setupAttachSubscription(connectionId, connectionId, undefined)
        return
      }

      // Tauri desktop: the global acp://event listener routes by
      // reverseMap. Register the identity mapping and drain any
      // envelopes that arrived between the child's spawn and now.
      reverseMapRef.current.set(connectionId, connectionId)
      const buffered = consumeBufferedEvents(connectionId)
      for (const env of buffered) {
        applyMappedEnvelope(connectionId, env)
      }
    },
    [
      applyMappedEnvelope,
      consumeBufferedEvents,
      dispatch,
      setupAttachSubscription,
    ]
  )

  const detachDelegationChild = useCallback(
    (connectionId: string) => {
      const existing = storeRef.current.connections.get(connectionId)
      if (!existing || !existing.isDelegationChild) return
      teardownAttachSubscription(connectionId)
      reverseMapRef.current.delete(connectionId)
      pendingUnmappedEventsRef.current.delete(connectionId)
      lastActivityRef.current.delete(connectionId)
      dispatch({ type: "DELEGATION_CHILD_DETACH", contextKey: connectionId })
    },
    [dispatch, teardownAttachSubscription]
  )

  const actions = useMemo<AcpActionsValue>(
    () => ({
      connect,
      disconnect,
      disconnectAll,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      respondPermission,
      answerQuestion,
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
      registerLiveMessageSink,
      clearAcpLoadError,
      attachDelegationChild,
      detachDelegationChild,
      reapplyConfig,
      dismissConfigStale,
    }),
    [
      connect,
      disconnect,
      disconnectAll,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      respondPermission,
      answerQuestion,
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
      registerLiveMessageSink,
      clearAcpLoadError,
      attachDelegationChild,
      detachDelegationChild,
      reapplyConfig,
      dismissConfigStale,
    ]
  )

  const eventSubscriberApi = useMemo<AcpEventSubscriberApi>(
    () => ({ subscribers: eventSubscribersRef.current }),
    []
  )

  return (
    <AcpActionsContext.Provider value={actions}>
      <ConnectionStoreContext.Provider value={storeApi}>
        <AcpEventSubscriberContext.Provider value={eventSubscriberApi}>
          {children}
        </AcpEventSubscriberContext.Provider>
      </ConnectionStoreContext.Provider>
    </AcpActionsContext.Provider>
  )
}
