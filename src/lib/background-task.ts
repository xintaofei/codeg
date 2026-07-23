/**
 * Shared parsing + detection helpers for Claude Code's built-in background-task
 * tools (`Bash(run_in_background)` launch → `TaskOutput` polls → `TaskStop`).
 *
 * Claude Code starts a background shell with `Bash(run_in_background: true)`,
 * whose result is a launch line carrying the task id ("Command running in
 * background with ID: <id>. …"). The agent then polls with
 * `TaskOutput({task_id, block, timeout})`, whose result is an XML-tagged
 * envelope, and may finally `TaskStop({task_id})`, whose result is a JSON
 * object carrying the original command.
 *
 *   poll  → <retrieval_status>success|timeout|not_ready</retrieval_status>
 *           <task_id>…</task_id> <task_type>local_bash</task_type>
 *           <status>running|completed</status> <exit_code>0</exit_code>
 *           <output>…(ANSI shell output)…</output>
 *   stop  → {"message":"Successfully stopped task: … (<command>)",
 *           "task_id":…, "task_type":…, "command":…}
 *
 * The same task is polled repeatedly (first timeout/running, then
 * success/completed), so the renderer collapses consecutive polls of one task
 * id into a single lifecycle card — mirroring the delegation-status group
 * (`@/lib/delegation-status`). Codeg owns only the rendering: the backend
 * (`parsers/claude.rs`) passes the tool-result text through verbatim, so all
 * parsing lives here (same convention as `delegation-status.ts`).
 */

import { isUnsettledToolCall } from "@/lib/tool-call-lifecycle"
import type { AdaptedToolCallPart } from "@/lib/adapters/ai-elements-adapter"

export interface BackgroundTaskEnvelope {
  /** `poll` = a `TaskOutput` retrieval; `stop` = a `TaskStop` acknowledgement. */
  kind: "poll" | "stop"
  /** Whether THIS poll fetched fresh data: success | timeout | not_ready. */
  retrievalStatus: string | null
  taskId: string | null
  /** e.g. `local_bash`, `local_workflow`. */
  taskType: string | null
  /** Task run state: running | completed (poll); `stopped` synthesized for stop. */
  status: string | null
  exitCode: number | null
  /** Shell output (may contain ANSI). Poll only. */
  output: string | null
  /** The original command. Stop only. */
  command: string | null
  /** The stop acknowledgement message. Stop only. */
  message: string | null
}

/** Read a single `<name>…</name>` tag (non-greedy, first match). */
function xmlTag(text: string, name: string): string | null {
  const match = text.match(new RegExp(`<${name}>([\\s\\S]*?)</${name}>`, "i"))
  return match ? match[1].trim() : null
}

/** Read `<output>…</output>`, greedy to the LAST close so shell output that
 *  itself contains `<…>` fragments isn't truncated. Outer wrapping newlines
 *  (the envelope's own formatting) are stripped; inner content is preserved. */
function xmlOutputTag(text: string): string | null {
  const match = text.match(/<output>([\s\S]*)<\/output>/i)
  if (!match) return null
  return match[1].replace(/^\r?\n/, "").replace(/\r?\n[ \t]*$/, "")
}

function parsePollEnvelope(text: string): BackgroundTaskEnvelope | null {
  const retrievalStatus = xmlTag(text, "retrieval_status")
  const taskType = xmlTag(text, "task_type")
  const taskId = xmlTag(text, "task_id")
  const status = xmlTag(text, "status")
  // Require at least one of the envelope-defining tags so arbitrary text that
  // merely contains a stray `<task_id>` isn't misread as a poll.
  if (
    retrievalStatus == null &&
    taskType == null &&
    !(taskId != null && status != null)
  ) {
    return null
  }
  const exitRaw = xmlTag(text, "exit_code")
  const exitCode =
    exitRaw != null && /^-?\d+$/.test(exitRaw) ? parseInt(exitRaw, 10) : null
  return {
    kind: "poll",
    retrievalStatus,
    taskId,
    taskType,
    status,
    exitCode,
    output: xmlOutputTag(text),
    command: null,
    message: null,
  }
}

