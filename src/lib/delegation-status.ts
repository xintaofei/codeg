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
import type {
  AdaptedToolCallPart,
  ToolCallState,
} from "@/lib/adapters/ai-elements-adapter"

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
 * The verbatim message(s) `get_delegation_status` returns for a still-running
 * task (`running_report` / `attach_live_reply` in
 * `src-tauri/src/acp/delegation/broker.rs`). Hosts that persist only
 * `CallToolResult.content` text drop `structuredContent` — notably Claude Code,
 * which keeps only `content[*].text`. On that path this text is the ONLY signal
 * that the poll returned "still running" rather than "completed", so recognize
 * it and synthesize a `running` status — otherwise the badge degrades to a
 * false `ok` ("done") for a task still in flight. These are backend protocol
 * strings (English-only), never localized UI copy.
 *
 * The running marker is the STANDALONE first line `"Running."`. The optional
 * live hint follows on its OWN line — `"Running.\nLatest sub-agent reply: <…>"`
 * — and is child-controlled text we deliberately never match against. Anchoring
 * to the full first line (plus, for the hint variant, the fixed second-line
 * prefix) instead of prefix-matching arbitrary output is what keeps a
 * *completed* result whose text merely starts with "Running. …" from being
 * misread as still-running. The legacy long sentinel is still accepted so
 * already-persisted historical rows resolve correctly.
 */
const RUNNING_MARKER = "running."
const RUNNING_REPLY_LINE_PREFIX = "latest sub-agent reply:"
const LEGACY_RUNNING_SENTINEL = "sub-agent is still running in the background."

function textRunningStatus(text: string | null): TaskStatus | null {
  if (text == null) return null
  const normalized = text.trim().toLowerCase()
  if (normalized === LEGACY_RUNNING_SENTINEL) return "running"
  // Anchor on the first line being EXACTLY the bare marker. A bare "Running."
  // (no second line) is running; the hint variant additionally requires the
  // second line to start with the fixed protocol prefix — the child's reply
  // text after it is never inspected.
  const newlineIdx = normalized.indexOf("\n")
  const firstLine = (
    newlineIdx === -1 ? normalized : normalized.slice(0, newlineIdx)
  ).trim()
  if (firstLine !== RUNNING_MARKER) return null
  if (newlineIdx === -1) return "running"
  const secondLine = normalized.slice(newlineIdx + 1).trimStart()
  return secondLine.startsWith(RUNNING_REPLY_LINE_PREFIX) ? "running" : null
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

/**
 * Parse a tool-result string into its envelope/report object, peeling one layer
 * of double-encoding (JSON-of-JSON). Some hosts re-stringify the RESULT text
 * exactly as they do tool arguments (see `findTaskId`): a direct parse then
 * yields a *string* rather than the payload object. Re-parse that once — falling
 * back to the embedded-JSON scanner so a re-stringified Codex "Wall time:…
 * Output:<json>" wrap still resolves. A parse that yields a non-string non-object
 * (array, number) is null, matching the prior inline behavior. Shared by
 * [`parseStatusReport`] and [`parseStatusReports`] so both honor the same shapes.
 */
function parseResultObject(raw: string): Record<string, unknown> | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return extractEmbeddedJsonObject(raw)
  }
  if (typeof parsed === "string") {
    try {
      return asObject(JSON.parse(parsed)) ?? extractEmbeddedJsonObject(parsed)
    } catch {
      return extractEmbeddedJsonObject(parsed)
    }
  }
  return asObject(parsed)
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

/** Array-aware sibling of [`findTaskId`]: collect a status poll's task ids from
 *  a `task_ids` array and/or a legacy single `task_id`, peeling host wrappers
 *  and double-encoded JSON. Order-preserving and de-duplicated. */
function findTaskIds(value: unknown, depth = 0): string[] {
  if (depth > 4 || value === null || value === undefined) return []
  // Double-encoded input (JSON-of-JSON): parse and recurse once.
  if (typeof value === "string") {
    try {
      return findTaskIds(JSON.parse(value), depth + 1)
    } catch {
      return []
    }
  }
  const obj = asObject(value)
  if (!obj) return []
  const out: string[] = []
  const seen = new Set<string>()
  const push = (v: unknown) => {
    if (typeof v === "string" && v.length > 0 && !seen.has(v)) {
      seen.add(v)
      out.push(v)
    }
  }
  // The batch `task_ids` array first, then a lone legacy `task_id`.
  if (Array.isArray(obj.task_ids)) for (const v of obj.task_ids) push(v)
  push(obj.task_id)
  if (out.length > 0) return out
  // Nothing at this level — peel one wrapper layer.
  for (const key of TASK_ID_WRAPPER_KEYS) {
    if (obj[key] === undefined) continue
    const found = findTaskIds(obj[key], depth + 1)
    if (found.length > 0) return found
  }
  return out
}

/** Extract the task id(s) a status poll was called with — `{ task_ids: [...] }`
 *  (current shape) or a legacy single `{ task_id }` from historical transcripts
 *  — peeling host wrappers and double-encoded JSON. Returns `[]` on a miss; the
 *  report's own `task_id` is the primary ref, these are only an index-aligned
 *  fallback. */
