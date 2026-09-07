/**
 * Collaboration-session client: the TypeScript mirror of the Rust
 * `continuation` wire DTOs (schema_version = 1), with runtime validation.
 *
 * The Rust side serializes `TurnReport` / `SessionSummary` /
 * `CollaborationSnapshot` (see `src-tauri/src/commands/collaboration.rs`);
 * this module parses the SAME shapes defensively — unknown schema versions
 * are rejected, nullable fields must be present, and state strings must be
 * from the closed vocabulary. The Rust↔TS contract tests
 * (`tests/fixtures/collaboration/*.json`) pin the exact bytes both sides
 * agree on, so a backend field rename fails a test instead of silently
 * rendering `undefined` in the UI.
 */

import { getTransport } from "@/lib/transport"

// ---------------------------------------------------------------------------
// Wire vocabulary (v2 design §6)
// ---------------------------------------------------------------------------

export const CONTINUATION_SCHEMA_VERSION = 1

export const TURN_STATES = [
  "accepted",
  "preparing",
  "dispatching",
  "running",
  "cancel_requested",
  "completed",
  "failed",
  "canceled",
  "interrupted",
  "outcome_unknown",
] as const

export type TurnState = (typeof TURN_STATES)[number]

export const ACTIVE_TURN_STATES: readonly TurnState[] = [
  "accepted",
  "preparing",
  "dispatching",
  "running",
  "cancel_requested",
]

export const SESSION_STATES = ["open", "blocked", "closed"] as const
export type CollaborationSessionState = (typeof SESSION_STATES)[number]

// ---------------------------------------------------------------------------
// DTOs — field-for-field mirrors of the Rust structs
// ---------------------------------------------------------------------------

export interface SessionSummary {
  schema_version: 1
  session_id: string
  source_task_id: string
  child_conversation_id: number
  state: CollaborationSessionState
}

export interface TurnReport {
  schema_version: 1
  session_id: string
  turn_id: string
  source_task_id: string
  ordinal: number
  state: TurnState
  message: string
  initiator_kind: string
  initiator_parent_conversation_id: number
  result_text: string | null
  text_truncated: boolean
  error_code: string | null
  error_message: string | null
  blocked_on: string | null
  created_at: string
  started_at: string | null
  finished_at: string | null
  version: number
}

export interface CollaborationSnapshot {
  schema_version: 1
  session: SessionSummary | null
  turns: TurnReport[]
  next_after_ordinal: number | null
}

// ---------------------------------------------------------------------------
// Runtime parsing — every field is checked, nothing is trusted
// ---------------------------------------------------------------------------

export class ContinuationParseError extends Error {}

function assertObject(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ContinuationParseError(`${what} must be an object`)
  }
  return value as Record<string, unknown>
}

function reqString(
  obj: Record<string, unknown>,
  key: string,
  what: string
): string {
  const v = obj[key]
  if (typeof v !== "string") {
    throw new ContinuationParseError(`${what}.${key} must be a string`)
  }
  return v
}

function optString(
  obj: Record<string, unknown>,
  key: string,
  what: string
): string | null {
  if (!Object.prototype.hasOwnProperty.call(obj, key)) {
    throw new ContinuationParseError(`${what}.${key} is required`)
  }
  const v = obj[key]
  if (v === null) return null
  if (typeof v !== "string") {
    throw new ContinuationParseError(`${what}.${key} must be a string or null`)
  }
  return v
}

function reqNumber(
  obj: Record<string, unknown>,
  key: string,
  what: string
): number {
  const v = obj[key]
  if (typeof v !== "number" || !Number.isFinite(v)) {
    throw new ContinuationParseError(`${what}.${key} must be a number`)
  }
  return v
}

function reqBoolean(
  obj: Record<string, unknown>,
  key: string,
  what: string
): boolean {
  const v = obj[key]
  if (typeof v !== "boolean") {
    throw new ContinuationParseError(`${what}.${key} must be a boolean`)
  }
  return v
}

function reqTime(
  obj: Record<string, unknown>,
  key: string,
  what: string
): string {
  const v = obj[key]
  if (typeof v !== "string" || Number.isNaN(Date.parse(v))) {
    throw new ContinuationParseError(
      `${what}.${key} must be an RFC3339 timestamp`
    )
  }
  return v
}

function optTime(
  obj: Record<string, unknown>,
  key: string,
  what: string
): string | null {
  if (!Object.prototype.hasOwnProperty.call(obj, key)) {
    throw new ContinuationParseError(`${what}.${key} is required`)
  }
  const v = obj[key]
  if (v === null) return null
  if (typeof v !== "string" || Number.isNaN(Date.parse(v))) {
    throw new ContinuationParseError(
      `${what}.${key} must be an RFC3339 timestamp or null`
    )
  }
  return v
}

function reqSchemaVersion(obj: Record<string, unknown>, what: string): 1 {
  const v = obj.schema_version
  if (v !== CONTINUATION_SCHEMA_VERSION) {
    throw new ContinuationParseError(
      `${what}.schema_version must be ${CONTINUATION_SCHEMA_VERSION}, got ${String(v)}`
    )
  }
  return CONTINUATION_SCHEMA_VERSION
}

function parseTurnState(value: unknown, what: string): TurnState {
  if (typeof value !== "string" || !TURN_STATES.includes(value as TurnState)) {
    throw new ContinuationParseError(
      `${what}.state has unknown value ${String(value)}`
    )
  }
  return value as TurnState
}

