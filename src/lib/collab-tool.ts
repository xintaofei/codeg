/**
 * codex collab / sub-agent live tool calls (codex-acp 1.0.1, PR #223).
 *
 * codex-acp maps the codex app-server `collabAgentToolCall` ThreadItem onto a
 * live ACP `tool_call` (and `tool_call_update` on completion). Its `rawInput`
 * carries inter-agent orchestration fields that no other tool emits:
 *   { prompt, senderThreadId, receiverThreadIds, agentsStates, status }
 * where `agentsStates` is a map keyed by sub-agent threadId:
 *   { [threadId]: { status: CollabAgentStatus, message: string | null } }
 * and the ACP `title` is the collab op (`CollabAgentTool`, e.g. `spawnAgent` /
 * `wait` / `closeAgent`). We detect collab calls by that distinctive rawInput
 * shape (independent of the title) and route them to a dedicated capsule.
 *
 * The op (title) is otherwise dropped downstream (only ACP `meta` is forwarded),
 * so the live input shaper merges it back in under `COLLAB_OP_KEY` — see
 * `mergeCollabOp` and `resolveLiveToolInput`.
 *
 * Live ceiling: codex-acp 1.1.3+ (#304) additionally emits `subAgentActivity`
 * as a SEPARATE live `tool_call` (`_meta.codex.subagent`), but codeg suppresses
 * it (redundant with this collab capsule — see the Rust `is_codex_subagent_
 * activity`), and it carries no transcript content anyway (only a
 * started/interacted/interrupted lifecycle marker). #304 also adds top-level
 * `model` / `reasoningEffort` to this collab `rawInput`, surfaced here. The
 * richest live signal thus remains `agentsStates[*].{status,message}` (on a
 * `wait` completion the `message` carries the sub-agent's full result); the full
 * nested transcript only exists on history reload, reconstructed by the Rust
 * parser into the richer "Agent" capsule from the on-disk `agent-<id>.jsonl`.
 */

/** Canonical tool name the live collab path collapses to (see `inferLiveToolName`). */
export const COLLAB_AGENT_TOOL_NAME = "collab_agent"

/**
 * Synthetic rawInput key the live input shaper uses to smuggle the collab op
 * (the ACP `title`, e.g. `spawnAgent`/`wait`/`closeAgent`) through to the card,
 * since the title is otherwise dropped. Not a codex field.
 */
export const COLLAB_OP_KEY = "__codegCollabOp"

/** Live state of a single sub-agent, from one entry of `agentsStates`. */
export interface CollabAgentState {
  /** The sub-agent's codex thread id (not a human-friendly name). */
  threadId: string
  /** Raw `CollabAgentStatus` string (e.g. "pendingInit" / "running"), or null. */
  status: string | null
  /** Short live progress line; on a `wait` completion, the full result. May be null. */
  message: string | null
}

export interface CollabToolInfo {
  /** The message/task exchanged with the sub-agent, when present. */
  prompt: string | null
  /** The collab op's overall status ("inProgress" / "completed" / "failed"), or null. */
  status: string | null
  /** The collab op (`CollabAgentTool`: spawnAgent/wait/closeAgent/…), or null. */
  op: string | null
  /**
   * The sub-agent's model id (codex-acp #304), top-level in the collab
   * `rawInput` (a sibling of `prompt`/`agentsStates`, NOT per-agent). May be null.
   */
  model: string | null
  /**
   * The sub-agent's reasoning effort (codex-acp #304), an unconstrained string
   * such as "minimal" / "low" / "medium" / "high". Top-level, may be null.
   */
  reasoningEffort: string | null
  /** Per-sub-agent live states, in `agentsStates` insertion order. */
  agents: CollabAgentState[]
}

/**
 * Display kind a raw collab status maps to. Drives icon/color/label in the card.
 * Covers the full `CollabAgentStatus` enum plus the op-status values.
 */
export type CollabStatusKind =
  | "pending"
  | "running"
  | "completed"
  | "closed"
  | "interrupted"
  | "failed"
  | "notFound"
  | "other"

/** Map a codex collab status string (agent status or op status) to a display kind. */
export function classifyCollabStatus(raw: string | null): CollabStatusKind {
  switch ((raw ?? "").trim()) {
    case "pendingInit":
      return "pending"
    case "running":
    case "inProgress":
      return "running"
    case "completed":
      return "completed"
    case "shutdown":
      return "closed"
    case "interrupted":
      return "interrupted"
    case "errored":
    case "failed":
      return "failed"
    case "notFound":
      return "notFound"
    default:
      return "other"
  }
}

/** Whether a status kind should surface as an error (red) in the UI. */
export function isErrorCollabStatusKind(kind: CollabStatusKind): boolean {
  return kind === "failed" || kind === "notFound"
}

/** Display kind for the collab op (the ACP title), used for op-aware titles. */
export type CollabOpKind = "spawn" | "wait" | "close" | "resume" | "other"

