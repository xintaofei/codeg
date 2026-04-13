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
import { subscribe } from "@/lib/platform"
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
  acpDisconnect,
} from "@/lib/api"
import type {
  AgentType,
  AcpAgentStatus,
  AcpEvent,
  AvailableCommandInfo,
  ConnectionStatus,
  PlanEntryInfo,
  PermissionOptionInfo,
  SessionConfigOptionInfo,
  SessionModeStateInfo,
  SessionUsageUpdateInfo,
  PromptCapabilitiesInfo,
  PromptInputBlock,
} from "@/lib/types"
import { AGENT_LABELS } from "@/lib/types"
import {
  CONNECTION_IDLE_TIMEOUT_MS,
  IDLE_SWEEP_INTERVAL_MS,
} from "@/lib/constants"
import { sendSystemNotification } from "@/lib/notification"
import {
  applySavedModePreference,
  applySavedConfigPreferences,
  saveModePreference,
  saveConfigPreference,
  clearStalePrefs,
} from "@/lib/selector-prefs-storage"
import { useAlertContext, type AlertAction } from "@/contexts/alert-context"
import { useFolderContext } from "@/contexts/folder-context"

// ── Shared types (re-exported for consumers) ──

export interface ToolCallInfo {
  tool_call_id: string
  title: string
  kind: string
  status: string
  content: string | null
  raw_input: string | null
  raw_output_chunks: string[]
  raw_output_total_bytes: number
}

export interface PendingPermission {
  request_id: string
  tool_call: unknown
  options: PermissionOptionInfo[]
}

export interface PendingQuestion {
  tool_call_id: string
  question: string
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
  pendingQuestion: PendingQuestion | null
  error: string | null
}

// ── Reducer actions ──

