"use client"

/**
 * Inline header for a delegated child sub-session under the parent's
 * `delegate_to_agent` ToolCallBlock. Renders as a self-contained card —
 * never falls through the generic tool-call shell — so users see "Agent
 * delegating: task" instead of "mcp__codeg-delegate__delegate_to_agent: codex".
 *
 * The card is intentionally a status + navigation affordance ONLY: it does not
 * render the child's output inline and does not expand. The child's result is
 * delivered to the LLM via `get_delegation_status` and to the user by opening
 * the child session ("查看会话" → SubAgentSessionSheet, which also hosts the
 * child's permission prompts). When the child is awaiting a permission decision
 * the status badge reflects it, cueing the user to open the session.
 */

import { useCallback, useMemo, useState, useSyncExternalStore } from "react"
import { Eye } from "lucide-react"
import { useTranslations } from "next-intl"

import { AgentIcon } from "@/components/agent-icon"
import { extractEmbeddedJsonObject } from "@/lib/embedded-json"
import { useDelegatedSubSession } from "@/hooks/use-delegated-sub-session"
import { AGENT_LABELS, type AgentType } from "@/lib/types"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import { type DelegationStatus } from "@/contexts/delegation-context"
import {
  useConnectionStore,
  type ConnectionState,
} from "@/contexts/acp-connections-context"
import { StatusBadge } from "@/components/message/delegation-status-badge"
import { SubAgentSessionSheet } from "@/components/message/sub-agent-session-sheet"

interface Props {
  parentToolUseId: string
  /** Raw JSON arguments the LLM sent to `delegate_to_agent`. Used to
   *  surface the task and agent_type before the broker's
   *  DelegationStarted event lands (or when binding never arrives — e.g.
   *  the wider session was reloaded with an inline child still around). */
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
  /**
   * ACP extensibility metadata on this tool call. Read here as a
   * tertiary fallback after the live `DelegationContext` binding when
   * the parent UI re-mounted on a page refresh and the live
   * `delegation_started` event was already consumed (lost): the
   * snapshot's `ToolCallState.meta["codeg.delegation"]` carries enough
   * to re-bind the card to the child conversation.
   */
  meta?: Record<string, unknown> | null
}

type ParsedInput = {
  agentType: AgentType | null
  task: string | null
  workingDir: string | null
}

const KNOWN_AGENT_TYPES: ReadonlySet<string> = new Set<AgentType>([
  "claude_code",
  "codex",
  "open_code",
  "gemini",
  "cline",
  "open_claw",
])

/**
 * Subscribe to the child connection's `ConnectionState` (live message,
 * pending permission, etc.) from the shared connections store. Returns
 * `undefined` while no synthetic entry exists yet — caller falls back to
 * the binding / persisted-turns view. Re-renders on every state change
 * via `useSyncExternalStore`.
 */
function useDelegationChildLive(
  childConnectionId: string | null
): ConnectionState | undefined {
  const store = useConnectionStore()
  const subscribe = useCallback(
    (cb: () => void) => {
      if (!childConnectionId) return () => {}
      return store.subscribeKey(childConnectionId, cb)
    },
    [store, childConnectionId]
  )
  const getSnapshot = useCallback(
    () =>
      childConnectionId ? store.getConnection(childConnectionId) : undefined,
    [store, childConnectionId]
  )
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}

type ParsedMeta = {
  status: DelegationStatus
  childConnectionId: string | null
  childConversationId: number | null
  errorCode: string | null
}

/**
 * Extract delegation state from a `ToolCallState.meta` value. Returns
 * `null` when the meta doesn't carry the `codeg.delegation` sub-object —
 * caller falls back to the live binding / `parseInput` chain.
 *
 * The shape mirrors what the broker writes via `DelegationMetaWriter`:
 *   `{ "codeg.delegation": { status, child_connection_id?,
 *     child_conversation_id?, error_code? } }`
 */