function parseSessionState(
  value: unknown,
  what: string
): CollaborationSessionState {
  if (
    typeof value !== "string" ||
    !SESSION_STATES.includes(value as CollaborationSessionState)
  ) {
    throw new ContinuationParseError(
      `${what}.state has unknown value ${String(value)}`
    )
  }
  return value as CollaborationSessionState
}

/** Parse one `TurnReport` exactly as the Rust side serializes it. */
export function parseTurnReport(raw: unknown): TurnReport {
  const obj = assertObject(raw, "TurnReport")
  return {
    schema_version: reqSchemaVersion(obj, "TurnReport"),
    session_id: reqString(obj, "session_id", "TurnReport"),
    turn_id: reqString(obj, "turn_id", "TurnReport"),
    source_task_id: reqString(obj, "source_task_id", "TurnReport"),
    ordinal: reqNumber(obj, "ordinal", "TurnReport"),
    state: parseTurnState(obj.state, "TurnReport"),
    message: reqString(obj, "message", "TurnReport"),
    initiator_kind: reqString(obj, "initiator_kind", "TurnReport"),
    initiator_parent_conversation_id: reqNumber(
      obj,
      "initiator_parent_conversation_id",
      "TurnReport"
    ),
    result_text: optString(obj, "result_text", "TurnReport"),
    text_truncated: reqBoolean(obj, "text_truncated", "TurnReport"),
    error_code: optString(obj, "error_code", "TurnReport"),
    error_message: optString(obj, "error_message", "TurnReport"),
    blocked_on: optString(obj, "blocked_on", "TurnReport"),
    created_at: reqTime(obj, "created_at", "TurnReport"),
    started_at: optTime(obj, "started_at", "TurnReport"),
    finished_at: optTime(obj, "finished_at", "TurnReport"),
    version: reqNumber(obj, "version", "TurnReport"),
  }
}

/** Parse one `SessionSummary`. */
export function parseSessionSummary(raw: unknown): SessionSummary {
  const obj = assertObject(raw, "SessionSummary")
  return {
    schema_version: reqSchemaVersion(obj, "SessionSummary"),
    session_id: reqString(obj, "session_id", "SessionSummary"),
    source_task_id: reqString(obj, "source_task_id", "SessionSummary"),
    child_conversation_id: reqNumber(
      obj,
      "child_conversation_id",
      "SessionSummary"
    ),
    state: parseSessionState(obj.state, "SessionSummary"),
  }
}

/** Parse the paginated read-only snapshot the backend returns. */
export function parseCollaborationSnapshot(
  raw: unknown
): CollaborationSnapshot {
  const obj = assertObject(raw, "CollaborationSnapshot")
  reqSchemaVersion(obj, "CollaborationSnapshot")
  if (!Object.prototype.hasOwnProperty.call(obj, "session")) {
    throw new ContinuationParseError(
      "CollaborationSnapshot.session is required"
    )
  }
  const sessionRaw = obj.session
  const session = sessionRaw === null ? null : parseSessionSummary(sessionRaw)
  if (!Array.isArray(obj.turns)) {
    throw new ContinuationParseError(
      "CollaborationSnapshot.turns must be an array"
    )
  }
  const turns = obj.turns.map((t) => parseTurnReport(t))
  if (!Object.prototype.hasOwnProperty.call(obj, "next_after_ordinal")) {
    throw new ContinuationParseError(
      "CollaborationSnapshot.next_after_ordinal is required"
    )
  }
  const next = obj.next_after_ordinal
  if (next !== null && typeof next !== "number") {
    throw new ContinuationParseError(
      "CollaborationSnapshot.next_after_ordinal must be a number or null"
    )
  }
  return {
    schema_version: CONTINUATION_SCHEMA_VERSION,
    session,
    turns,
    next_after_ordinal: next,
  }
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/** Pagination defaults (v2 design §6): default 20, hard cap 100. */
export const COLLAB_SNAPSHOT_DEFAULT_LIMIT = 20
export const COLLAB_SNAPSHOT_MAX_LIMIT = 100

/**
 * Read-only snapshot of the collaboration session for a source task, scoped
 * to `parentConversationId`. `null`-looking answers (no session) come back
 * as `{ session: null, turns: [] }` — a genuine transport error REJECTS, it
 * is never masked as an empty snapshot.
 */
export async function getCollaborationSession(params: {
  parentConversationId: number
  sourceTaskId: string
  afterOrdinal?: number
  limit?: number
}): Promise<CollaborationSnapshot> {
  const raw = await getTransport().call("get_collaboration_session", {
    parentConversationId: params.parentConversationId,
    sourceTaskId: params.sourceTaskId,
    afterOrdinal: params.afterOrdinal ?? 0,
    limit: params.limit ?? COLLAB_SNAPSHOT_DEFAULT_LIMIT,
  })
  return parseCollaborationSnapshot(raw)
}

/** Read the continuable-delegation experiment flag. */
export async function getContinuationSettings(): Promise<{
  continuable_delegation_enabled: boolean
}> {
  const raw = await getTransport().call("get_continuation_settings")
  if (
    typeof raw !== "object" ||
    raw === null ||
    typeof (raw as Record<string, unknown>).continuable_delegation_enabled !==
      "boolean"
  ) {
    throw new ContinuationParseError(
      "get_continuation_settings: malformed response"
    )
  }
  return raw as { continuable_delegation_enabled: boolean }
}