type Action =
  | {
      type: "CONNECTION_CREATED"
      contextKey: string
      connectionId: string
      agentType: AgentType
    }
  | { type: "CONNECTION_REMOVED"; contextKey: string }
  | { type: "REMOVE_ALL" }
  | {
      type: "STATUS_CHANGED"
      contextKey: string
      status: ConnectionStatus
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
  | { type: "PERMISSION_CLEARED"; contextKey: string }
  | {
      type: "SET_PENDING_QUESTION"
      contextKey: string
      pendingQuestion: PendingQuestion
    }
  | { type: "CLEAR_PENDING_QUESTION"; contextKey: string }
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
  | { type: "ERROR"; contextKey: string; message: string }
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

function serializePermissionToolCall(toolCall: unknown): string | null {
  const record = asRecord(toolCall)
  if (!record) return null
  try {
    // Extract the actual tool input from the nested rawInput/raw_input field
    // rather than serializing the entire permission wrapper (which includes
    // internal fields like content, kind, title, toolCallId).
    const nestedInput = record.rawInput ?? record.raw_input
    if (nestedInput !== undefined && nestedInput !== null) {
      if (typeof nestedInput === "string") return nestedInput
      return JSON.stringify(nestedInput)
    }
    // Fallback: strip wrapper-only fields to avoid rendering internal
    // permission structure as raw text.
    const wrapperKeys = new Set([
      "content",
      "kind",
      "title",
      "toolCallId",
      "tool_call_id",
      "callId",
      "call_id",
      "rawInput",
      "raw_input",
    ])
    const rest: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(record)) {
      if (!wrapperKeys.has(k)) rest[k] = v
    }
    return Object.keys(rest).length > 0
      ? JSON.stringify(rest)
      : JSON.stringify(record)
  } catch {
    return null
  }
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

function extractQuestionText(rawInput: string | null): string | null {
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

function applyStreamingAction(
  conn: ConnectionState,
  action: StreamingAction
): ConnectionState | null {
  const prev = conn.liveMessage
  if (!prev || action.text.length === 0) return null

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
  }
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
        pendingQuestion: null,
        error: null,
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
        updated.error = null
      }
      next.set(action.contextKey, updated)
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
      if (!conn?.liveMessage) return state
      const prev = conn.liveMessage
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
            },
          },
        ]
      }
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: { ...prev, content: newContent },
      })
      return next
    }

    case "TOOL_CALL_UPDATE": {
      const conn = state.get(action.contextKey)
      if (!conn?.liveMessage) return state
      const prev = conn.liveMessage
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
              raw_output_total_bytes: newTotalBytes,
            },
          },
          ...prev.content.slice(existingIndex + 1),
        ]
      }

      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        liveMessage: { ...prev, content: newContent },
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
      const permissionToolInput = serializePermissionToolCall(action.tool_call)
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
          tool_call: action.tool_call,
          options: action.options,
        },
      })
      return next
    }

    case "PERMISSION_CLEARED": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
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
      if (!conn?.liveMessage) return state
      const prev = conn.liveMessage
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
      })
      return next
    }

    case "ERROR": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        error: action.message,
      })
      return next
    }

    case "AVAILABLE_COMMANDS": {
      const conn = state.get(action.contextKey)
      if (!conn) return state
      if (sameCommands(conn.availableCommands, action.commands)) return state
      const next = new Map(state)
      next.set(action.contextKey, {
        ...conn,
        availableCommands: action.commands,
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

export interface AcpActionsValue {
  connect(
    contextKey: string,
    agentType: AgentType,
    workingDir?: string,
    sessionId?: string
  ): Promise<void>
  disconnect(contextKey: string): Promise<void>
  disconnectAll(): Promise<void>
  sendPrompt(contextKey: string, blocks: PromptInputBlock[]): Promise<void>
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
  setActiveKey(key: string | null): void
  touchActivity(contextKey: string): void
  registerOpenTabKeys(keys: Set<string>): void
}

const AcpActionsContext = createContext<AcpActionsValue | null>(null)

export function useAcpActions(): AcpActionsValue {
  const ctx = useContext(AcpActionsContext)
  if (!ctx) {
    throw new Error("useAcpActions must be used within AcpConnectionsProvider")
  }
  return ctx
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
  const { pushAlert } = useAlertContext()
  const { folder } = useFolderContext()
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

  // connectionId → contextKey reverse mapping
  const reverseMapRef = useRef(new Map<string, string>())

  // Open tab keys — updated by child TabProvider via registerOpenTabKeys
  const openTabKeysRef = useRef(new Set<string>())

  // Guard against concurrent connect() calls
  const connectingKeysRef = useRef(new Set<string>())
  // Keys whose disconnect was requested while connect was still in flight
  const abandonedKeysRef = useRef(new Set<string>())

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

  // Activity tracking (no re-renders)
  const lastActivityRef = useRef(new Map<string, number>())
  const streamingQueueRef = useRef<StreamingAction[]>([])
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pendingUnmappedEventsRef = useRef(new Map<string, AcpEvent[]>())
  const listenerReadyRef = useRef(false)
  const listenerReadyWaitersRef = useRef<Array<() => void>>([])

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

      if (action.type === "REMOVE_ALL") {
        notifyAllKeyListeners()
      } else if (action.type === "STREAM_BATCH") {
        const keys = new Set(action.actions.map((item) => item.contextKey))
        for (const key of keys) {
          notifyKeyListeners(key)
        }
      } else if (action.type === "BATCH_TOOL_CALL_UPDATES") {
        const keys = new Set(action.actions.map((item) => item.contextKey))
        for (const key of keys) {
          notifyKeyListeners(key)
        }
      } else {
        const key = getAffectedKey(action)
        if (key) notifyKeyListeners(key)
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

  const bufferUnmappedEvent = useCallback((event: AcpEvent) => {
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
    (connectionId: string): AcpEvent[] => {
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
    (contextKey: string, e: AcpEvent) => {
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
          })
          scheduleToolCallUpdateFlush()
          break
        case "permission_request":
          flushStreamingQueue()
          dispatch({
            type: "PERMISSION_REQUEST",
            contextKey,
            request_id: e.request_id,
            tool_call: e.tool_call,
            fallback_title: t("toolFallbackTitle"),
            fallback_kind: "tool",
            options: e.options,
          })
          break
        case "session_started":
          flushStreamingQueue()
          dispatch({
            type: "SESSION_STARTED",
            contextKey,
            sessionId: e.session_id,
          })
          break
        case "session_modes": {
          flushStreamingQueue()
          const modeConn = storeRef.current.connections.get(contextKey)
          const resolvedModes = modeConn
            ? applySavedModePreference(modeConn.agentType, e.modes)
            : e.modes
          dispatch({
            type: "SESSION_MODES",
            contextKey,
            modes: resolvedModes,
          })
          if (modeConn) {
            const entry = selectorsCache.get(modeConn.agentType) ?? {
              modes: null,
              configOptions: null,
            }
            entry.modes = resolvedModes
            selectorsCache.set(modeConn.agentType, entry)
            // Sync cached mode to backend if it differs from server default
            if (
              resolvedModes.current_mode_id &&
              resolvedModes.current_mode_id !== e.modes.current_mode_id
            ) {
              acpSetMode(
                modeConn.connectionId,
                resolvedModes.current_mode_id
              ).catch((err: unknown) =>
                console.error(
                  "[ACP] Failed to sync saved mode to backend:",
                  err
                )
              )
            }
          }
          break
        }
        case "session_config_options": {
          flushStreamingQueue()
          const cfgConn = storeRef.current.connections.get(contextKey)
          const resolvedConfigOptions = cfgConn
            ? applySavedConfigPreferences(cfgConn.agentType, e.config_options)
            : e.config_options
          dispatch({
            type: "SESSION_CONFIG_OPTIONS",
            contextKey,
            configOptions: resolvedConfigOptions,
          })
          if (cfgConn) {
            const entry = selectorsCache.get(cfgConn.agentType) ?? {
              modes: null,
              configOptions: null,
            }
            entry.configOptions = resolvedConfigOptions
            selectorsCache.set(cfgConn.agentType, entry)
            // Sync cached config options to backend if they differ
            for (let i = 0; i < resolvedConfigOptions.length; i++) {
              const resolved = resolvedConfigOptions[i]
              const original = e.config_options[i]
              if (
                resolved.kind.type === "select" &&
                original.kind.type === "select" &&
                resolved.kind.current_value !== original.kind.current_value &&
                resolved.kind.current_value
              ) {
                acpSetConfigOption(
                  cfgConn.connectionId,
                  resolved.id,
                  resolved.kind.current_value
                ).catch((err: unknown) =>
                  console.error(
                    "[ACP] Failed to sync saved config option to backend:",
                    err
                  )
                )
              }
            }
          }
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
          // Clean up stale localStorage prefs for agents that genuinely
          // no longer provide modes or config options.
          if (rdyConn) {
            const hasModes = (rdyConn.modes?.available_modes.length ?? 0) > 0
            const hasConfig = (rdyConn.configOptions?.length ?? 0) > 0
            clearStalePrefs(rdyConn.agentType, hasModes, hasConfig)
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
    ]
  )

  // Single global event listener
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | null = null

    listenerReadyRef.current = false

    subscribe<AcpEvent>("acp://event", (payload) => {
      const contextKey = reverseMapRef.current.get(payload.connection_id)
      if (!contextKey) {
        bufferUnmappedEvent(payload)
        return
      }

      // Touch activity on every incoming event
      lastActivityRef.current.set(contextKey, Date.now())
      handleMappedEvent(contextKey, payload)
    })
      .then((fn) => {
        if (cancelled) {
          try {
            fn()
          } catch {
            // Tauri listener may not be fully registered yet
          }
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
      try {
        unlisten?.()
      } catch {
        // Tauri listener may not be fully registered yet
      }
    }
  }, [bufferUnmappedEvent, handleMappedEvent, resolveListenerReadyWaiters])

  // ── Idle sweep timer ──
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
        lastActivityRef.current.delete(contextKey)
        pendingUnmappedEventsRef.current.delete(connectionId)
        dispatch({ type: "CONNECTION_REMOVED", contextKey })
      }
    }, IDLE_SWEEP_INTERVAL_MS)

    return () => clearInterval(timer)
  }, [dispatch])

  // Disconnect all on unmount
  useEffect(() => {
    const reverseMap = reverseMapRef.current
    return () => {
      for (const [connectionId] of reverseMap) {
        acpDisconnect(connectionId).catch(() => {})
      }
    }
  }, [])

  const connect = useCallback(
    async (
      contextKey: string,
      agentType: AgentType,
      workingDir?: string,
      sessionId?: string
    ) => {
      if (connectingKeysRef.current.has(contextKey)) return
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

        const existing = storeRef.current.connections.get(contextKey)
        if (existing) {
          if (
            existing.agentType === agentType &&
            existing.status !== "disconnected" &&
            existing.status !== "error"
          ) {
            return
          }
          if (
            existing.status !== "disconnected" &&
            existing.status !== "error"
          ) {
            await acpDisconnect(existing.connectionId).catch(() => {})
            reverseMapRef.current.delete(existing.connectionId)
            lastActivityRef.current.delete(contextKey)
            pendingUnmappedEventsRef.current.delete(existing.connectionId)
          }
        }

        await waitForListenerReady()
        const connectionId = await acpConnect(agentType, workingDir, sessionId)

        // If disconnect was requested while connect was in flight,
        // tear down immediately instead of registering the connection.
        if (abandonedKeysRef.current.delete(contextKey)) {
          acpDisconnect(connectionId).catch(() => {})
          return
        }

        reverseMapRef.current.set(connectionId, contextKey)
        lastActivityRef.current.set(contextKey, Date.now())
        dispatch({
          type: "CONNECTION_CREATED",
          contextKey,
          connectionId,
          agentType,
        })

        const buffered = consumeBufferedEvents(connectionId)
        if (buffered.length > 0) {
          for (const event of buffered) {
            lastActivityRef.current.set(contextKey, Date.now())
            handleMappedEvent(contextKey, event)
          }
        }
      } catch (err) {
        if (!isAlertedError(err)) {
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
        throw err
      } finally {
        connectingKeysRef.current.delete(contextKey)
        abandonedKeysRef.current.delete(contextKey)
      }
    },
    [
      buildOpenAgentsSettingsAction,
      consumeBufferedEvents,
      dispatch,
      handleMappedEvent,
      resolveConnectBlockState,
      t,
      waitForListenerReady,
    ]
  )

  const disconnect = useCallback(
    async (contextKey: string) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) {
        // connect() is still in flight — mark as abandoned so it
        // tears down immediately when acpConnect returns.
        if (connectingKeysRef.current.has(contextKey)) {
          abandonedKeysRef.current.add(contextKey)
        }
        return
      }
      await acpDisconnect(conn.connectionId)
      reverseMapRef.current.delete(conn.connectionId)
      lastActivityRef.current.delete(contextKey)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
      dispatch({ type: "CONNECTION_REMOVED", contextKey })
    },
    [dispatch]
  )

  const disconnectAll = useCallback(async () => {
    const promises: Promise<void>[] = []
    for (const [, conn] of storeRef.current.connections) {
      promises.push(acpDisconnect(conn.connectionId).catch(() => {}))
      reverseMapRef.current.delete(conn.connectionId)
      pendingUnmappedEventsRef.current.delete(conn.connectionId)
    }
    lastActivityRef.current.clear()
    await Promise.all(promises)
    dispatch({ type: "REMOVE_ALL" })
  }, [dispatch])

  const sendPrompt = useCallback(
    async (contextKey: string, blocks: PromptInputBlock[]) => {
      const conn = storeRef.current.connections.get(contextKey)
      if (!conn) return
      lastActivityRef.current.set(contextKey, Date.now())
      await acpPrompt(conn.connectionId, blocks)
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
      // Persist user selection to localStorage
      const updatedConn = storeRef.current.connections.get(contextKey)
      const allOptions =
        updatedConn?.configOptions ??
        selectorsCache.get(conn.agentType)?.configOptions
      if (allOptions) {
        saveConfigPreference(conn.agentType, configId, valueId, allOptions)
      }
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
        dispatch({ type: "PERMISSION_CLEARED", contextKey })
      } catch (e) {
        console.error("[AcpConnections] respondPermission failed:", e)
        throw e
      }
    },
    [dispatch]
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
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
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
      setActiveKey,
      touchActivity,
      registerOpenTabKeys,
    ]
  )

  return (
    <AcpActionsContext.Provider value={actions}>
      <ConnectionStoreContext.Provider value={storeApi}>
        {children}
      </ConnectionStoreContext.Provider>
    </AcpActionsContext.Provider>
  )
}