function parseDelegationMeta(
  meta: Record<string, unknown> | null | undefined
): ParsedMeta | null {
  if (!meta || typeof meta !== "object") return null
  const inner = meta["codeg.delegation"]
  if (!inner || typeof inner !== "object" || Array.isArray(inner)) return null
  const obj = inner as Record<string, unknown>
  const rawStatus = obj["status"]
  let status: DelegationStatus
  switch (rawStatus) {
    case "running":
    case "pending":
      status = "running"
      break
    case "completed":
    case "ok":
      status = "ok"
      break
    case "failed":
    case "err":
      status = "err"
      break
    default:
      return null
  }
  const child_connection_id = obj["child_connection_id"]
  const child_conversation_id = obj["child_conversation_id"]
  const error_code = obj["error_code"]
  return {
    status,
    childConnectionId:
      typeof child_connection_id === "string" ? child_connection_id : null,
    childConversationId:
      typeof child_conversation_id === "number" ? child_conversation_id : null,
    errorCode: typeof error_code === "string" ? error_code : null,
  }
}

const EMPTY_PARSED_INPUT: ParsedInput = {
  agentType: null,
  task: null,
  workingDir: null,
}

// Wrapper keys that hosts use to nest the actual tool arguments. JSON-RPC
// servers and various MCP relays will pack the call as `{name, arguments}`
// or `{params: {...}}`; some agents stash the args under a generic
// `input`/`payload` key alongside metadata. Walked recursively (small
// depth cap) so any single layer of wrapping peels off without false
// positives on legitimate shallow fields.
const ARGS_WRAPPER_KEYS = [
  "arguments",
  "input",
  "params",
  "payload",
  "_meta",
] as const

function findDelegationArgs(
  value: unknown,
  depth = 0
): Record<string, unknown> | null {
  if (depth > 4) return null
  if (value === null || value === undefined) return null
  // Some hosts double-encode the raw input (JSON-of-JSON). Recurse once
  // on the parsed inner value before giving up.
  if (typeof value === "string") {
    try {
      return findDelegationArgs(JSON.parse(value), depth + 1)
    } catch {
      return null
    }
  }
  if (typeof value !== "object" || Array.isArray(value)) return null
  const obj = value as Record<string, unknown>
  // Direct hit: this object has at least one of the delegation fields
  // declared on its top level.
  if (
    typeof obj.task === "string" ||
    typeof obj.agent_type === "string" ||
    typeof obj.working_dir === "string"
  ) {
    return obj
  }
  for (const key of ARGS_WRAPPER_KEYS) {
    const child = obj[key]
    if (child === undefined) continue
    const found = findDelegationArgs(child, depth + 1)
    if (found) return found
  }
  return null
}

// One-line debug breadcrumb. The walker covers the wrappers we know about
// (`arguments`, `input`, `params`, `payload`, `_meta`); if a non-empty raw
// input still doesn't yield delegation args, the host is using a shape we
// haven't accounted for. Logging a truncated sample makes the next "task
// didn't show up" report self-debugging — the actual wire shape lands in
// the user's devtools instead of needing a repro.
function warnDelegationInputUnparseable(raw: string, reason: string): void {
  const sample = raw.length > 240 ? `${raw.slice(0, 240)}…` : raw
  console.warn(
    `[DelegatedSubThread] could not extract delegation args (${reason}). raw=${sample}`
  )
}

function parseInput(raw: string | null | undefined): ParsedInput {
  if (!raw || typeof raw !== "string") return EMPTY_PARSED_INPUT
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    warnDelegationInputUnparseable(raw, "JSON.parse threw")
    return EMPTY_PARSED_INPUT
  }
  const obj = findDelegationArgs(parsed)
  if (!obj) {
    warnDelegationInputUnparseable(raw, "no known wrapper matched")
    return EMPTY_PARSED_INPUT
  }
  const at = typeof obj.agent_type === "string" ? obj.agent_type : null
  return {
    agentType: at && KNOWN_AGENT_TYPES.has(at) ? (at as AgentType) : null,
    task: typeof obj.task === "string" ? obj.task : null,
    workingDir: typeof obj.working_dir === "string" ? obj.working_dir : null,
  }
}