export function parseTaskIds(raw: string | null | undefined): string[] {
  if (!raw) return []
  try {
    return findTaskIds(JSON.parse(raw))
  } catch {
    return []
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

  const obj = parseResultObject(raw)
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

/** Build a `StatusReport` from a known delegation report object — one element
 *  of a `tasks` batch. The element IS the report, so display text comes from
 *  its own `text` / `message` (there's no separate content envelope to show). */
function reportFromObject(report: Record<string, unknown>): StatusReport {
  return {
    status: validStatus(report),
    taskId: str(report, "task_id"),
    text: str(report, "text") ?? str(report, "message"),
    errorCode: str(report, "error_code"),
    durationMs: num(report, "duration_ms"),
  }
}

/**
 * Locate a batch `tasks` array across the shapes a `get_delegation_status`
 * batch result can arrive in: trusted `structuredContent.tasks`; a top-level
 * `tasks`; or a `{ "tasks": [...] }` JSON embedded in `content[0].text` (the
 * content-only host path, e.g. Claude Code). `trusted` mirrors
 * `parseStatusReport`: only the `structuredContent` source is trusted outright;
 * the others are still gated per-element on carrying a real report.
 */
function findTasksArray(
  obj: Record<string, unknown>
): { tasks: unknown[]; trusted: boolean } | null {
  const sc = asObject(obj.structuredContent)
  if (sc && Array.isArray(sc.tasks)) return { tasks: sc.tasks, trusted: true }
  if (Array.isArray(obj.tasks)) return { tasks: obj.tasks, trusted: false }
  const contentText = firstContentText(obj)
  if (contentText) {
    const embedded = extractEmbeddedJsonObject(contentText)
    if (embedded && Array.isArray(embedded.tasks))
      return { tasks: embedded.tasks, trusted: false }
  }
  return null
}

/**
 * Parse a `get_delegation_status` tool output into ONE report per task.
 *
 * A batch poll returns a `{ "tasks": [report, ...] }` envelope (carried in
 * `structuredContent`, at the top level, or — on hosts that persist only
 * `CallToolResult.content` text — as the JSON content text itself). When such
 * an array is found AND at least one element looks like a real report, every
 * element is mapped to a `StatusReport`, preserving order so a grouper can
 * index-align them with the call's `task_ids`. Otherwise this falls back to the
 * single-report [`parseStatusReport`], leaving the historical single-task /
 * cancel shapes unchanged.
 */
export function parseStatusReports(
  output: string | null | undefined,
  errorText: string | null | undefined
): StatusReport[] {
  const raw = (output ?? errorText ?? "").trim()
  if (raw) {
    const obj = parseResultObject(raw)
    if (obj) {
      const found = findTasksArray(obj)
      if (
        found &&
        found.tasks.length > 0 &&
        found.tasks.some((t) => isReport(asObject(t), found.trusted))
      ) {
        return found.tasks.map((t) => reportFromObject(asObject(t) ?? {}))
      }
    }
  }
  // Not a batch envelope → single report (one task / cancel, unchanged).
  return [parseStatusReport(output, errorText)]
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

/** One resolved task row for the status card(s): the task ref, its latest
 *  poll's report + badge, and one result entry per poll that touched the task
 *  (the `×N` hint + the expanded pager). */
export interface DelegationTaskRow {
  key: string
  taskId: string | null
  report: StatusReport
  badge: ResolvedBadge
  /** One entry per poll of this task, chronological (`null` where a poll
   *  returned no text) — paginated on expand; length is the `×N` header hint. */
  results: (string | null)[]
}

interface ParsedPollReport {
  poll: AdaptedToolCallPart
  report: StatusReport
}

const EMPTY_STATUS_REPORT: StatusReport = {
  status: null,
  taskId: null,
  text: null,
  errorCode: null,
  durationMs: null,
}

/**
 * Group a run of `get_delegation_status` polls into one row per task.
 *
 * Each poll resolves to ONE report (a single-task poll) or MANY (a batch poll
 * issued with `task_ids`). A report is attributed to a task by its own
 * `task_id`, falling back to the poll's input ids by index, then grouped across
 * all polls — so a task polled N times shows `×N` with its latest outcome, and
 * parallel waits surface as one row each. A report we still can't attribute is
 * keyed by `toolCallId:index` so distinct unknowns (incl. two in one batch
 * poll) never collapse into one row. First-appearance order is preserved.
 */
export function buildDelegationTaskRows(
  polls: AdaptedToolCallPart[]
): DelegationTaskRow[] {
  const order: string[] = []
  const byKey = new Map<
    string,
    { taskId: string | null; polls: ParsedPollReport[] }
  >()
  for (const poll of polls) {
    const reports = parseStatusReports(poll.output, poll.errorText)
    const inputIds = parseTaskIds(poll.input)
    // A poll declares its task set via its input ids too. An in-flight BATCH
    // poll (`task_ids:[a,b]`, no output yet) parses to a single empty fallback
    // report, so iterate the wider of reports/inputIds — every requested task
    // gets a (spinner) row immediately instead of all-but-the-first vanishing
    // until the batch resolves.
    const count = Math.max(reports.length, inputIds.length)
    for (let i = 0; i < count; i++) {
      const report = reports[i] ?? EMPTY_STATUS_REPORT
      const taskId = report.taskId ?? inputIds[i] ?? null
      const key = taskId ?? `__unattributed__:${poll.toolCallId}:${i}`
      let entry = byKey.get(key)
      if (!entry) {
        entry = { taskId, polls: [] }
        byKey.set(key, entry)
        order.push(key)
      }
      entry.polls.push({ poll, report })
    }
  }
  return order.map((key) => {
    const entry = byKey.get(key)!
    const latest = entry.polls[entry.polls.length - 1]
    const badge = deriveBadge(
      "status",
      latest.report,
      latest.poll.state,
      !!latest.poll.errorText
    )
    // One entry per poll, in order — so `×N` and the pager total both equal the
    // number of times this task was actually checked.
    const results = entry.polls.map(({ report }) => report.text)
    return { key, taskId: entry.taskId, report: latest.report, badge, results }
  })
}
