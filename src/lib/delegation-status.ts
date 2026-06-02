/**
 * Shared parsing + status-resolution helpers for the codeg-mcp delegation
 * companion tools (`get_delegation_status` / `cancel_delegation`).
 *
 * Extracted from `delegation-status-card.tsx` so both the single card and the
 * merged group card (`delegation-status-group-card.tsx`) resolve a poll's
 * task id, report and badge through one implementation.
 *
 * Status resolution degrades gracefully; precision hinges on the HOST, not on
 * live-vs-persisted. Neither the ACP live wire nor our persisted tool-result
 * model carries `structuredContent` as a field — the helpers only ever see the
 * result TEXT plus an `is_error` flag — so:
 *   - hosts that echo the full MCP `CallToolResult` envelope (or the bare
 *     report JSON) as that text — e.g. Codex's "Wall time:…\nOutput:\n<json>" —
 *     let us recover the structured `DelegationTaskReport`, so badge, duration
 *     and result text are precise;
 *   - hosts that surface only `CallToolResult.content` text — e.g. Claude Code
 *     via claude-agent-acp — give no structured fields, so the badge is derived
 *     from the tool-call state / `is_error`, with no duration.
 */

import { extractEmbeddedJsonObject } from "@/lib/embedded-json"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

/**
 * Visual badge states.
 *
 * `checked` is the neutral, non-spinning state for a poll that RETURNED while
 * the task was still running — a stale snapshot, not live activity. The live
 * spinner (`running`) is reserved for a poll that is still in flight (no
 * result yet). This is what lets a superseded / settled "running" poll stop
 * spinning once the agent has moved past it.
 */
export type BadgeStatus =
  | "starting"
  | "running"
  | "waiting"
  | "ok"
  | "err"
  | "checked"

const TASK_STATUSES = [
  "running",
  "completed",
  "failed",
  "canceled",
  "unknown",
] as const
export type TaskStatus = (typeof TASK_STATUSES)[number]

export type StatusReport = {
  status: TaskStatus | null
  /** The report's own `task_id` — recovered when the structured report parsed.
   *  Lets a grouper fall back to it when the call input lost the id. */
  taskId: string | null
  /** Result/message text to reveal on expand (verbatim for the live-wire shape). */
  text: string | null
  /** Wire-stable error code for a failed/canceled report (badge specificity). */
  errorCode: string | null
  /** Task execution time in ms — set only for terminal cached results. */
  durationMs: number | null
}

/**
 * The verbatim message `get_delegation_status` returns for a still-running task
 * (`running_report` in `src-tauri/src/acp/delegation/broker.rs`). Hosts that
 * persist only `CallToolResult.content` text drop `structuredContent` — notably
 * a Claude Code session reloaded from its JSONL transcript, where the parser
 * keeps only `content[*].text`. On that historical path this sentence is the
 * ONLY signal that the poll returned "still running" rather than "completed",
 * so recognize it and synthesize a `running` status — otherwise the badge
 * degrades to a false `ok` ("done") for a task that is still in flight. It's a
 * backend protocol string (English-only), never localized UI copy.
 */
const RUNNING_SENTINEL = "sub-agent is still running in the background."

// EXACT (normalized) match, not a substring test: a *completed* poll's
// content-only text is the child's arbitrary result, which could legitimately
// quote this phrase. The backend emits the sentinel as the whole message, so
// only text that IS the sentinel means "still running".
function textRunningStatus(text: string | null): TaskStatus | null {
  return text != null && text.trim().toLowerCase() === RUNNING_SENTINEL
    ? "running"
    : null
}

export type ResolvedBadge = { status: BadgeStatus; errorCode?: string }

function asObject(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null
}

function str(obj: Record<string, unknown>, key: string): string | null {
  const v = obj[key]
  return typeof v === "string" && v.length > 0 ? v : null
}

function num(obj: Record<string, unknown>, key: string): number | null {
  const v = obj[key]
  return typeof v === "number" && Number.isFinite(v) ? v : null
}