function parseStopEnvelope(text: string): BackgroundTaskEnvelope | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
    return null
  const obj = parsed as Record<string, unknown>
  const message = typeof obj.message === "string" ? obj.message : null
  const command = typeof obj.command === "string" ? obj.command : null
  const taskId = typeof obj.task_id === "string" ? obj.task_id : null
  const taskType = typeof obj.task_type === "string" ? obj.task_type : null
  // Strict: must be a stop acknowledgement, so other JSON tool results aren't
  // hijacked. The phrase is the primary signal; the id+command+message shape is
  // a defensive fallback for wording drift.
  const looksLikeStop =
    (message != null && /successfully stopped task/i.test(message)) ||
    (taskId != null && command != null && message != null && taskType != null)
  if (!looksLikeStop) return null
  return {
    kind: "stop",
    retrievalStatus: null,
    taskId,
    taskType,
    status: "stopped",
    exitCode: null,
    output: null,
    command,
    message,
  }
}

/** Parse a background-task tool result into its structured envelope, or `null`
 *  when the text is neither a `TaskOutput` poll nor a `TaskStop` ack (callers
 *  fall back to generic rendering). */
export function parseBackgroundTaskEnvelope(
  text: string | null | undefined
): BackgroundTaskEnvelope | null {
  const raw = text?.trim()
  if (!raw) return null
  return parsePollEnvelope(raw) ?? parseStopEnvelope(raw)
}

const LAUNCH_RE = /Command running in background with ID:\s*([A-Za-z0-9_-]+)/i

/** Recognize the `Bash(run_in_background: true)` launch result and pull the task
 *  id. Lets the command card flag itself as a background launch. */
export function parseBackgroundLaunch(
  text: string | null | undefined
): { taskId: string } | null {
  if (!text) return null
  const match = text.match(LAUNCH_RE)
  return match ? { taskId: match[1] } : null
}

const BACKGROUND_TASK_NAMES: ReadonlySet<string> = new Set([
  "taskoutput",
  "taskstop",
  "task_output",
  "task_stop",
])