/**
 * Parsed form of the parent `delegate_to_agent` tool output.
 *
 * Under ASYNC delegation the tool output is a *running ack* — the result
 * arrives later via the `delegation_completed` event / meta, NOT on the tool
 * output. So we must distinguish:
 *   - `ack`     — a running (or otherwise non-terminal) task: there is NO
 *                 result to render on the card yet.
 *   - `outcome` — a terminal result to render (a fast-complete ack where the
 *                 child finished during setup, or a legacy pre-async
 *                 synchronous result).
 * Returning `ack` — rather than letting the raw ack JSON fall through as an
 * "outcome" — is what stops the card from painting the ack as the result and
 * from prematurely flipping the status badge to "ok".
 */
type ParsedToolOutput =
  | { kind: "ack"; childConversationId: number | null }
  | {
      kind: "outcome"
      text: string
      isError: boolean
      childConversationId: number | null
    }

function readChildConversationId(obj: Record<string, unknown>): number | null {
  return typeof obj.child_conversation_id === "number"
    ? obj.child_conversation_id
    : null
}

/**
 * Interpret the broker's inner shape — the async `DelegationTaskReport`
 * (discriminated by `status`) or the legacy synchronous `DelegationOutcome`
 * (discriminated by `kind`). Returns null when neither discriminator is present
 * so the caller can fall through to other unwrapping strategies.
 */
function interpretReport(
  obj: Record<string, unknown>
): ParsedToolOutput | null {
  const childConversationId = readChildConversationId(obj)
  const status = typeof obj.status === "string" ? obj.status : null
  if (status) {
    switch (status) {
      case "running":
      case "unknown":
        // No terminal result to show on the card — it's an ack.
        return { kind: "ack", childConversationId }
      case "completed":
        return {
          kind: "outcome",
          text: typeof obj.text === "string" ? obj.text : "",
          isError: false,
          childConversationId,
        }
      case "failed":
      case "canceled": {
        const message = typeof obj.message === "string" ? obj.message : ""
        const code = typeof obj.error_code === "string" ? obj.error_code : ""
        return {
          kind: "outcome",
          text: message || code || "Delegation failed.",
          isError: true,
          childConversationId,
        }
      }
      default:
        return { kind: "ack", childConversationId }
    }
  }
  // Legacy synchronous outcome shape.
  const kind = typeof obj.kind === "string" ? obj.kind : null
  if (kind === "ok") {
    return {
      kind: "outcome",
      text: typeof obj.text === "string" ? obj.text : "",
      isError: false,
      childConversationId,
    }
  }
  if (kind === "err") {
    const message = typeof obj.message === "string" ? obj.message : ""
    const code = typeof obj.code === "string" ? obj.code : ""
    return {
      kind: "outcome",
      text: message || code || "Delegation failed.",
      isError: true,
      childConversationId,
    }
  }
  return null
}

/**
 * When an MCP `CallToolResult` lacks a usable `structuredContent`, the broker's
 * `DelegationTaskReport` may still be inlined in `content[0]` — either as a
 * structured `.json` object, or (Codex-style) as a JSON string in `.text`
 * (optionally wrapped, e.g. `"Wall time: N seconds\nOutput:\n<json>_"`).
 * Recognize it so a running ack yields `kind:"ack"` (not a premature "ok") and
 * its `child_conversation_id` is preserved for the "查看会话" affordance. Returns
 * null when no report can be recovered from the content array.
 */