function firstContentText(envelope: Record<string, unknown>): string | null {
  if (!Array.isArray(envelope.content)) return null
  const first = asObject(envelope.content[0])
  return first ? str(first, "text") : null
}

// Wrapper keys hosts use to nest the actual tool arguments (mirrors
// `delegated-sub-thread.tsx`): JSON-RPC/MCP relays pack the call as
// `{name, arguments}` or `{params: {...}}`; some agents stash args under a
// generic `input`/`payload` key. Walked recursively (small depth cap) so any
// single layer of wrapping — including double-encoded JSON strings — peels off.
const TASK_ID_WRAPPER_KEYS = [
  "arguments",
  "input",
  "params",
  "payload",
  "_meta",
] as const

function findTaskId(value: unknown, depth = 0): string | null {
  if (depth > 4 || value === null || value === undefined) return null
  // Some hosts double-encode the input (JSON-of-JSON); parse and recurse once.
  if (typeof value === "string") {
    try {
      return findTaskId(JSON.parse(value), depth + 1)
    } catch {
      return null
    }
  }
  const obj = asObject(value)
  if (!obj) return null
  const direct = str(obj, "task_id")
  if (direct) return direct
  for (const key of TASK_ID_WRAPPER_KEYS) {
    if (obj[key] === undefined) continue
    const found = findTaskId(obj[key], depth + 1)
    if (found) return found
  }
  return null
}

/** Extract the `task_id` the tool was called with (`{ task_id, wait_ms? }`),
 *  peeling host wrappers and double-encoded JSON. These tools require a
 *  non-empty `task_id`, so a miss should be rare — but degrade gracefully. */
export function parseTaskId(raw: string | null | undefined): string | null {
  if (!raw) return null
  try {
    return findTaskId(JSON.parse(raw))
  } catch {
    // unparseable input — the task ref is just a nicety, skip it
    return null
  }
}

/** The `status` value if it's one of the delegation report statuses, else null. */
function validStatus(obj: Record<string, unknown> | null): TaskStatus | null {
  if (!obj) return null
  const s = obj.status
  return typeof s === "string" &&
    (TASK_STATUSES as readonly string[]).includes(s)
    ? (s as TaskStatus)
    : null
}

/**
 * Whether `obj` is a delegation report. `structuredContent` is trusted (the
 * host only surfaces it for an actual `CallToolResult`). An UNtrusted source —
 * raw output text or `content[0].text`, which on the live wire is the child's
 * own (arbitrary) result — must ALSO carry the report's `task_id`; otherwise a
 * child whose output happens to be JSON-with-`status` would be misread as a
 * report (false failure tint / dropped output). Every real status/cancel report
 * carries `task_id`, so this never rejects a genuine one.
 */
function isReport(
  obj: Record<string, unknown> | null,
  trusted: boolean
): boolean {
  if (!validStatus(obj)) return false
  if (trusted) return true
  return typeof obj!.task_id === "string" && obj!.task_id.length > 0
}

/**
 * Parse the tool output into a delegation report. Handles every shape the
 * report can arrive in:
 *   - the MCP `CallToolResult` envelope (`{ content, structuredContent?,
 *     isError? }`) — persisted / snapshot rows;
 *   - a host-wrapped envelope/report — notably Codex's
 *     `"Wall time:…\nOutput:\n<json>"` (recovered via `extractEmbeddedJsonObject`);
 *   - an inlined report (`{ status, ... }`), incl. one embedded in
 *     `content[0].text` when the host surfaces no `structuredContent`;
 *   - the plain-text result the live stream forwards (no structured fields →
 *     status is derived from the tool-call state instead).
 * Recovering the structured `status` matters because terminal outcomes
 * (`unknown` / `failed` / `canceled`) must not degrade into a non-error row.
 */