function parseInputObject(
  input: string | null | undefined
): Record<string, unknown> | null {
  if (!input) return null
  try {
    const obj = JSON.parse(input)
    return obj && typeof obj === "object" && !Array.isArray(obj)
      ? (obj as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

/** A `TaskOutput` poll's input shape: `{task_id, block?, timeout?}`. The
 *  `block`/`timeout` requirement is what distinguishes it from `cancel_delegation`
 *  / `TaskStop` (which carry a bare `{task_id}`); `subagent_type` is excluded so
 *  real sub-agent `Agent`/`Task` calls never match. Catches the live in-flight
 *  poll before its output (and thus its envelope) has arrived. */
function inputIsBackgroundPoll(input: string | null | undefined): boolean {
  const obj = parseInputObject(input)
  if (!obj) return false
  if (typeof obj.task_id !== "string" || obj.task_id.length === 0) return false
  if ("subagent_type" in obj) return false
  return "block" in obj || "timeout" in obj
}

/**
 * Whether a tool-call part is a Claude Code background-task poll/stop. True when
 * ANY holds: the raw tool name is `TaskOutput`/`TaskStop` (historical path); the
 * output parses as a background-task envelope (covers the live `task` alias and
 * any naming); or the input is a `TaskOutput` poll shape (covers the live
 * in-flight poll). Deliberately does NOT match real sub-agent `Agent` calls
 * (`subagent_type`), `get_delegation_status` (`task_ids` + JSON), or
 * `cancel_delegation` (bare `{task_id}`).
 */
export function isBackgroundTaskToolCall(part: AdaptedToolCallPart): boolean {
  if (BACKGROUND_TASK_NAMES.has(part.toolName.trim().toLowerCase())) return true
  if (
    parseBackgroundTaskEnvelope(part.output ?? part.errorText ?? null) !== null
  ) {
    return true
  }
  return inputIsBackgroundPoll(part.input)
}

export type BackgroundTaskBadge = "running" | "completed" | "failed" | "stopped"

/** One resolved task row for the card: the latest poll's outcome plus the
 *  freshest output/command gathered across every poll of that task id. */
export interface BackgroundTaskRow {
  key: string
  taskId: string | null
  taskType: string | null
  badge: BackgroundTaskBadge
  /** `true` when the latest poll is still in flight (no result yet) → spinner. */
  isInFlight: boolean
  exitCode: number | null
  output: string | null
  command: string | null
  /** Number of polls collapsed into this row (the `×N` hint). */
  pollCount: number
}

function inputTaskId(input: string | null | undefined): string | null {
  const obj = parseInputObject(input)
  return obj && typeof obj.task_id === "string" && obj.task_id.length > 0
    ? obj.task_id
    : null
}

function isInFlightState(part: AdaptedToolCallPart): boolean {
  return part.state === "input-available" || part.state === "input-streaming"
}

function deriveBackgroundBadge(
  envelope: BackgroundTaskEnvelope | null,
  part: AdaptedToolCallPart
): BackgroundTaskBadge {
  if (envelope?.kind === "stop") return "stopped"
  if (envelope?.status === "completed") {
    return envelope.exitCode != null && envelope.exitCode !== 0
      ? "failed"
      : "completed"
  }
  // An errored poll with no clean "completed" envelope is a failure.
  if (
    part.state === "output-error" ||
    (part.errorText && part.errorText.trim() !== "")
  ) {
    return "failed"
  }
  // running / timeout / not_ready / no envelope yet → still running.
  return "running"
}

interface ParsedBackgroundPoll {
  poll: AdaptedToolCallPart
  envelope: BackgroundTaskEnvelope | null
}

/**
 * Group a run of background-task polls into one row per task id. Mirrors
 * `buildDelegationTaskRows`: a poll is attributed to a task by its envelope's
 * `task_id` (falling back to the call input's `task_id`), so a task polled N
 * times shows one row with its latest outcome, and parallel tasks (interleaved
 * polls) surface as one row each. First-appearance order is preserved.
 */
export function buildBackgroundTaskRows(
  polls: AdaptedToolCallPart[]
): BackgroundTaskRow[] {
  const order: string[] = []
  const byKey = new Map<
    string,
    { taskId: string | null; entries: ParsedBackgroundPoll[] }
  >()
  for (const poll of polls) {
    const envelope = parseBackgroundTaskEnvelope(
      poll.output ?? poll.errorText ?? null
    )
    const taskId = envelope?.taskId ?? inputTaskId(poll.input) ?? null
    // Drop an unsettled poll that carries no identity AND no output yet — a live
    // `TaskOutput` whose `task_id` hasn't streamed onto the wire. claude-agent-acp
    // emits an arg-less initial `tool_call` (rawInput `{}`) and fills the real
    // args only on a later update, so a still-blocking poll parses to no envelope
    // and no input id. Each such re-poll would otherwise render as its own
    // anonymous "background task running" row, stacking identical duplicates of
    // the same wait; it folds into its task's row once the id resolves (and the
    // settled transcript, whose polls always carry the id, is unaffected).
    // `isUnsettledToolCall` (not a bare live-state check) also covers an orphan
    // promoted into `localTurns` at COMPLETE_TURN — output-available yet never
    // settled — which would otherwise re-stack after the turn completes.
    // Mirrors `buildDelegationTaskRows`.
    if (taskId == null && envelope == null && isUnsettledToolCall(poll)) {
      continue
    }
    const key = taskId ?? `__bg__:${poll.toolCallId}`
    let entry = byKey.get(key)
    if (!entry) {
      entry = { taskId, entries: [] }
      byKey.set(key, entry)
      order.push(key)
    }
    entry.entries.push({ poll, envelope })
  }
  return order.map((key) => {
    const entry = byKey.get(key)!
    const latest = entry.entries[entry.entries.length - 1]
    const env = latest.envelope
    // The terminal poll carries the full output; fall back to the last poll that
    // captured any so an in-flight final poll doesn't blank a completed run.
    let output: string | null = null
    let command: string | null = null
    let taskType: string | null = null
    for (const { envelope } of entry.entries) {
      if (envelope?.output != null && envelope.output.trim() !== "") {
        output = envelope.output
      }
      if (envelope?.command) command = envelope.command
      if (envelope?.taskType) taskType = envelope.taskType
    }
    return {
      key,
      taskId: entry.taskId,
      taskType,
      badge: deriveBackgroundBadge(env, latest.poll),
      isInFlight:
        isInFlightState(latest.poll) && (env == null || env.output == null),
      exitCode: env?.exitCode ?? null,
      output,
      command,
      pollCount: entry.entries.length,
    }
  })
}