function interpretMcpContentArray(
  obj: Record<string, unknown>
): ParsedToolOutput | null {
  if (!Array.isArray(obj.content)) return null
  const first = (obj.content as unknown[])[0]
  if (!first || typeof first !== "object" || Array.isArray(first)) return null
  const firstObj = first as Record<string, unknown>
  // Some hosts attach a structured `json` field on the content item.
  if (
    firstObj.json &&
    typeof firstObj.json === "object" &&
    !Array.isArray(firstObj.json)
  ) {
    const interpreted = interpretReport(
      firstObj.json as Record<string, unknown>
    )
    if (interpreted) return interpreted
  }
  // Codex-style: `content[0].text` is itself the serialized report.
  if (typeof firstObj.text === "string") {
    const embedded = extractEmbeddedJsonObject(firstObj.text)
    if (embedded) {
      const interpreted = interpretReport(embedded)
      if (interpreted) return interpreted
    }
  }
  return null
}

/**
 * Best-effort parse of the `delegate_to_agent` tool output into a
 * `ParsedToolOutput`. Mirrors the old unwrapping chain (direct JSON →
 * embedded-object scan → MCP `CallToolResult` envelope from
 * `companion.rs::render_task_report`) but yields the ack/outcome tagged union
 * so a running ack is never rendered as a result. `forceError` is set when
 * parsing the tool's `errorText` channel.
 */
function parseToolOutput(
  raw: string | null | undefined,
  forceError = false
): ParsedToolOutput | null {
  if (!raw || typeof raw !== "string") return null
  const trimmed = raw.trim()
  if (!trimmed) return null

  let obj: Record<string, unknown> | null = null
  try {
    const v = JSON.parse(trimmed) as unknown
    if (v && typeof v === "object" && !Array.isArray(v)) {
      obj = v as Record<string, unknown>
    } else {
      // Top-level primitive (string/number/bool): render directly.
      return {
        kind: "outcome",
        text: String(v),
        isError: forceError,
        childConversationId: null,
      }
    }
  } catch {
    obj = extractEmbeddedJsonObject(trimmed)
  }

  if (!obj) {
    return {
      kind: "outcome",
      text: trimmed,
      isError: forceError,
      childConversationId: null,
    }
  }

  // MCP `CallToolResult` envelope: `{ content: [...], structuredContent?, isError? }`.
  if (Array.isArray(obj.content)) {
    const inner =
      obj.structuredContent &&
      typeof obj.structuredContent === "object" &&
      !Array.isArray(obj.structuredContent)
        ? (obj.structuredContent as Record<string, unknown>)
        : null
    // 1. Prefer the full structured report.
    if (inner) {
      const interpreted = interpretReport(inner)
      if (interpreted) {
        // Honor an outer `isError: true` the host already decided.
        if (interpreted.kind === "outcome" && obj.isError === true) {
          return { ...interpreted, isError: true }
        }
        return interpreted
      }
    }
    // 2. No usable `structuredContent` (e.g. a host that surfaces only the
    //    content array): the report may be inlined in `content[0]`. Recognize a
    //    running ack here so it isn't mis-rendered as a terminal "ok" and its
    //    child id survives.
    const fromContent = interpretMcpContentArray(obj)
    if (fromContent) {
      if (fromContent.kind === "outcome" && obj.isError === true) {
        return { ...fromContent, isError: true }
      }
      return fromContent
    }
    // 3. Last resort: render `content[0].text` as opaque outcome text, carrying
    //    any child id from `structuredContent` if it was present but
    //    uninterpretable.
    const first = (obj.content as unknown[])[0]
    if (first && typeof first === "object" && !Array.isArray(first)) {
      const text = (first as Record<string, unknown>).text
      if (typeof text === "string") {
        return {
          kind: "outcome",
          text,
          isError: obj.isError === true || forceError,
          childConversationId: inner ? readChildConversationId(inner) : null,
        }
      }
    }
  }

  const interpreted = interpretReport(obj)
  if (interpreted) {
    if (interpreted.kind === "outcome" && forceError) {
      return { ...interpreted, isError: true }
    }
    return interpreted
  }

  // Unrecognized JSON — pretty-print so we don't surface raw braces.
  return {
    kind: "outcome",
    text: "```json\n" + JSON.stringify(obj, null, 2) + "\n```",
    isError: forceError,
    childConversationId: null,
  }
}