export function parseStatusReport(
  output: string | null | undefined,
  errorText: string | null | undefined
): StatusReport {
  const empty: StatusReport = {
    status: null,
    taskId: null,
    text: null,
    errorCode: null,
    durationMs: null,
  }
  const raw = (output ?? errorText ?? "").trim()
  if (!raw) return empty

  let obj: Record<string, unknown> | null
  try {
    obj = asObject(JSON.parse(raw))
  } catch {
    obj = extractEmbeddedJsonObject(raw)
  }
  // Plain text (no recoverable JSON) — the historical content-only shape. The
  // only structured hint left is the backend's running sentinel sentence.
  if (!obj) return { ...empty, status: textRunningStatus(raw), text: raw }

  // Locate the structured report across the shapes it can hide in:
  // structuredContent (trusted) → top-level → inlined in content[0].text. The
  // last two are gated on `task_id` so a child's own JSON output isn't misread.
  const contentText = firstContentText(obj)
  const sc = asObject(obj.structuredContent)
  let report: Record<string, unknown> | null = null
  let displayText: string | null = contentText
  if (isReport(sc, true)) {
    report = sc
  } else if (isReport(obj, false)) {
    report = obj
  } else if (contentText) {
    const embedded = extractEmbeddedJsonObject(contentText)
    if (isReport(embedded, false)) {
      report = embedded
      // content[0].text WAS the report JSON, not a human message — fall back to
      // the report's own message/text for display instead of raw JSON.
      displayText = null
    }
  }

  if (report) {
    return {
      status: validStatus(report),
      taskId: str(report, "task_id"),
      text: displayText ?? str(report, "text") ?? str(report, "message"),
      errorCode: str(report, "error_code"),
      durationMs: num(report, "duration_ms"),
    }
  }

  // Parsed JSON but no report (e.g. a content envelope stripped of
  // structuredContent) — still honor the running sentinel in the display text.
  const fallbackText = contentText ?? raw
  return {
    ...empty,
    status: textRunningStatus(fallbackText),
    text: fallbackText,
  }
}

/**
 * Resolve the status badge. The structured `status` wins when present
 * (persisted rows); otherwise fall back to the tool-call lifecycle state
 * (live stream, before / without structured output).
 *
 * A RETURNED `running` poll resolves to `checked` (neutral, no spinner): it is
 * a stale snapshot of "still running at the time of that check", not live
 * activity. The live spinner is only produced by the lifecycle fallback below
 * — i.e. a poll that is genuinely still in flight (`input-*`, no result yet).
 */
export function deriveBadge(
  kind: "status" | "cancel",
  report: StatusReport,
  state: ToolCallState | undefined,
  hasError: boolean
): ResolvedBadge {
  switch (report.status) {
    case "completed":
      return { status: "ok" }
    case "running":
      // The poll RETURNED while the task was still running — a settled
      // snapshot, not live work. Show a neutral state, not an endless spinner.
      return { status: "checked" }
    case "unknown":
      // Terminal "task id not known" — surface as error, not an endless spinner.
      return { status: "err", errorCode: "unknown" }
    case "failed":
      return { status: "err", errorCode: report.errorCode ?? undefined }
    case "canceled":
      // Canceling is the *success* outcome for `cancel_delegation`; for a
      // status query a canceled task is a terminal error.
      return kind === "cancel"
        ? { status: "ok" }
        : { status: "err", errorCode: report.errorCode ?? "canceled" }
    default:
      break
  }
  if (state === "output-error" || hasError) return { status: "err" }
  if (state === "output-available") return { status: "ok" }
  if (state === "input-available" || state === "input-streaming")
    return { status: "running" }
  return { status: "starting" }
}

/** Compact human duration: `350ms`, `1.2s`, `12s`, `2m 0s`. Total seconds are
 *  rounded once before splitting so the remainder never rolls to `60s`. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)}s`
  const totalSec = Math.round(ms / 1000)
  if (totalSec < 60) return `${totalSec}s`
  return `${Math.floor(totalSec / 60)}m ${totalSec % 60}s`
}