/**
 * Classify the collab op spelling-agnostically. The op reaches us as the ACP
 * `title` (`CollabAgentTool`), which codex-acp emits camelCase (`spawnAgent` /
 * `wait` / `closeAgent` / `resumeAgent`), but the rollout/alias spellings are
 * snake_case (`spawn_agent` / `wait_agent` / `close_agent`). Normalize both so
 * the op-aware title survives either spelling. `sendInput`/unknown → "other"
 * (these normally carry a prompt, which the title prefers anyway).
 */
export function classifyCollabOp(op: string | null): CollabOpKind {
  const s = (op ?? "").toLowerCase().replace(/[_-]/g, "")
  if (s.includes("spawn")) return "spawn"
  if (s.includes("wait")) return "wait"
  if (s.includes("close")) return "close"
  if (s.includes("resume")) return "resume"
  return "other"
}

/**
 * Short, badge-friendly form of a sub-agent id: the first segment of the UUID
 * (everything before the first "-"), e.g. `019f07aa-f57b-…` → `019f07aa`. Falls
 * back to the whole string when there's no "-".
 */
export function shortAgentId(id: string): string {
  const dash = id.indexOf("-")
  return dash > 0 ? id.slice(0, dash) : id
}

/**
 * Aggregate ONE sub-agent's status across all collab ops that referenced it
 * (spawn → wait(s) → close), for the live execution capsule. Returns a canonical
 * raw status string that {@link classifyCollabStatus} maps correctly, chosen by
 * display-kind priority: error > completed > closed > running > pending, else the
 * last non-empty raw status, else null.
 *
 * This is what stops the execution (spawn) capsule from being frozen at the
 * spawn-time `pendingInit` ("初始化中"): once a later `wait` reports the agent
 * running/completed, the execution capsule reflects it. The full result text is
 * deliberately NOT aggregated here — it stays in the wait capsule.
 */
export function mergeCollabAgentStatus(
  statuses: (string | null | undefined)[]
): string | null {
  const kinds = statuses.map((s) => classifyCollabStatus(s ?? null))
  if (kinds.includes("failed")) return "errored"
  if (kinds.includes("notFound")) return "notFound"
  if (kinds.includes("completed")) return "completed"
  if (kinds.includes("closed")) return "shutdown"
  if (kinds.includes("running")) return "running"
  if (kinds.includes("pending")) return "pendingInit"
  for (let i = statuses.length - 1; i >= 0; i--) {
    const s = statuses[i]
    if (s && s.trim().length > 0) return s
  }
  return null
}

function tryParseObject(
  rawInput: string | null | undefined
): Record<string, unknown> | null {
  if (!rawInput) return null
  try {
    const parsed = JSON.parse(rawInput)
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return null
    }
    return parsed as Record<string, unknown>
  } catch {
    return null
  }
}

/** A non-empty (trimmed) string, or null for anything else. */
function asText(v: unknown): string | null {
  if (typeof v !== "string") return null
  const trimmed = v.trim()
  return trimmed.length > 0 ? trimmed : null
}

/**
 * True when `rawInput` is a codex collab tool call, identified by the trio of
 * inter-agent fields the codex-acp mapper always sets. Key presence (not
 * truthiness) is the signal: `receiverThreadIds` can be an empty array and
 * `prompt` can be empty for some collab phases.
 */
export function isCodexCollabInput(
  rawInput: string | null | undefined
): boolean {
  const parsed = tryParseObject(rawInput)
  if (!parsed) return false
  return (
    "senderThreadId" in parsed &&
    "receiverThreadIds" in parsed &&
    "agentsStates" in parsed
  )
}

/**
 * Merge the collab op (the ACP `title`) into a collab `rawInput` JSON string
 * under `COLLAB_OP_KEY`, so the card can render an op-aware title. Returns null
 * when there's no op or the input isn't a JSON object (caller falls back to the
 * original rawInput). Live-only; history never goes through this path.
 */
export function mergeCollabOp(
  rawInput: string | null | undefined,
  op: string | null | undefined
): string | null {
  const cleanOp = asText(op)
  if (!cleanOp) return null
  const parsed = tryParseObject(rawInput)
  if (!parsed) return null
  return JSON.stringify({ ...parsed, [COLLAB_OP_KEY]: cleanOp })
}

/** Parse the displayable fields out of a collab tool call's `rawInput`. */
export function parseCollabToolInput(
  rawInput: string | null | undefined
): CollabToolInfo | null {
  const parsed = tryParseObject(rawInput)
  if (!parsed) return null

  const agents: CollabAgentState[] = []
  const states = parsed.agentsStates
  // The authoritative shape is an object map { [threadId]: { status, message } }.
  // Anything else (missing, array, primitive) yields no rows rather than throwing.
  if (states && typeof states === "object" && !Array.isArray(states)) {
    for (const [threadId, value] of Object.entries(
      states as Record<string, unknown>
    )) {
      if (!value || typeof value !== "object" || Array.isArray(value)) continue
      const entry = value as Record<string, unknown>
      agents.push({
        threadId,
        status: asText(entry.status),
        message: asText(entry.message),
      })
    }
  }

  return {
    prompt: asText(parsed.prompt),
    status: asText(parsed.status),
    op: asText(parsed[COLLAB_OP_KEY]),
    model: asText(parsed.model),
    reasoningEffort: asText(parsed.reasoningEffort),
    agents,
  }
}