/**
 * Surface the broker-minted `task_id` from the `delegate_to_agent` ack so the
 * user can correlate this delegation with the later `get_delegation_status` /
 * `cancel_delegation` cards. It is carried two ways: as
 * `structuredContent.task_id` (persisted / snapshot rows) and embedded in the
 * running-ack message text as `task_id=<id>` (the live wire forwards only the
 * `CallToolResult.content` text, not `structuredContent`). Returns null when no
 * id can be recovered. The structured form is tried first; the text scan is a
 * fallback so a stray `"task_id":...` inside JSON never beats the real field.
 */
function parseDelegateTaskId(
  output: string | null | undefined,
  errorText: string | null | undefined
): string | null {
  for (const raw of [output, errorText]) {
    if (!raw || typeof raw !== "string") continue
    const trimmed = raw.trim()
    if (!trimmed) continue
    let obj: Record<string, unknown> | null = null
    try {
      const v = JSON.parse(trimmed) as unknown
      if (v && typeof v === "object" && !Array.isArray(v)) {
        obj = v as Record<string, unknown>
      }
    } catch {
      obj = extractEmbeddedJsonObject(trimmed)
    }
    if (obj) {
      const sc = obj.structuredContent
      if (sc && typeof sc === "object" && !Array.isArray(sc)) {
        const id = (sc as Record<string, unknown>).task_id
        if (typeof id === "string" && id) return id
      }
      if (typeof obj.task_id === "string" && obj.task_id) return obj.task_id
    }
    // Live wire: the ack message text embeds `task_id=<id>`.
    const m = trimmed.match(/task_id[=:]\s*"?([A-Za-z0-9][\w-]*)"?/)
    if (m) return m[1]
  }
  return null
}

