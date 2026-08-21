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
import { toast } from "sonner"
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
  acpGoalControl,
  acpCancel,
  acpRespondPermission,
  acpAnswerQuestion,
  acpAnswerPlanApproval,
  acpDisconnect,
  acpTouchConnection,
  acpGetSessionSnapshot,
  acpFindConnectionForConversation,
} from "@/lib/api"
import { denormalizeSnapshot } from "@/lib/snapshot-denormalize"
import { buildDelegationSeedEnvelopes } from "@/lib/delegation-seed"
import {
  isConnectionBusy,
  isConnectionGoneError,
} from "@/lib/connection-teardown"
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
  PendingPlanApprovalState,
  PlanApprovalAnswer,
  SessionConfigOptionInfo,
  SessionFailureRecord,
  SessionModeStateInfo,
  SessionUsageUpdateInfo,
  PromptCapabilitiesInfo,
  PromptInputBlock,
  ToolCallImageWire,
  UserMessageBlock,
} from "@/lib/types"
import {
  dismissSessionFailures,
  hasSettleableRetryIncident,
  mergeSessionFailures,
  settleSessionFailures,
  upsertSessionFailure,
  type SessionFailureSettleScope,
} from "@/lib/session-failures"
import { getAgentLabel } from "@/lib/custom-agents"
import {
  CONNECTION_IDLE_TIMEOUT_MS,
  CONNECTION_KEEPALIVE_INTERVAL_MS,
  IDLE_SWEEP_INTERVAL_MS,
} from "@/lib/constants"
import { sendSystemNotification } from "@/lib/notification"
import {
  playEventSound,
  primeNotificationSoundOutput,
  withEventSoundsSuppressed,
} from "@/lib/notification-sound"
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
  /** Requests queued behind this card (only one shows at a time). */
  queued?: number
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
  /**
   * `parentToolUseId`: subagent attribution (claude-agent-acp ≥0.63,
   * `_meta.claudeCode.parentToolUseId`). Parented text/thinking belongs to
   * the live Agent capsule of that tool call — the runtime store routes it
   * out of the main thread. `undefined` = main-thread content.
   */
  | { type: "text"; text: string; parentToolUseId?: string }
  | { type: "thinking"; text: string; parentToolUseId?: string }
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
  /** Awaiting-decision Grok `exit_plan_mode` approval (the plan the agent is
   *  blocked on). Set from a `plan_approval_request` event or a snapshot's
   *  `pending_plan_approval`; cleared on `plan_approval_resolved` or turn end. */
  pendingPlanApproval: PendingPlanApprovalState | null
  claudeApiRetry: ClaudeApiRetryState | null
  /** AIR typed session failure table (see `lib/session-failures.ts` for the
   *  merge/settle contract). Retained resolved — entries double as per-id
   *  revision watermarks; the banner splits active from resolved itself. */
  sessionFailures: SessionFailureRecord[]
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
   * The count itself is never rendered — it is a busy signal. Non-zero exempts
   * the connection from the frontend idle sweep and the unmount/preview
   * teardowns (killing the connection kills the agent CLI and the background
   * work with it), and marks a manual reconnect destructive so the status
   * popover warns before interrupting that work.
   */
  backgroundOutstanding: number
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
      // One AIR typed session-failure upsert (`session_failure` event).
      // Merged monotonically by id+revision; see `lib/session-failures.ts`.
      type: "SESSION_FAILURE"
      contextKey: string
      record: SessionFailureRecord
    }
  | {
      // Lifecycle settle for the AIR failure table (mirrors
      // `SessionState::apply_event`). `retry_incidents` rides turn PROGRESS —
      // fresh output proves the adapter reconnected. `warnings` is dispatched
      // from the `turn_complete` handler on a CLEAN (`end_turn`) end only: a
      // cancelled/failed exit ended a turn that did NOT recover, so its
      // warnings must stay active.
      type: "SETTLE_SESSION_FAILURES"
      contextKey: string
      scope: SessionFailureSettleScope
    }
  | {
      // The user closed a strip. Client-local, like `DISMISS_CONFIG_STALE`.
      // Takes every id that strip stood for: the collapsed warning bar closes
      // its hidden siblings with it.
      type: "DISMISS_SESSION_FAILURES"
      contextKey: string
      ids: string[]
    }
  | {
      // Mirror of a `background_activity` event's `outstanding` count (the
      // backend transcript watcher's authoritative accounting) onto the
      // connection, where it gates the teardowns. No-op when the count didn't
      // change, so repeat events don't re-render connection consumers.
      type: "SET_BACKGROUND_OUTSTANDING"
      contextKey: string
      outstanding: number
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
      queued?: number
    }
  | {
      type: "PERMISSION_QUEUE_DEPTH"
      contextKey: string
      depth: number
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
  | {
      type: "SET_PLAN_APPROVAL"
      contextKey: string
      pendingPlanApproval: PendingPlanApprovalState
    }
  | {
      type: "CLEAR_PLAN_APPROVAL"
      contextKey: string
      /** When present, only clear if the current approval_id matches (guards a
       *  late `plan_approval_resolved` from wiping a freshly-raised approval). */
      approvalId?: string
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
  | {
      type: "CONTENT_DELTA"
      contextKey: string
      text: string
      parentToolUseId?: string
    }
  | {
      type: "THINKING"
      contextKey: string
      text: string
      parentToolUseId?: string
    }

type ConnectionsMap = Map<string, ConnectionState>
const MAX_LIVE_TOOL_RAW_OUTPUT_CHARS = 200_000
const MAX_BUFFERED_UNMAPPED_EVENTS_PER_CONNECTION = 64
const MAX_BUFFERED_UNMAPPED_CONNECTIONS = 128
/**
 * How many times a user-driven `reconnect` will wait for an in-flight
 * `connect()` on the same key before giving up and rebuilding anyway. Small on
 * purpose: this only absorbs a connect that was already running (or the one
 * connect() itself re-dispatches for a superseded request), and a key that
 * keeps reconnecting on its own must not hang the button forever.
 */
const MAX_RECONNECT_SETTLE_WAITS = 3
/**
 * How long each of those waits will hold. A connect settles by resolving its
 * IPC, so one that never answers would otherwise park the reconnect FOREVER —
 * and a wedged connect is the very state this button is clicked from. Generous
 * enough to cover a real agent spawn (the wait exists to let that finish);
 * expiring just returns the button to the user to try again.
 */
const CONNECT_SETTLE_WAIT_TIMEOUT_MS = 15_000

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

  // Orphan gate for subagent-attributed chunks: the parent Agent tool_call
  // always precedes its subagent's chunks on the seq-ordered wire (and
  // `tool_call` dispatch flushes the streaming queue first), so a parented
  // delta whose parent is absent from liveMessage is out-of-turn residue —
  // e.g. an async subagent still streaming after its parent turn settled.
  // Dropping it here keeps liveMessage, its runtime-store sinks, and
  // COMPLETE_TURN promotion consistent, and bounds memory (orphan text never
  // accumulates).
  if (action.parentToolUseId) {
    const parentPresent = conn.liveMessage?.content.some(
      (b) =>
        b.type === "tool_call" && b.info.tool_call_id === action.parentToolUseId
    )
    if (!parentPresent) return null
  }

  const prev = ensureLiveMessage(conn.liveMessage)
  const lastBlock = prev.content[prev.content.length - 1]
  let newContent: LiveContentBlock[] | null = null

  // Merge only into a trailing block of the same kind AND the same subagent
  // attribution — main → subagent → main must produce three blocks. Mirrors
  // the backend's `append_text_delta` predicate so a snapshot-hydrated client
  // converges on identical block boundaries.
  if (action.type === "CONTENT_DELTA") {
    if (
      lastBlock?.type === "text" &&
      lastBlock.parentToolUseId === action.parentToolUseId
    ) {
      newContent = [
        ...prev.content.slice(0, -1),
        {
          type: "text",
          text: lastBlock.text + action.text,
          parentToolUseId: action.parentToolUseId,
        },
      ]
    } else {
      newContent = [
        ...prev.content,
        {
          type: "text",
          text: action.text,
          parentToolUseId: action.parentToolUseId,
        },
      ]
    }
  } else {
    if (
      action.text.length === 0 &&
      lastBlock?.type === "thinking" &&
      lastBlock.parentToolUseId === action.parentToolUseId
    ) {
      // Already have a thinking block of this attribution; an empty
      // follow-up event is a no-op. (A parented empty chunk must not
      // suppress the main thread's "Thinking..." placeholder, nor vice
      // versa — hence the attribution check.)
      return null
    }
    if (
      lastBlock?.type === "thinking" &&
      lastBlock.parentToolUseId === action.parentToolUseId
    ) {
      newContent = [
        ...prev.content.slice(0, -1),
        {
          type: "thinking",
          text: lastBlock.text + action.text,
          parentToolUseId: action.parentToolUseId,
        },
      ]
    } else {
      newContent = [
        ...prev.content,
        {
          type: "thinking",
          text: action.text,
          parentToolUseId: action.parentToolUseId,
        },
      ]
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
        pendingPlanApproval: null,
        claudeApiRetry: null,
        sessionFailures: [],
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
        pendingPlanApproval: null,
        claudeApiRetry: null,
        sessionFailures: [],
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
      // AIR failure records merge on BOTH branches: the per-id monotonic rule
      // is idempotent and can only add or upgrade entries, never clobber a
      // fresher live one — so even a stale-by-eventSeq snapshot may safely
      // contribute records this client attached too late to see live.
      const mergedSessionFailures = mergeSessionFailures(
        current.sessionFailures,
        action.patch.sessionFailures
      )

      if (action.patch.eventSeq <= current.lastAppliedSeq) {
        if (
          mergedSelectorsReady === current.selectorsReady &&
          mergedSupportsFork === current.supportsFork &&
          mergedModes === current.modes &&
          mergedConfigOptions === current.configOptions &&
          mergedAvailableCommands === current.availableCommands &&
          mergedPromptCapabilities === current.promptCapabilities &&
          mergedSessionFailures === current.sessionFailures
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
          sessionFailures: mergedSessionFailures,
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
        pendingPlanApproval: action.patch.pendingPlanApproval,
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
        // replay for it, so its teardown gates hold.
        backgroundOutstanding: action.patch.backgroundOutstanding,
        sessionFailures: mergedSessionFailures,
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
        // Starting a prompt past an active AIR failure acknowledges it —
        // settle EVERYTHING (watermarks retained). A failure that is still
        // real re-arms via a higher revision on the same id.
        updated.sessionFailures = settleSessionFailures(
          conn.sessionFailures,
          "all"
        )
        // The out-of-turn window ended: its tool-call contexts (kept only for
        // background permission enrichment) are stale for the new turn.
        updated.outOfTurnToolCalls = null
      } else if (conn.status === "prompting") {
        // Prompt cycle ended: clear in-flight Claude API retry banner.
        updated.claudeApiRetry = null
        // AIR failures deliberately NOT settled here: leaving `prompting`
        // covers error/cancel exits too, where the incident did not recover —
        // settling on any exit painted a still-dead connection as a recovered
        // warning. The `turn_complete` handler settles warnings on a clean
        // `end_turn` instead (SETTLE_SESSION_FAILURES), after the response's
        // terminal error escalation (if any) has already landed.
        // A blocked ask_user_question can't outlive its turn. The normal path
        // clears it via `question_resolved`; this is the safety net for a turn
        // that ended without one (agent error / abandoned block).
        updated.pendingAskQuestion = null
        // Likewise a blocked exit_plan_mode approval — cleared via
        // `plan_approval_resolved` normally; this is the turn-end safety net.
        updated.pendingPlanApproval = null
      }
      next.set(action.contextKey, updated)
      return next
    }

    case "SET_BACKGROUND_OUTSTANDING": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (conn.backgroundOutstanding === action.outstanding) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        backgroundOutstanding: action.outstanding,
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
          queued: action.queued,
        },
      })
      return next
    }

    case "PERMISSION_QUEUE_DEPTH": {
      // Depth-only: a request queued up behind the visible card, which emits no
      // PERMISSION_REQUEST of its own. No card up → nothing to annotate (a late
      // depth event after a drain must not resurrect one).
      const conn = state.get(action.contextKey)
      if (!conn?.pendingPermission) return state
      if (conn.pendingPermission.queued === action.depth) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingPermission: {
          ...conn.pendingPermission,
          queued: action.depth,
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

    case "SET_PLAN_APPROVAL": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingPlanApproval: action.pendingPlanApproval,
      })
      return next
    }

    case "CLEAR_PLAN_APPROVAL": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (
        action.approvalId !== undefined &&
        conn.pendingPlanApproval?.approval_id !== action.approvalId
      ) {
        return state
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        pendingPlanApproval: null,
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

    case "SESSION_FAILURE": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const merged = upsertSessionFailure(conn.sessionFailures, action.record)
      // Stale/replayed upserts are rejected by reference — no re-render.
      if (merged === conn.sessionFailures) return state
      const next = new Map(state)
      next.set(action.contextKey, { ...conn, sessionFailures: merged })
      return next
    }

    case "SETTLE_SESSION_FAILURES": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const settled = settleSessionFailures(conn.sessionFailures, action.scope)
      // Nothing needed settling — same reference, no re-render.
      if (settled === conn.sessionFailures) return state
      const next = new Map(state)
      next.set(action.contextKey, { ...conn, sessionFailures: settled })
      return next
    }

    case "DISMISS_SESSION_FAILURES": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const dismissed = dismissSessionFailures(conn.sessionFailures, action.ids)
      // Unknown ids / already resolved — same reference, no re-render.
      if (dismissed === conn.sessionFailures) return state
      const next = new Map(state)
      next.set(action.contextKey, { ...conn, sessionFailures: dismissed })
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
  /**
   * Release the connection for `contextKey`. The LOCAL entry always goes away
   * — a stranded one would make the next `connect()` take its "already
   * connected" fast path onto a session that may be dead.
   *
   * Resolves `true` when the backend teardown is confirmed (including a
   * connection that was already gone), `false` when it failed for any other
   * reason and the agent process may still be alive. Callers that report a
   * restart to the user gate on it; fire-and-forget teardowns ignore it.
   */
  disconnect(contextKey: string): Promise<boolean>
  /**
   * Release a connection whose SURFACE went away on its own (a preview tab
   * replaced by the next single-click in the sidebar) — never a user-intent
   * teardown. Disconnects viewers and idle owners; a busy owner (prompting
   * turn, or unresolved background tasks) is left running for the idle sweep,
   * because `acpDisconnect` kills the agent CLI mid-turn and the agent records
   * that as an interrupted request. Use `disconnect` when the user asked to
   * stop.
   */
  disconnectIfIdle(contextKey: string): Promise<void>
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
  answerPlanApproval(
    contextKey: string,
    approvalId: string,
    answer: PlanApprovalAnswer
  ): Promise<void>
  /** Pause or clear the session's active Codex goal (codex-acp #293). */
  goalControl(contextKey: string, action: "pause" | "clear"): Promise<void>
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
    /**
     * Backfill the in-flight turn from a session snapshot before routing
     * live events. Required when attaching MID-TURN, which the desktop
     * firehose cannot serve on its own: `acp://event` only carries FUTURE
     * events, so without a snapshot the viewer misses everything the turn
     * already produced and its status stays `connected` instead of
     * `prompting` (no streaming affordance, empty live message). Real
     * delegation children attach at `delegation_started` — before the
     * child's first event — and leave this off. No effect on web/remote:
     * the attach protocol always opens with a snapshot.
     */
    hydrate?: boolean
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
   * User-driven reconnect for the composer's connection-status popover, usable
   * in ANY state — unlike `reapplyConfig`, which only restarts a live owner.
   *
   *   * live owner  → disconnect + connect (a restart; a prompting turn dies
   *     with the agent CLI, which is why the popover warns before offering it)
   *   * viewer      → detach + re-run discovery, so it re-attaches (or spawns
   *     its own agent if the previous owner is gone). Never `acpDisconnect`s.
   *   * no entry    → connect with the params `connect()` last recorded for
   *     this key, which is what makes the button work from `disconnected` /
   *     `error`, where the store holds nothing at all.
   *
   * Returns `false` on a no-op: a delegation child (broker-owned) or a key we
   * have no params for (never connected in this session).
   */
  reconnect(contextKey: string): Promise<boolean>
  /**
   * The params `reconnect(contextKey)` would use, or `null` when it would be a
   * no-op. Lets the status popover name the agent and enable its button while
   * NO connection exists. Non-reactive by design — the values only change when
   * `connect()` runs, which also notifies the store.
   */
  getReconnectInfo(contextKey: string): {
    agentType: AgentType
    workingDir: string | null
    sessionId: string | null
  } | null
  /**
   * Dismiss the "restart to apply" banner for the current drift WITHOUT
   * restarting (client-local; the underlying `configStale` is untouched). A
   * subsequent settings change re-shows it. Wired to the banner's X button.
   */
  dismissConfigStale(contextKey: string): void
  /**
   * Close AIR failure strips (client-local, like `dismissConfigStale`) — one
   * call per strip, carrying every record that strip stood for. The records
   * stay in the table as their revision watermarks, so this silences only what
   * was on screen: a failure that is still real re-arms via a higher revision.
   * Unlike the recovery actions this is NOT gated on owning the session — a
   * viewer dismissing a strip only edits its own projection.
   */
  dismissSessionFailures(contextKey: string, ids: string[]): void
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

  // Notification sounds: browsers only open audio output from inside a user
  // gesture, so start watching for one now. The user's ordinary first click in
  // the workspace unlocks it, well before an agent event needs it — otherwise
  // the session's first cue is lost (the Settings preview cannot stand in for
  // it: that is a different window with its own audio context). No-op while
  // sounds are disabled.
  useEffect(() => primeNotificationSoundOutput(), [])

  // Ref-based store — mutations don't trigger React state updates
  const storeRef = useRef<InternalStore>({
    connections: new Map(),
    activeKey: null,
    keyListeners: new Map(),
    activeKeyListeners: new Set(),
  })

  // connectionId → the contextKeys whose events are routed from the legacy
  // global `acp://event` listener. Attach-protocol connections (web mode)
  // bypass this entirely — their events are routed by the per-subscription
  // handlers registered in `attachSubscriptionsRef`.
  //
  // One-to-MANY, deliberately. Several surfaces can watch the SAME backend
  // connection at once: a conversation tab plus the work-task transcript
  // viewer (`attachDelegationChild` routes the task's own connection under
  // `contextKey === connectionId`), two split tiles on one conversation, a
  // viewer attach racing an owner re-spawn. While this was a 1:1 Map the last
  // attach silently STOLE routing from the earlier surface and the first
  // detach DELETED routing the surviving surface still depended on. Either way
  // the abandoned surface keeps a live-looking `prompting` ConnectionState that
  // no event can ever settle — and every recovery path (connect()'s fast
  // return, the idle sweep, the keepalive touch) only acts on
  // `disconnected`/`error`, so the tab is stuck on "responding" with a dead
  // Stop button until the app restarts. Always mutate through
  // `bindConnectionRoute` / `releaseConnectionRoute`.
  const reverseMapRef = useRef(new Map<string, Set<string>>())

  /** Route `connectionId`'s firehose events to `contextKey` too. Idempotent. */
  const bindConnectionRoute = useCallback(
    (connectionId: string, contextKey: string) => {
      const keys = reverseMapRef.current.get(connectionId)
      if (keys) keys.add(contextKey)
      else reverseMapRef.current.set(connectionId, new Set([contextKey]))
    },
    []
  )

  /**
   * Drop ONLY this surface's route. A no-op for a connection this contextKey
   * never routed, and — the load-bearing part — it leaves every OTHER surface
   * watching the same connection routed.
   */
  const releaseConnectionRoute = useCallback(
    (connectionId: string, contextKey: string) => {
      const keys = reverseMapRef.current.get(connectionId)
      if (!keys) return
      keys.delete(contextKey)
      if (keys.size === 0) reverseMapRef.current.delete(connectionId)
    },
    []
  )

  // contextKey → diagnostic evidence already surfaced as an alert. The same
  // error reaches us twice: live on the wire, and again in `last_error` on
  // every re-attach snapshot. Without this a browser refresh would re-raise
  // the alert each time.
  const alertedErrorDetailsRef = useRef(new Map<string, string>())

  // contextKey → active EventStream subscription handle. Populated only for
  // connections established via the Subscribe-with-Snapshot attach
  // protocol (web + remote-desktop). Used to (a) detach on disconnect /
  // tab close, and (b) re-attach with the current cursor when a connection
  // is rekeyed (orphan rescue) so handlers reference the new contextKey.
  const attachSubscriptionsRef = useRef(
    new Map<string, EventStreamSubscription>()
  )

  // contextKey → how many times an entry has been REKEYed OUT of it (orphan
  // rescue moving a connection to its canonical key). `connect()` samples this
  // across its awaits: when its entry is gone afterwards it has to know WHY,
  // and the store's current shape can't say. "Rekeyed away" means another key
  // owns the connection now and this call must stand down — continuing would
  // reach its own orphan rescue and drag the connection back. "Simply gone"
  // (the attach handler's `connection_gone`, a terminal event) means nobody
  // holds it and this call must go on and build one. Inferring either from
  // who currently references the id gets both wrong: two surfaces can legally
  // share an id via backend dedup, and a rekey destination can itself be torn
  // down before the sampler resumes.
  const rekeyGenerationRef = useRef(new Map<string, number>())

  // Open tab keys — updated by child TabProvider via registerOpenTabKeys
  const openTabKeysRef = useRef(new Set<string>())

  // Guard against concurrent connect() calls
  const connectingKeysRef = useRef(new Set<string>())
  const pendingConnectRequestsRef = useRef(new Map<string, ConnectRequest>())
  // Last params `connect()` was called with, per contextKey — kept AFTER the
  // connection is gone (teardown removes the store entry entirely, so a
  // `disconnected` / `error` composer has nothing left to reconnect from).
  // Recorded even for attempts that fail, which is exactly the `error` case the
  // status popover's Reconnect button has to serve.
  //
  // Backend-RESOLVED identity is folded back in as it arrives (see
  // `rememberResolvedIdentity`): a new conversation connects with no sessionId
  // at all, so the request as issued would reconnect into a FRESH session and
  // silently abandon the conversation's history.
  const lastConnectParamsRef = useRef(new Map<string, ConnectRequest>())
  // Keys whose disconnect was requested while connect was still in flight
  const abandonedKeysRef = useRef(new Set<string>())
  // Resolvers waiting for an in-flight connect() on a key to settle. Only a
  // user-driven `reconnect` uses this: connect() parks a same-parameter request
  // in `pendingConnectRequestsRef` and its `finally` then DROPS it as a
  // duplicate, so a reconnect landing mid-connect would vanish silently.
  const connectSettledWaitersRef = useRef(new Map<string, Array<() => void>>())
  const connectRef = useRef<AcpActionsValue["connect"] | null>(null)

  /**
   * Fold backend-resolved identity into the remembered connect params.
   *
   * `connect()` records what the CALLER asked for, and for a new conversation
   * that request carries no `sessionId` / `conversationId` — the backend mints
   * them later and they only ever land on the store entry. But the entry is
   * exactly what disappears when a connection is removed WITHOUT a user
   * teardown (backend GC via `connection_gone`, the idle sweep, the unmount
   * cleanup), which is the main way a composer ends up needing Reconnect. With
   * only the original request left, that button would start a fresh ACP session
   * instead of resuming the conversation.
   *
   * No-op when nothing was remembered for the key: `agentType` alone makes a
   * request reconnectable, and it can only come from `connect()`.
   */
  const rememberResolvedIdentity = useCallback(
    (
      contextKey: string,
      patch: { sessionId?: string; conversationId?: number }
    ) => {
      const remembered = lastConnectParamsRef.current.get(contextKey)
      if (!remembered) return
      lastConnectParamsRef.current.set(contextKey, { ...remembered, ...patch })
    },
    []
  )

  /**
   * Snapshot the live entry's resolved identity into the remembered params
   * immediately BEFORE that entry goes away.
   *
   * Identity reaches the entry by several routes — the `session_started` event,
   * a snapshot hydrate on a cold attach (where the event was already consumed
   * before this client attached, so it is never replayed), a replayed event —
   * but it leaves by exactly one: the entry being removed. Capturing at the
   * single exit covers every route in, including ones added later.
   */
  const captureIdentityBeforeRemoval = useCallback(
    (contextKey: string) => {
      const sessionId = storeRef.current.connections.get(contextKey)?.sessionId
      if (!sessionId) return
      rememberResolvedIdentity(contextKey, { sessionId })
    },
    [rememberResolvedIdentity]
  )

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

      const agentLabel = getAgentLabel(agent.agent_type)
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
        // Claude Code / Codex install a separate ACP adapter package, not the
        // vendor CLI — saying "{agent} is not installed" to someone who has
        // `claude` on their PATH reads as a bug in codeg. Name what's actually
        // missing instead.
        reason: agent.is_acp_adapter
          ? t("blocked.adapterMissing", { agent: agentLabel })
          : t("blocked.sdkMissing", { agent: agentLabel }),
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
      // Same-type AND same subagent attribution: within one flush window,
      // main-thread and parented deltas (or two different subagents') must
      // not concatenate — this pre-coalescing runs BEFORE the reducer's
      // attribution-aware merge and would otherwise defeat it.
      if (
        last &&
        last.type === action.type &&
        last.parentToolUseId === action.parentToolUseId
      ) {
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

  /**
   * Turn PROGRESS settles in-flight AIR retry incidents — codex's own
   * `completeRetryIncidentOnTurnProgress`: a reconnect warning is published
   * when the upstream drops, and the next byte of turn output is the proof it
   * came back. Without this the only settle points are a clean `end_turn` and
   * the next prompt, so a long turn that reconnected N times kept N permanent
   * strips docked under the composer (issue #496).
   *
   * Called per chunk, so it reads the store and dispatches only when something
   * would actually change; the common case is a `some()` over an empty array.
   */
  const settleRetryIncidentsOnProgress = useCallback(
    (contextKey: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn || !hasSettleableRetryIncident(conn.sessionFailures)) return
      dispatch({
        type: "SETTLE_SESSION_FAILURES",
        contextKey,
        scope: "retry_incidents",
      })
    },
    [dispatch]
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

  /**
   * Say so when the agent settled a config-option pick somewhere else.
   *
   * `session/set_config_option` is advisory: the agent answers with the option
   * list it adopted and codeg renders that verbatim, so a refused or downgraded
   * pick reads as the selector springing back for no reason. pi does this for a
   * model whose reasoning it can't honour; grok does it for a model switch
   * mid-conversation.
   *
   * The request/answer correlation is the backend's (`ConfigOptionRejected`) —
   * `acpSetConfigOption` resolves as soon as the command is queued, and the
   * resulting option list arrives as a broadcast indistinguishable from an
   * unsolicited update. This side only renders the verdict.
   *
   * Reporting only — the saved preference deliberately keeps the ATTEMPTED value.
   * A rejection is often about this session rather than the pick itself (grok's
   * mid-conversation switch succeeds in a fresh one).
   */
  const reportConfigOptionVerdict = useCallback(
    (
      agentType: AgentType | undefined,
      rejection: { option_name: string; requested: string; actual: string }
    ) => {
      toast.warning(
        t("configOptionAdjusted", {
          agent: agentType ? getAgentLabel(agentType) : "",
          option: rejection.option_name,
          requested: rejection.requested,
          actual: rejection.actual,
        })
      )
    },
    [t]
  )

  const handleMappedEvent = useCallback(
    (
      contextKey: string,
      e: EventEnvelope,
      /**
       * True when this envelope was ALREADY delivered to another surface in
       * this same fan-out (one backend connection can be routed to several
       * contextKeys — see `reverseMapRef`). Store effects still run per
       * surface: each has its own ConnectionState. Effects that belong to the
       * ENVELOPE rather than to a surface — the notification sound, OS
       * notifications, status-bar alerts, toasts — must fire exactly once, or
       * a tab and the work-task transcript viewer on one session would double
       * every ping.
       */
      echo = false
    ) => {
      // Audible cue for the events the user opted into (Settings → General →
      // notification sounds). One call for the whole catalogue rather than a
      // line per case: the mapping — including which events are cues at all —
      // lives in `soundEventIdForEnvelope`, alongside the preference schema it
      // mirrors. Off unless configured, and self-throttling, so this is a
      // cheap no-op on the hot path.
      if (!echo) playEventSound(e)
      switch (e.type) {
        case "status_changed":
          flushStreamingQueue()
          dispatch({ type: "STATUS_CHANGED", contextKey, status: e.status })
          break
        case "content_delta":
          settleRetryIncidentsOnProgress(contextKey)
          enqueueStreamingAction({
            type: "CONTENT_DELTA",
            contextKey,
            text: e.text,
            // Wire `null` normalizes to `undefined` so the reducer's strict
            // attribution equality works on one representation.
            parentToolUseId: e.parent_tool_use_id ?? undefined,
          })
          break
        case "thinking":
          settleRetryIncidentsOnProgress(contextKey)
          enqueueStreamingAction({
            type: "THINKING",
            contextKey,
            text: e.text,
            parentToolUseId: e.parent_tool_use_id ?? undefined,
          })
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
          settleRetryIncidentsOnProgress(contextKey)
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
        case "permission_queue_depth":
          // A request queued up behind the visible card. Only the count on the
          // already-rendered card changes, so no streaming flush is needed.
          dispatch({
            type: "PERMISSION_QUEUE_DEPTH",
            contextKey,
            depth: e.depth,
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
        case "plan_approval_request":
          // Grok called `exit_plan_mode`: it's blocked on the user's approval of
          // the plan. Flush queued streaming so the card renders against current
          // content, then raise the interactive plan-approval card.
          flushStreamingQueue()
          dispatch({
            type: "SET_PLAN_APPROVAL",
            contextKey,
            pendingPlanApproval: {
              approval_id: e.approval_id,
              tool_call_id: e.tool_call_id,
              plan_markdown: e.plan_markdown,
              created_at: new Date().toISOString(),
            },
          })
          break
        case "plan_approval_resolved":
          // The approval was answered (this or another window) or canceled.
          // Matched by approval_id so a stale event can't wipe a fresh one.
          dispatch({
            type: "CLEAR_PLAN_APPROVAL",
            contextKey,
            approvalId: e.approval_id,
          })
          break
        case "background_activity": {
          // Out-of-turn transcript activity from the backend watcher: async
          // task completions, the agent's continued work after them, cron//
          // loop turns. Three consumers:
          // 1. the outstanding mirror, which gates the teardowns (nothing
          //    renders the count);
          dispatch({
            type: "SET_BACKGROUND_OUTSTANDING",
            contextKey,
            outstanding: e.outstanding,
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
            if (!echo) {
              const nc = storeRef.current.connections.get(contextKey)
              const agentLabel = nc ? getAgentLabel(nc.agentType) : "Agent"
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
            queued: e.queued,
          })
          // Send OS notification when permission approval is needed
          {
            const nc = echo
              ? null
              : storeRef.current.connections.get(contextKey)
            if (nc) {
              const agentLabel = getAgentLabel(nc.agentType)
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
          // The id that turns a later reconnect into a RESUME. It reaches us
          // only here, and only the store entry holds it — which is precisely
          // what a backend GC / idle sweep removes.
          rememberResolvedIdentity(contextKey, { sessionId: e.session_id })
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
          // Same reason as session_started: a reconnect needs the conversation
          // id to be able to attach as a viewer to a surviving owner.
          if (e.conversation_id > 0) {
            rememberResolvedIdentity(contextKey, {
              conversationId: e.conversation_id,
            })
          }
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
        case "config_option_rejected": {
          // Arrives immediately before the `session_config_options` carrying the
          // value the agent actually adopted, so the notice and the selector
          // settle together.
          if (!echo) {
            reportConfigOptionVerdict(
              storeRef.current.connections.get(contextKey)?.agentType,
              e
            )
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
        case "session_failure": {
          // JetBrains AIR typed session failure upsert — merged monotonically
          // by id+revision (stale/replayed records are dropped in the
          // reducer). Rendering + action buttons live in
          // `SessionFailureBanner`; resolution is inferred at turn/prompt
          // boundaries (STATUS_CHANGED).
          dispatch({
            type: "SESSION_FAILURE",
            contextKey,
            record: e.record,
          })
          break
        }
        case "turn_retrying": {
          // codex-acp #289: a retryable turn error keeps the turn alive (codex
          // auto-retries). Reuse the Claude API-retry banner — codex doesn't
          // report attempt/limit/delay, so those stay null; the banner clears at
          // the next turn boundary like the Claude path.
          const retryConn = storeRef.current.connections.get(contextKey)
          dispatch({
            type: "CLAUDE_API_RETRY",
            contextKey,
            retry: {
              sessionId: retryConn?.sessionId ?? "",
              attempt: null,
              maxRetries: null,
              error: e.message,
              errorStatus: e.error_status ?? null,
              retryDelayMs: null,
            },
          })
          break
        }
        case "turn_complete": {
          flushStreamingQueue()
          flushPendingToolCallUpdates()
          // AIR retry warnings settle only at a CLEAN turn end, mirroring the
          // backend's `apply_event`. A failed turn's terminal failure rides
          // the prompt response and was emitted as a `session_failure` event
          // just before this one (same-id higher-revision error escalation),
          // so settling here can no longer paint an unrecovered incident as
          // recovered; any other exit (cancelled/empty/refusal) keeps the
          // warnings active until the next prompt's settle-all.
          //
          // Incidents that recovered MID-turn are already gone (see
          // `settleRetryIncidentsOnProgress`); this boundary catches the ones
          // still in flight at the end, plus the category-"unknown" notices
          // that progress deliberately skips.
          if (e.stop_reason === "end_turn") {
            dispatch({
              type: "SETTLE_SESSION_FAILURES",
              contextKey,
              scope: "warnings",
            })
          }
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
            const nc = echo
              ? null
              : storeRef.current.connections.get(contextKey)
            if (nc) {
              const agentLabel = getAgentLabel(nc.agentType)
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
            ? getAgentLabel(nc.agentType)
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
              case "mcp_rejected_by_agent":
                return t("backendErrors.mcpRejectedByAgent", {
                  agent: agentLabel,
                  message: e.message,
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
              // The agent did emit something, but the backend couldn't parse
              // it — points at an agent/protocol version mismatch rather than
              // at the agent's configuration.
              case "turn_failed_empty_protocol":
                return t("backendErrors.turnFailedEmptyProtocol", {
                  agent: agentLabel,
                })
              // Only metadata (plan / mode / usage) arrived. Reported as an
              // observation, NOT as "this turn was fine" — a real failure can
              // follow a plan or usage update.
              case "turn_failed_empty_metadata":
                return t("backendErrors.turnFailedEmptyMetadata", {
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

          // Backend-supplied diagnostic evidence (agent stderr tail, unparsed
          // update counts), already redacted and bounded there. It rides its
          // own alert slot rather than being concatenated into `detail`, so
          // `StatusBarAlerts` can put it behind a real expander instead of
          // dumping a stderr wall into the alert list. Whitespace-only details
          // collapse to `undefined` so every consumer below (tooltip pointer,
          // alert slot, re-attach dedup) agrees there is nothing to show.
          const evidence = e.details?.trim() || undefined

          // `conn.error` feeds the composer status tooltip — keep it the
          // one-line localized message, never the multi-line evidence. The
          // tooltip has no room for a disclosure, so when there IS evidence,
          // say where it can be opened; otherwise the message would point at
          // an expander the user can't find (and, with no evidence, one that
          // wouldn't be worth finding).
          const tooltipMessage = evidence
            ? `${localizedMessage} ${t("backendErrors.detailsInAlerts")}`
            : localizedMessage
          dispatch({ type: "ERROR", contextKey, message: tooltipMessage })
          if (!echo) {
            pushAlertRef.current(
              "error",
              t("eventErrorTitle"),
              localizedMessage,
              undefined,
              evidence
            )
          }
          // Remember what we surfaced so the snapshot path doesn't repeat it
          // when this client re-attaches. Recorded per surface even on an echo:
          // it is the re-attach dedup key for THIS contextKey, and the alert it
          // suppresses is the one the primary delivery already raised.
          if (evidence) {
            alertedErrorDetailsRef.current.set(contextKey, evidence)
          }
          // Send OS notification for agent errors. Deliberately message-only:
          // notification centers persist their payload outside the app, so
          // agent output must not be forwarded there.
          if (nc && !echo) {
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
          const agentLabel = nc ? getAgentLabel(nc.agentType) : ""
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
      rememberResolvedIdentity,
      reportConfigOptionVerdict,
      scheduleToolCallUpdateFlush,
      settleRetryIncidentsOnProgress,
      t,
      tChat,
    ]
  )

  // Latest-ref for the event handler, so nothing downstream has to depend on
  // `handleMappedEvent`'s identity. It closes over the i18n `t` / `tChat`,
  // which are only stable while the locale is unchanged — a language switch
  // (or any future unstable dep added to the callback) would otherwise churn
  // both the global `acp://event` subscription below and every
  // `setupAttachSubscription` consumer that hangs off `applyMappedEnvelope`.
  // Tauri's `listen` / `unlisten` are both async IPC, so re-running that
  // effect briefly leaves two listeners registered and every envelope is
  // delivered twice. Duplicate delivery is already idempotent — the
  // `lastAppliedSeq` guard below runs before the synchronous `EVENT_APPLIED`
  // dispatch — but the subscription should simply never churn in the first
  // place. See the mount-once regression test.
  const handleMappedEventRef = useRef(handleMappedEvent)
  // Re-sync each render so the latest closure is used at fire time.
  useEffect(() => {
    handleMappedEventRef.current = handleMappedEvent
  })

  // Apply a single envelope to the store. Shared by the legacy global
  // listener and the attach-protocol per-subscription handlers so dedup +
  // dispatch ordering + JS subscriber fan-out stays identical between
  // the two paths.
  const applyMappedEnvelope = useCallback(
    (contextKey: string, envelope: EventEnvelope) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (conn && envelope.seq <= conn.lastAppliedSeq) return
      lastActivityRef.current.set(contextKey, Date.now())
      handleMappedEventRef.current(contextKey, envelope)
      dispatch({ type: "EVENT_APPLIED", contextKey, seq: envelope.seq })
      for (const ref of eventSubscribersRef.current) {
        try {
          ref.current(envelope)
        } catch (err) {
          console.error("[acp-context] subscriber threw:", err)
        }
      }
    },
    [dispatch]
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

  // Surface diagnostic evidence carried by a snapshot's `last_error`.
  //
  // Alerts are live-only, so a client that attached AFTER the error fired
  // (browser refresh, second tab, cold attach mid-session) would otherwise
  // never learn why the turn came back empty — the snapshot is its only
  // channel. Scoped to errors that actually carry evidence, i.e. the inferred
  // `turn_failed_empty*` family, so attaching to a connection with any older
  // error doesn't start raising alerts it never used to.
  //
  // Same routing rules as the live path: evidence goes to the alert only,
  // never to `conn.error` (composer tooltip) and never to an OS notification.
  // Held in a ref, following `pushAlertRef` above: the attach/hydrate
  // callbacks below are deliberately identity-stable (an empty dep array keeps
  // a re-render from tearing down and re-establishing live subscriptions), so
  // they must not close over a `t`-dependent callback directly.
  const surfaceSnapshotErrorDetails = useCallback(
    (
      contextKey: string,
      patch: import("@/lib/snapshot-denormalize").SnapshotPatch
    ) => {
      const evidence = patch.lastErrorDetails?.trim()
      if (!evidence) return
      if (alertedErrorDetailsRef.current.get(contextKey) === evidence) return
      alertedErrorDetailsRef.current.set(contextKey, evidence)
      pushAlertRef.current(
        "error",
        t("eventErrorTitle"),
        patch.lastError ?? undefined,
        undefined,
        evidence
      )
    },
    [t]
  )
  const surfaceSnapshotErrorDetailsRef = useRef(surfaceSnapshotErrorDetails)
  useEffect(() => {
    surfaceSnapshotErrorDetailsRef.current = surfaceSnapshotErrorDetails
  }, [surfaceSnapshotErrorDetails])

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
          surfaceSnapshotErrorDetailsRef.current(contextKey, patch)
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
          // Catching up on a gap (reconnect / lagged detach) re-delivers events
          // that already happened. They belong in the UI, but replaying them
          // must not fire a burst of notification sounds for turns that
          // finished minutes ago.
          withEventSoundsSuppressed(() => {
            for (const envelope of events) {
              applyMappedEnvelope(contextKey, envelope)
            }
          })
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
          // The composer that survives this is exactly the one whose Reconnect
          // button has to resume the session rather than start a new one.
          captureIdentityBeforeRemoval(contextKey)
          dispatch({ type: "CONNECTION_REMOVED", contextKey })
        },
      }

      activeSub = stream.attach(connectionId, { sinceSeq }, handlers)
      attachSubscriptionsRef.current.set(contextKey, activeSub)
      return activeSub
    },
    [
      applyMappedEnvelope,
      captureIdentityBeforeRemoval,
      dispatch,
      seedDelegationsFromSnapshot,
    ]
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
      const routes = reverseMapRef.current.get(envelope.connection_id)
      if (!routes || routes.size === 0) {
        bufferUnmappedEvent(envelope)
        return
      }

      // Deliver to EVERY surface routing this connection (see
      // `reverseMapRef`). Iterated over a copy: a handler may bind or release
      // a route re-entrantly, and mutating the live Set mid-iteration would
      // skip or double-deliver. Each surface keeps its own `lastAppliedSeq`,
      // so they dedup independently.
      let deliveredToAny = false
      for (const contextKey of Array.from(routes)) {
        // Seq dedup: skip envelopes already accounted for by a snapshot or a
        // prior delivery. snapshot.event_seq sets the lower bound; subsequent
        // envelopes with seq <= lastAppliedSeq are no-op duplicates.
        const conn = storeRef.current.connections.get(contextKey)
        if (conn && envelope.seq <= conn.lastAppliedSeq) {
          continue
        }
        // Touch activity on every incoming event
        lastActivityRef.current.set(contextKey, Date.now())
        // `deliveredToAny` doubles as "some surface already ran this
        // envelope's user-facing effects", so the sound / OS notification /
        // alert fire once no matter how many surfaces are watching.
        handleMappedEventRef.current(contextKey, envelope, deliveredToAny)
        deliveredToAny = true

        // Advance lastAppliedSeq after the event's effects have dispatched.
        // EVENT_APPLIED is idempotent (only advances if higher).
        dispatch({
          type: "EVENT_APPLIED",
          contextKey,
          seq: envelope.seq,
        })
      }

      // Fan out to JS-level subscribers (e.g. ConversationDetailPanel's
      // background turn_complete handler). Runs AFTER the reducer dispatches
      // and AFTER seq dedup, so subscribers see a consistent, deduped stream.
      // Unmapped events return early above and never reach here. Fired ONCE
      // per envelope no matter how many surfaces routed it — these subscribers
      // are keyed by connection, not by contextKey, and double-delivery would
      // e.g. re-register a delegation binding. One bad subscriber must not kill
      // the others — wrap each call in try/catch.
      if (!deliveredToAny) return
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
    // Every dep here is a `useCallback(..., [])` — the subscription is
    // registered once per mount and torn down only on unmount. The event
    // handler deliberately isn't a dep; it's reached through
    // `handleMappedEventRef` so a changing closure can't churn the listener.
  }, [bufferUnmappedEvent, dispatch, resolveListenerReadyWaiters])

  /**
   * Ask the backend whether it still holds a live connection under this id.
   * `acp_touch_connection` answers `false` for BOTH "unknown id" and "already
   * terminal", which is exactly the question. A transport failure is
   * inconclusive, so it counts as alive — a flaky IPC must never settle a
   * healthy streaming session.
   */
  const isConnectionLiveOnBackend = useCallback(
    async (connectionId: string): Promise<boolean> => {
      try {
        return await acpTouchConnection(connectionId)
      } catch {
        return true
      }
    },
    []
  )

  /**
   * Settle a ConnectionState whose backend connection is gone but whose
   * terminal event never arrived.
   *
   * Without this the entry is IMMORTAL: `prompting` / `connecting` are skipped
   * by the idle sweep and (previously) by the keepalive touch, `connect()` and
   * `handleFocus` only act on `disconnected`/`error`, and `cancel()` swallows
   * the backend's "connection not found" — so the tab shows "responding" with a
   * dead Stop button until the app restarts. Flipping to `disconnected` (rather
   * than dropping the entry) keeps the agent/session identity around, so the
   * composer's Reconnect and the next `connect()` resume this session instead
   * of starting a new one.
   *
   * No-op unless the entry still points at THIS connection, so a settle racing
   * a re-spawn can't knock out the replacement.
   */
  const markConnectionGone = useCallback(
    (contextKey: string, connectionId: string): boolean => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn || conn.connectionId !== connectionId) return false
      if (conn.status === "disconnected" || conn.status === "error")
        return false
      releaseConnectionRoute(connectionId, contextKey)
      teardownAttachSubscription(contextKey)
      pendingUnmappedEventsRef.current.delete(connectionId)
      dispatch({ type: "STATUS_CHANGED", contextKey, status: "disconnected" })
      return true
    },
    [dispatch, releaseConnectionRoute, teardownAttachSubscription]
  )

  // ── Backend keepalive + liveness reconciliation timer ──
  // Frontend is the only side that knows which conversation tabs the
  // user has open. Without this, the backend's idle sweep
  // (CODEG_ACP_IDLE_TIMEOUT_SECS, default 180s) would reap connections
  // backing visible tabs whenever the user was just reading without
  // sending — forcing them to re-spawn the agent on next message.
  // Touching only bumps last_activity_at; it does not emit any event.
  //
  // The touch doubles as a liveness probe, and every non-terminal state is
  // probed — not just `connected`. A `prompting` entry whose terminal event
  // went missing is otherwise unreachable: no sweep re-checks it and
  // `connect()` treats it as already connected, so the tab sits on
  // "responding" with a dead Stop button. `false` means the backend has no
  // live connection under that id, which is exactly the condition to settle.
  useEffect(() => {
    const timer = setInterval(() => {
      const currentActiveKey = storeRef.current.activeKey
      const currentOpenTabKeys = openTabKeysRef.current
      const seen = new Set<string>()
      const toTouch: { contextKey: string; connectionId: string }[] = []
      const consider = (contextKey: string) => {
        if (seen.has(contextKey)) return
        seen.add(contextKey)
        const conn = storeRef.current.connections.get(contextKey)
        if (!conn) return
        if (conn.status === "disconnected" || conn.status === "error") return
        // Broker-owned children come and go on the parent's schedule and are
        // released by `detachDelegationChild`; settling one here would fight
        // that lifecycle.
        if (conn.isDelegationChild) return
        toTouch.push({ contextKey, connectionId: conn.connectionId })
      }
      if (currentActiveKey) consider(currentActiveKey)
      for (const contextKey of currentOpenTabKeys) consider(contextKey)
      for (const { contextKey, connectionId } of toTouch) {
        void isConnectionLiveOnBackend(connectionId).then((live) => {
          if (!live) markConnectionGone(contextKey, connectionId)
        })
      }
    }, CONNECTION_KEEPALIVE_INTERVAL_MS)

    return () => clearInterval(timer)
  }, [isConnectionLiveOnBackend, markConnectionGone])

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
        releaseConnectionRoute(connectionId, contextKey)
        teardownAttachSubscription(contextKey)
        lastActivityRef.current.delete(contextKey)
        pendingUnmappedEventsRef.current.delete(connectionId)
        // Reclaimed for idleness, not closed: the tab is still open and its
        // Reconnect button must resume this session, not start a new one.
        captureIdentityBeforeRemoval(contextKey)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
      }
    }, IDLE_SWEEP_INTERVAL_MS)

    return () => clearInterval(timer)
  }, [
    captureIdentityBeforeRemoval,
    dispatch,
    releaseConnectionRoute,
    teardownAttachSubscription,
  ])

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
      // A connection can be routed by several surfaces (see `reverseMapRef`);
      // tear it down at most once, and only if at least one of them OWNS it.
      const alreadyTornDown = new Set<string>()
      for (const [connectionId, contextKeys] of reverseMap) {
        for (const contextKey of contextKeys) {
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
          if (alreadyTornDown.has(connectionId)) continue
          alreadyTornDown.add(connectionId)
          acpDisconnect(connectionId).catch(() => {})
        }
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

  // True when this client already OWNS the given backend connection — i.e.
  // holds an entry whose teardown `acpDisconnect`s the agent. Guards the
  // discovery gate from demoting an owner to a viewer on a re-render: a viewer
  // never `acpDisconnect`s, so a mis-tagged owner would leak its agent process.
  //
  // Non-owning entries (viewers, delegation children — the work-task transcript
  // dialog attaches the task's OWN connection that way) are deliberately NOT
  // ownership. Counting them inverted the guard: with the transcript viewer
  // open, a tab opening the same conversation was refused the viewer path,
  // fell through to `acpConnect`, got the SAME backend connection back by
  // reuse, and registered itself as its owner — so closing that tab killed the
  // running task's agent.
  //
  // NOT the predicate for "may I tear this connection down?" — see
  // `isConnectionReferencedLocally`.
  const isConnectionOwnedLocally = useCallback((connectionId: string) => {
    for (const conn of storeRef.current.connections.values()) {
      if (conn.connectionId !== connectionId) continue
      if (conn.isViewer || conn.isDelegationChild) continue
      return true
    }
    return false
  }, [])

  // True when ANY local surface references the connection — owner, viewer,
  // delegation child, or a bare firehose route. This is the teardown guard: an
  // abandoned/superseded `connect()` must only `acpDisconnect` a connection it
  // actually created, and the backend dedups by (agent, cwd, session), so
  // `acpConnect` can hand back one that is already on screen somewhere. A
  // transcript viewer counts here even though it does not count as OWNERSHIP:
  // it does not license a teardown, but it does prove someone is watching, and
  // disconnecting would kill the agent out from under them.
  const isConnectionReferencedLocally = useCallback((connectionId: string) => {
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
  //
  // Returns `false` when the connection turned out to be already gone, so the
  // caller can fall through and spawn/own one instead of leaving a viewer
  // attached to nothing.
  const connectAsViewer = useCallback(
    async (
      contextKey: string,
      connectionId: string,
      agentType: AgentType,
      workingDir: string | null
    ): Promise<boolean> => {
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
        return true
      }

      // Desktop firehose: the global `acp://event` stream only carries FUTURE
      // events, so fetch a snapshot to backfill the in-flight turn, then route
      // this connection's events via the reverse-map and drain anything that
      // arrived while the snapshot was in flight. Mirrors the legacy owner
      // path in `connect()`.
      let patch: import("@/lib/snapshot-denormalize").SnapshotPatch | null =
        null
      // `null` (as opposed to a thrown error) is the backend saying it holds no
      // state under this id — i.e. the connection is definitively gone, not
      // merely unreachable.
      let connectionGone = false
      try {
        const snapshot = await acpGetSessionSnapshot(connectionId)
        if (snapshot) patch = denormalizeSnapshot(snapshot)
        else connectionGone = true
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
        return true
      }
      if (connectionGone) {
        // The owner tore the connection down between discovery and this
        // snapshot. That window is small but routinely hit: a work-task run
        // disconnects within milliseconds of `TurnComplete`. Attaching anyway
        // would strand the tab on `connecting` FOREVER — no event can arrive on
        // a dead id, the idle sweep skips viewers, and `connect()` refuses to
        // retry any entry that isn't `disconnected`/`error`. Drop the stillborn
        // viewer and tell the caller to spawn/own instead.
        releaseConnectionRoute(connectionId, contextKey)
        pendingUnmappedEventsRef.current.delete(connectionId)
        lastActivityRef.current.delete(contextKey)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
        return false
      }
      if (patch) {
        dispatch({ type: "HYDRATE_FROM_SNAPSHOT", contextKey, patch })
        surfaceSnapshotErrorDetailsRef.current(contextKey, patch)
        seedDelegationsFromSnapshot(
          patch.connectionId,
          patch.activeDelegations,
          patch.eventSeq
        )
      }
      bindConnectionRoute(connectionId, contextKey)
      for (const env of consumeBufferedEvents(connectionId)) {
        applyMappedEnvelope(contextKey, env)
      }
      return true
    },
    [
      applyMappedEnvelope,
      bindConnectionRoute,
      consumeBufferedEvents,
      dispatch,
      releaseConnectionRoute,
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
      // Remember BEFORE the in-flight early return and before the preflight can
      // throw: a connect that never produced a store entry is precisely when
      // `reconnect()` has nothing else to go on.
      lastConnectParamsRef.current.set(contextKey, request)
      if (connectingKeysRef.current.has(contextKey)) {
        pendingConnectRequestsRef.current.set(contextKey, request)
        return
      }
      connectingKeysRef.current.add(contextKey)

      // Declared outside the try so the catch below can still tell whether this
      // agent is an ACP adapter when picking its "not installed" wording.
      let configuredAgent: AcpAgentStatus | null = null

      try {
        // Preflight: read agent status and block if the SDK / binary is
        // not installed. The session page must never trigger a download
        // or install — if the agent is not ready, prompt the user to
        // install it from Agent Settings instead.
        try {
          configuredAgent = await acpGetAgentStatus(agentType)
        } catch (error) {
          const reason = t("unableReadAgentConfig", {
            message: normalizeErrorMessage(error),
          })
          const failedTitle = t("connectFailedTitle", {
            agent: getAgentLabel(agentType),
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
            agent: getAgentLabel(agentType),
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
        let existing = storeRef.current.connections.get(contextKey)
        // Stale-state gate. The fast path below trusts any non-terminal status
        // as "already connected", which is what makes a routing-less
        // `prompting` / `connecting` entry unrecoverable — the tab keeps saying
        // "responding" and re-opening it changes nothing, because this very
        // return fires. Those two states are also the only ones no sweep ever
        // re-checks, so verify them against the backend before trusting them.
        // `connected` is deliberately not probed: the keepalive already touches
        // it every cycle and settles it when it goes away.
        if (
          existing &&
          (existing.status === "prompting" || existing.status === "connecting")
        ) {
          const staleId = existing.connectionId
          const rekeysBefore = rekeyGenerationRef.current.get(contextKey) ?? 0
          if (!(await isConnectionLiveOnBackend(staleId))) {
            markConnectionGone(contextKey, staleId)
          }
          // Re-read: the probe awaited, and `markConnectionGone` (or a live
          // event, or a concurrent disconnect) may have moved this entry.
          existing = storeRef.current.connections.get(contextKey)
          if (abandonedKeysRef.current.has(contextKey)) {
            return
          }
          // The entry we were probing LEFT this key while we awaited
          // (`markConnectionGone` keeps it — it only flips the status — and the
          // sweeps skip the two states we probe, so something else moved it).
          // Two very different reasons:
          //   * another key's orphan rescue REKEYED it, and the entry is now
          //     over there. Continuing would reach this call's own orphan
          //     rescue and drag the connection straight back to a surface that
          //     just gave it up. Bail.
          //   * the connection is simply gone — the web attach handler drops
          //     the entry on `connection_gone`. Nothing to protect, and bailing
          //     would leave the tab with no connection and no retry (the
          //     auto-connect effect doesn't re-fire on status changes). Carry
          //     on and build one.
          // Which one happened is HISTORY, not state: `rekeyGenerationRef`
          // records it, because two surfaces can legally share a connection id
          // (backend dedup) and a rekey destination can itself be torn down
          // before we get here — so who currently references the id answers
          // neither question.
          if (
            !existing &&
            (rekeyGenerationRef.current.get(contextKey) ?? 0) !== rekeysBefore
          ) {
            return
          }
          const pendingAfterProbe =
            pendingConnectRequestsRef.current.get(contextKey)
          if (
            pendingAfterProbe &&
            !sameConnectRequest(pendingAfterProbe, request)
          ) {
            return
          }
        }
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
            releaseConnectionRoute(existing.connectionId, contextKey)
            teardownAttachSubscription(contextKey)
            lastActivityRef.current.delete(contextKey)
            pendingUnmappedEventsRef.current.delete(existing.connectionId)
            // Routing is gone, so this entry can no longer be settled by an
            // event. Retire it now rather than counting on the `acpConnect`
            // below to overwrite it: a spawn that throws (agent removed
            // mid-session), is abandoned, or is superseded returns early and
            // would otherwise leave a routing-less non-terminal entry — the
            // same immortal "responding" state this gate exists to prevent.
            captureIdentityBeforeRemoval(contextKey)
            dispatch({ type: "CONNECTION_REMOVED", contextKey })
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
            // The entry MOVES (REKEY_CONNECTION below deletes `orphanKey`), so
            // its route has to move with it — a stale orphan-key route would
            // deliver this connection's events to a contextKey with no entry.
            bindConnectionRoute(orphanConn.connectionId, contextKey)
            releaseConnectionRoute(orphanConn.connectionId, orphanKey)
            // Record that `orphanKey` lost its entry to a rekey, for any
            // connect() of that key currently parked on an await.
            rekeyGenerationRef.current.set(
              orphanKey,
              (rekeyGenerationRef.current.get(orphanKey) ?? 0) + 1
            )
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
            const attached = await connectAsViewer(
              contextKey,
              discovered.connection_id,
              agentType,
              nextWorkingDir
            )
            // Attached (or superseded) — done. Otherwise the connection died
            // between discovery and the attach, so fall through and spawn one
            // rather than leaving a viewer bound to a dead id.
            if (attached) return
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

        // If disconnect was requested while connect was in flight, tear down
        // immediately instead of registering the connection — but tear down
        // ONLY what this connect actually created. The backend dedups by
        // (agent, cwd, session), so `acpConnect` may have handed back a
        // connection this client already holds under another contextKey (a
        // session still running in / behind another tab). Killing that one
        // would end a turn nobody asked to stop; it stays reachable at its own
        // key and is reclaimed by the sweeps.
        // Peek, don't consume: the `finally` clears the flag, and it has to
        // still see it to know this call established nothing (see there).
        if (abandonedKeysRef.current.has(contextKey)) {
          if (!isConnectionReferencedLocally(connectionId)) {
            acpDisconnect(connectionId).catch(() => {})
          }
          return
        }
        const pendingRequest = pendingConnectRequestsRef.current.get(contextKey)
        if (pendingRequest && !sameConnectRequest(pendingRequest, request)) {
          if (!isConnectionReferencedLocally(connectionId)) {
            acpDisconnect(connectionId).catch(() => {})
          }
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
          // Teardown race, same guard `connectAsViewer` applies: the tab may
          // have been disconnected (entry removed) or replaced while the
          // snapshot was in flight. Hydrating or routing past that point would
          // install a route for a contextKey with no entry — a bare route that
          // nothing releases and that makes every later liveness/ownership
          // question about this connection answer from a surface that is gone.
          if (
            storeRef.current.connections.get(contextKey)?.connectionId !==
            connectionId
          ) {
            return
          }

          if (snapshotPatch) {
            dispatch({
              type: "HYDRATE_FROM_SNAPSHOT",
              contextKey,
              patch: snapshotPatch,
            })
            surfaceSnapshotErrorDetailsRef.current(contextKey, snapshotPatch)
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

          bindConnectionRoute(connectionId, contextKey)

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
          const agentLabel = getAgentLabel(agentType)
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
              configuredAgent?.is_acp_adapter
                ? t("blocked.adapterMissing", { agent: agentLabel })
                : t("blocked.sdkMissing", { agent: agentLabel }),
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
        // Read before the clear below: the abandon branches leave the flag set
        // so this is the one place that consumes it.
        const wasAbandoned = abandonedKeysRef.current.has(contextKey)
        connectingKeysRef.current.delete(contextKey)
        abandonedKeysRef.current.delete(contextKey)
        const settledWaiters = connectSettledWaitersRef.current.get(contextKey)
        if (settledWaiters) {
          connectSettledWaitersRef.current.delete(contextKey)
          for (const resolveWaiter of settledWaiters) resolveWaiter()
        }
        const pendingRequest = pendingConnectRequestsRef.current.get(contextKey)
        if (pendingRequest) {
          pendingConnectRequestsRef.current.delete(contextKey)
          // A same-parameter pending request is normally a duplicate of the
          // connection THIS call just established, so it is dropped. An
          // ABANDONED call established nothing, though — `disconnect()` cancels
          // it precisely so it won't — and the queued request is then the only
          // one left to run. Dropping it there is how `reapplyConfig` (and a
          // fast close/reopen of the same key) ended up with no connection at
          // all while still reporting success.
          if (wasAbandoned || !sameConnectRequest(pendingRequest, request)) {
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
      bindConnectionRoute,
      buildOpenAgentsSettingsAction,
      captureIdentityBeforeRemoval,
      connectAsViewer,
      consumeBufferedEvents,
      dispatch,
      isConnectionLiveOnBackend,
      isConnectionOwnedLocally,
      isConnectionReferencedLocally,
      markConnectionGone,
      releaseConnectionRoute,
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
    async (contextKey: string): Promise<boolean> => {
      pendingConnectRequestsRef.current.delete(contextKey)
      // An in-flight connect() must abandon its result whether or not it has
      // already put an entry in the store. It awaits several times after that
      // point (liveness probe, discovery, acpConnect), and each of those
      // resumption points re-checks `abandonedKeys` — so marking only the
      // no-entry case let a mid-flight connect resurrect a surface the caller
      // had just closed: re-attaching a viewer, or spawning an agent for a tab
      // that is gone.
      if (connectingKeysRef.current.has(contextKey)) {
        abandonedKeysRef.current.add(contextKey)
      }
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        return true
      }
      // Before either branch drops the entry: an explicit teardown is also how
      // a `reconnect` starts, and the session it resumes may only ever have
      // been known to the entry (cold attach hydrates it from the snapshot,
      // never from a replayed `session_started`).
      captureIdentityBeforeRemoval(contextKey)
      if (conn.isViewer) {
        // Viewer teardown: drop our read-only attachment WITHOUT
        // `acpDisconnect` — the backend connection belongs to another client,
        // and disconnecting it would kill the owner's agent mid-turn. Mirrors
        // detachDelegationChild. The owner's own disconnect / the idle sweep
        // governs the connection's real lifetime.
        teardownAttachSubscription(contextKey)
        releaseConnectionRoute(conn.connectionId, contextKey)
        pendingUnmappedEventsRef.current.delete(conn.connectionId)
        lastActivityRef.current.delete(contextKey)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
        return true
      }
      // A failed backend teardown must not strand the local entry: propagating
      // would leak the attach subscription and leave an entry that makes the
      // next `connect()` take its "already connected" fast path, which is
      // exactly the dead session a user-driven `reconnect` has to rebuild. So
      // the local release below is unconditional — the same policy the idle
      // sweep and connect()'s own re-spawn teardown already apply.
      //
      // The OUTCOME still has to be honest, though. "Already gone" is a real
      // teardown (another window reaped it, the agent died, the backend
      // restarted); anything else may have left the agent process running, and
      // a caller that announces "restarted" on one of those would be wrong —
      // the follow-up connect can re-attach to the process it believed it had
      // replaced. Report which happened and let the caller decide.
      let tornDown = true
      await acpDisconnect(conn.connectionId).catch((error: unknown) => {
        if (isConnectionGoneError(error)) return
        console.warn("[Acp] backend teardown failed, releasing locally:", error)
        tornDown = false
      })
      releaseConnectionRoute(conn.connectionId, contextKey)
      teardownAttachSubscription(contextKey)
      lastActivityRef.current.delete(contextKey)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
      dispatch({ type: "CONNECTION_REMOVED", contextKey })
      return tornDown
    },
    [
      captureIdentityBeforeRemoval,
      dispatch,
      releaseConnectionRoute,
      teardownAttachSubscription,
    ]
  )

  // Lifecycle release for a surface that vanished on its own — currently the
  // preview tab replaced by the next single-click in the sidebar. `disconnect`
  // stays unconditional because its other callers express user INTENT (agent
  // switch, restart-to-apply, an explicit close); this one must not destroy
  // work nobody asked to stop. Same policy as the unmount cleanup
  // (`shouldDisconnectOnUnmount`): a busy owner keeps running and the idle
  // sweep reclaims it once its turn / background work settles — it is no
  // longer in `openTabKeys`, so nothing else keeps it alive.
  const disconnectIfIdle = useCallback(
    async (contextKey: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      // Owners only: a viewer's disconnect just detaches (it never
      // acpDisconnects), and leaving one attached would leak its subscription
      // — the idle sweep skips viewers.
      if (conn && !conn.isViewer && isConnectionBusy(conn)) return
      await disconnect(contextKey)
    },
    [disconnect]
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
      const tornDown = await disconnect(contextKey)
      await connect(
        contextKey,
        agentType,
        workingDir ?? undefined,
        sessionId ?? undefined
      )
      // Reconnect regardless — the user is left with a working connection
      // either way — but an unconfirmed teardown means the old process may
      // still be alive and holding the OLD config, and `connect()` can land
      // right back on it. Returning false keeps the caller from showing an
      // "applied" confirmation it can't stand behind.
      return tornDown
    },
    [connect, disconnect]
  )

  // Params a reconnect would use: the LIVE connection wins (it carries what the
  // backend actually resolved — notably a sessionId minted after connect), with
  // the remembered request filling in what the store doesn't hold
  // (conversationId, and everything at all once the entry is gone).
  const resolveReconnectRequest = useCallback(
    (contextKey: string): ConnectRequest | null => {
      const conn = storeRef.current.connections.get(contextKey)
      // Broker-owned: its lifetime is the parent's delegation_started /
      // _completed pair, and disconnecting would kill a child the user never
      // spawned. Bail before falling back to any remembered params.
      if (conn?.isDelegationChild) return null
      const remembered = lastConnectParamsRef.current.get(contextKey)
      const agentType = conn?.agentType ?? remembered?.agentType
      if (!agentType) return null
      return {
        agentType,
        workingDir: conn?.workingDir ?? remembered?.workingDir ?? undefined,
        sessionId: conn?.sessionId ?? remembered?.sessionId ?? undefined,
        conversationId: remembered?.conversationId,
      }
    },
    []
  )

  const getReconnectInfo = useCallback(
    (contextKey: string) => {
      const request = resolveReconnectRequest(contextKey)
      if (!request) return null
      return {
        agentType: request.agentType,
        workingDir: request.workingDir ?? null,
        sessionId: request.sessionId ?? null,
      }
    },
    [resolveReconnectRequest]
  )

  // Settle-point for an in-flight connect() on a key. Resolves `true` once that
  // connect finishes (immediately when nothing is connecting), `false` if it
  // has not answered within the timeout — a connect whose IPC never settles
  // must not hold the caller forever.
  const waitForConnectSettled = useCallback(
    (contextKey: string): Promise<boolean> => {
      if (!connectingKeysRef.current.has(contextKey))
        return Promise.resolve(true)
      return new Promise<boolean>((resolve) => {
        const onSettled = () => {
          clearTimeout(timer)
          resolve(true)
        }
        const timer = setTimeout(() => {
          // Drop our resolver so the abandoned wait can't accumulate on a key
          // that keeps failing to settle — including the key itself once the
          // last waiter gives up, since a connect that never answers would
          // otherwise leave the empty list behind for good.
          const waiters = connectSettledWaitersRef.current.get(contextKey)
          const at = waiters?.indexOf(onSettled) ?? -1
          if (waiters && at >= 0) waiters.splice(at, 1)
          if (waiters?.length === 0) {
            connectSettledWaitersRef.current.delete(contextKey)
          }
          resolve(false)
        }, CONNECT_SETTLE_WAIT_TIMEOUT_MS)
        const waiters = connectSettledWaitersRef.current.get(contextKey)
        if (waiters) waiters.push(onSettled)
        else connectSettledWaitersRef.current.set(contextKey, [onSettled])
      })
    },
    []
  )

  const reconnect = useCallback(
    async (contextKey: string): Promise<boolean> => {
      // A connect() already in flight would SWALLOW this one: connect() parks a
      // same-parameter request as pending and its `finally` discards it as a
      // duplicate, so the button would spin once and change nothing — with no
      // store entry yet, the teardown below wouldn't run either. Wait for the
      // attempt to settle and then rebuild: the user asked for a new
      // connection, not to join whatever is already running (they typically
      // click precisely BECAUSE the connecting state is stuck).
      //
      // Bounded rather than looped-to-clear: a key that keeps reconnecting on
      // its own must not hang the button forever, and one more contending
      // connect is what connectingKeysRef already exists to serialise.
      //
      // Each wait is also time-bounded, because the connect this one is stuck
      // behind may never answer at all. Rebuilding anyway would be worse than
      // useless — connect() would park it as a duplicate and drop it — so give
      // up and hand the button back instead of spinning on a wedged IPC.
      for (let i = 0; i < MAX_RECONNECT_SETTLE_WAITS; i++) {
        if (!connectingKeysRef.current.has(contextKey)) break
        if (!(await waitForConnectSettled(contextKey))) return false
      }
      // Resolved AFTER the wait: the connect we just waited on may have minted
      // the sessionId that makes this a resume rather than a fresh session.
      const request = resolveReconnectRequest(contextKey)
      if (!request) return false
      // Tear down first even though connect() would: its "same params, still
      // alive → no-op" fast path would otherwise swallow the whole thing, and
      // this button exists precisely to rebuild a connection whose params did
      // NOT change. `disconnect` detaches viewers without killing the owner's
      // agent, so this stays safe for them too.
      //
      // An unconfirmed teardown is deliberately NOT fatal here: the local entry
      // is released either way, and refusing to reconnect would strand the user
      // on the dead connection this button exists to replace.
      if (storeRef.current.connections.has(contextKey)) {
        await disconnect(contextKey)
      }
      await connect(
        contextKey,
        request.agentType,
        request.workingDir,
        request.sessionId,
        request.conversationId
      )
      return true
    },
    [connect, disconnect, resolveReconnectRequest, waitForConnectSettled]
  )

  const dismissConfigStale = useCallback(
    (contextKey: string) => {
      dispatch({ type: "DISMISS_CONFIG_STALE", contextKey })
    },
    [dispatch]
  )

  const dismissSessionFailuresAction = useCallback(
    (contextKey: string, ids: string[]) => {
      dispatch({ type: "DISMISS_SESSION_FAILURES", contextKey, ids })
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
      teardownAttachSubscription(contextKey)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
    }
    // Every entry is about to be dropped (REMOVE_ALL below), so drop every
    // route with them — including any held by a surface whose entry this loop
    // didn't visit.
    reverseMapRef.current.clear()
    lastActivityRef.current.clear()
    // Context keys are reused across backends, so a surviving entry here would
    // suppress the first snapshot alert of an unrelated session.
    alertedErrorDetailsRef.current.clear()
    // Same reuse hazard: remembered connect params must not let a reconnect
    // resurrect the previous backend's session under a recycled key.
    lastConnectParamsRef.current.clear()
    rekeyGenerationRef.current.clear()
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
      try {
        await acpPrompt(
          conn.connectionId,
          blocks,
          opts?.folderId ?? null,
          opts?.conversationId ?? null,
          opts?.clientMessageId ?? null
        )
      } catch (e) {
        // Same reasoning as `cancel`: the backend disowning this id proves the
        // local state is stale. Settle it (the caller still gets the error and
        // surfaces its toast / rolls back its optimistic turn) so the composer
        // doesn't keep sending into a connection that no longer exists.
        if (isConnectionGoneError(e)) {
          markConnectionGone(contextKey, conn.connectionId)
        }
        throw e
      }
    },
    [markConnectionGone]
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

  const cancel = useCallback(
    async (contextKey: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) return
      try {
        await acpCancel(conn.connectionId)
      } catch (e) {
        // Pressing Stop on a connection the backend no longer has is the
        // clearest evidence that this entry's terminal event went missing —
        // and, before this, the point where the bug became visible: the
        // rejection was logged to the console and the button just did nothing,
        // forever. Settle the state instead so the UI leaves "responding" and
        // offers a reconnect.
        if (isConnectionGoneError(e)) {
          markConnectionGone(contextKey, conn.connectionId)
          return
        }
        throw e
      }
    },
    [markConnectionGone]
  )

  const goalControl = useCallback(
    async (contextKey: string, action: "pause" | "clear") => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) return
      // Fire-and-forget: there is no in-flight card UI to settle (unlike
      // answerQuestion). The resulting goal snapshot arrives as a normal
      // session_info_update, and a wire failure is surfaced by the backend's
      // recoverable Error event — so log here and don't rethrow.
      try {
        lastActivityRef.current.set(contextKey, Date.now())
        await acpGoalControl(conn.connectionId, action)
      } catch (e) {
        console.error("[AcpConnections] goalControl failed:", e)
      }
    },
    []
  )

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

  const answerPlanApproval = useCallback(
    async (
      contextKey: string,
      approvalId: string,
      answer: PlanApprovalAnswer
    ) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        // Throw, don't silently return: PlanApprovalCard awaits this and holds a
        // disabled in-flight state until it resolves. A silent resolve would
        // leave the card stuck; the throw routes to its retryable inline error.
        throw new Error(
          `[AcpConnections] answerPlanApproval: no connection for ${contextKey}`
        )
      }
      try {
        lastActivityRef.current.set(contextKey, Date.now())
        await acpAnswerPlanApproval(conn.connectionId, approvalId, answer)
        // Optimistically clear; the backend also broadcasts
        // plan_approval_resolved (idempotent on the matched id).
        dispatch({ type: "CLEAR_PLAN_APPROVAL", contextKey, approvalId })
      } catch (e) {
        console.error("[AcpConnections] answerPlanApproval failed:", e)
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
      hydrate?: boolean
    }) => {
      const {
        connectionId,
        parentConnectionId,
        parentToolUseId,
        agentType,
        hydrate,
      } = args
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
      // ADDS a route rather than replacing one: the work-task transcript viewer
      // attaches the task's OWN connection through here, and a conversation tab
      // may be watching that same connection.
      const route = () => {
        bindConnectionRoute(connectionId, connectionId)
        for (const env of consumeBufferedEvents(connectionId)) {
          applyMappedEnvelope(connectionId, env)
        }
      }
      if (!hydrate) {
        route()
        return
      }
      // Mid-turn attach: the firehose carries only future events, so backfill
      // the turn already in flight from a snapshot FIRST, then route (same
      // order as `connectAsViewer` — anything that lands while the fetch is in
      // flight stays in the unmapped buffer and is deduped by seq on drain).
      void (async () => {
        let patch: import("@/lib/snapshot-denormalize").SnapshotPatch | null =
          null
        try {
          const snapshot = await acpGetSessionSnapshot(connectionId)
          if (snapshot) patch = denormalizeSnapshot(snapshot)
        } catch (e) {
          console.warn(
            "[acp-context] child snapshot fetch failed for",
            connectionId,
            e
          )
        }
        // The viewer may have closed while the snapshot was in flight —
        // never hydrate or install routing for a detached child.
        const still = storeRef.current.connections.get(connectionId)
        if (!still?.isDelegationChild || still.connectionId !== connectionId) {
          return
        }
        if (patch) {
          dispatch({
            type: "HYDRATE_FROM_SNAPSHOT",
            contextKey: connectionId,
            patch,
          })
          // Same recovery the other three snapshot consumers do
          // (`setupAttachSubscription.onSnapshot`, `connectAsViewer`,
          // `connect()`'s legacy branch): `delegation_started` is transient and
          // never replayed, so a viewer opening onto a turn that ALREADY
          // delegated (the work-task transcript dialog is the case) would
          // otherwise establish no binding — no agent icon/label, no child
          // sub-stream, no "待批准" badge on the sub-agent card. Idempotent
          // against any live event for the same `parent_tool_use_id`.
          seedDelegationsFromSnapshot(
            patch.connectionId,
            patch.activeDelegations,
            patch.eventSeq
          )
        }
        route()
      })()
    },
    [
      applyMappedEnvelope,
      bindConnectionRoute,
      consumeBufferedEvents,
      dispatch,
      seedDelegationsFromSnapshot,
      setupAttachSubscription,
    ]
  )

  const detachDelegationChild = useCallback(
    (connectionId: string) => {
      const existing = storeRef.current.connections.get(connectionId)
      if (!existing || !existing.isDelegationChild) return
      teardownAttachSubscription(connectionId)
      // Release only THIS surface's route (contextKey === connectionId for a
      // child). Deleting the whole entry used to cut off a conversation tab
      // watching the same connection — closing the work-task transcript dialog
      // silently blinded the tab, which then sat on `prompting` forever.
      releaseConnectionRoute(connectionId, connectionId)
      pendingUnmappedEventsRef.current.delete(connectionId)
      lastActivityRef.current.delete(connectionId)
      dispatch({ type: "DELEGATION_CHILD_DETACH", contextKey: connectionId })
    },
    [dispatch, releaseConnectionRoute, teardownAttachSubscription]
  )

  const actions = useMemo<AcpActionsValue>(
    () => ({
      connect,
      disconnect,
      disconnectIfIdle,
      disconnectAll,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      goalControl,
      respondPermission,
      answerQuestion,
      answerPlanApproval,
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
      registerLiveMessageSink,
      clearAcpLoadError,
      attachDelegationChild,
      detachDelegationChild,
      reapplyConfig,
      reconnect,
      getReconnectInfo,
      dismissConfigStale,
      dismissSessionFailures: dismissSessionFailuresAction,
    }),
    [
      connect,
      disconnect,
      disconnectIfIdle,
      disconnectAll,
      sendPrompt,
      setMode,
      setConfigOption,
      cancel,
      goalControl,
      respondPermission,
      answerQuestion,
      answerPlanApproval,
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
      registerLiveMessageSink,
      clearAcpLoadError,
      attachDelegationChild,
      detachDelegationChild,
      reapplyConfig,
      reconnect,
      getReconnectInfo,
      dismissConfigStale,
      dismissSessionFailuresAction,
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