export function DelegatedSubThread({
  parentToolUseId,
  input,
  output,
  errorText,
  state,
  meta,
}: Props) {
  const t = useTranslations("Folder.chat.delegation")
  const [sheetOpen, setSheetOpen] = useState(false)
  const parsed = useMemo(() => parseInput(input), [input])
  const parsedMeta = useMemo(() => parseDelegationMeta(meta), [meta])
  const taskId = useMemo(
    () => parseDelegateTaskId(output, errorText),
    [output, errorText]
  )
  // `enabled: false` — the card never fetches the child's persisted detail; it
  // only needs the live `binding` (agent type, status, child ids) from the
  // DelegationContext map. The child's output is viewed via "查看会话".
  const { binding } = useDelegatedSubSession(parentToolUseId, {
    enabled: false,
  })

  // Parse the parent `delegate_to_agent` tool output once. Under async this is
  // a running *ack* (kind:"ack") while the child runs; a terminal kind:"outcome"
  // only for a fast-complete or a legacy synchronous result. Used purely to
  // derive the status badge and the child id for synthetic-id cards (which get
  // no binding/meta) — never rendered inline.
  const toolOutput = useMemo<ParsedToolOutput | null>(() => {
    if (errorText) {
      const parsedErr = parseToolOutput(errorText, true)
      if (parsedErr) return parsedErr
    }
    return parseToolOutput(output)
  }, [output, errorText])

  // The child id drives the "查看会话" button (and the sheet). Resolution order:
  // live binding → persisted snapshot meta → the broker's ack output (the
  // synthetic-id path that emits no binding/meta).
  const childConnectionId =
    binding?.childConnectionId ?? parsedMeta?.childConnectionId ?? null
  const childConversationId =
    binding?.childConversationId ??
    parsedMeta?.childConversationId ??
    toolOutput?.childConversationId ??
    null

  // Whether the child is blocked on a permission decision. The child is pulled
  // into the store by DelegationProvider (live or snapshot-seeded), so this is
  // a status read only — the prompt itself is answered inside the sheet.
  const childLive = useDelegationChildLive(childConnectionId)
  const childAwaitingPermission = childLive?.pendingPermission != null

  const agentType: AgentType | null = binding?.agentType ?? parsed.agentType
  const status: "starting" | "running" | "waiting" | "ok" | "err" = (() => {
    // A child awaiting a permission decision is blocked until the user acts;
    // surface it over the plain running state so the card cues opening "查看会话".
    if (childAwaitingPermission) return "waiting"
    if (binding) return binding.status
    if (parsedMeta) return parsedMeta.status
    if (state === "output-error" || errorText) return "err"
    // Async: the parent output is a running ack while the child runs — keep
    // "running" rather than letting output-available flip the badge to "ok".
    if (toolOutput?.kind === "ack") return "running"
    if (toolOutput?.kind === "outcome") return toolOutput.isError ? "err" : "ok"
    if (state === "output-available") return "ok"
    // No binding, no meta, parent tool call not yet terminal: the sub-agent
    // connection is still being set up. Flips the instant a binding, meta, or
    // terminal output arrives.
    return "starting"
  })()
  const errorCode = binding?.errorCode ?? parsedMeta?.errorCode ?? undefined

  // A snapshot replay with an empty/unparseable input AND no live binding has
  // no useful card to draw — fall through to the standard renderer instead of
  // an "unknown sub-agent" stub. Placed AFTER all hooks so hook order is stable.
  if (!binding && !parsed.agentType && !parsed.task) {
    return null
  }

  return (
    <div
      data-testid="delegated-sub-thread"
      className="rounded-lg border border-border bg-card"
    >
      <div className="flex w-full items-stretch rounded-lg overflow-hidden">
        <div className="flex flex-1 min-w-0 items-center gap-3 px-3 py-2.5 text-left">
          <span className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-border bg-background text-foreground">
            {agentType ? (
              <AgentIcon agentType={agentType} className="h-5 w-5" />
            ) : (
              <span className="h-2.5 w-2.5 rounded-sm bg-muted-foreground/60" />
            )}
          </span>
          <div className="min-w-0 flex-1 space-y-0.5">
            <div className="flex items-center gap-2">
              <span className="text-sm font-semibold text-foreground">
                {agentType ? AGENT_LABELS[agentType] : t("unknownAgent")}
              </span>
              {taskId && (
                <span
                  className="shrink-0 font-mono text-xs text-muted-foreground"
                  title={taskId}
                >
                  #{taskId.slice(0, 8)}
                </span>
              )}
              <StatusBadge status={status} errorCode={errorCode} />
            </div>
            {parsed.task && (
              <div className="text-xs text-muted-foreground whitespace-pre-wrap break-words line-clamp-1">
                {parsed.task}
              </div>
            )}
          </div>
        </div>
        {childConversationId != null && (
          <button
            type="button"
            onClick={() => setSheetOpen(true)}
            className="shrink-0 flex items-center gap-1.5 px-3 border-l border-border text-xs font-medium text-foreground/80 hover:bg-muted/60 hover:text-foreground transition-colors"
            title={t("openDetail")}
            aria-label={t("openDetail")}
          >
            <Eye className="h-3.5 w-3.5" />
            <span className="hidden sm:inline">{t("openDetail")}</span>
          </button>
        )}
      </div>
      {childConversationId != null && (
        <SubAgentSessionSheet
          open={sheetOpen}
          onOpenChange={setSheetOpen}
          childConversationId={childConversationId}
          childConnectionId={childConnectionId}
          agentType={agentType}
        />
      )}
    </div>
  )
}
