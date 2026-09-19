/**
 * Shared field parsing for a NATIVE sub-agent launch tool call (Claude
 * `Task`, codex `spawn_agent`, grok `spawn_subagent`, Cursor `task`, Open
 * Code's `call_omo_agent`).
 *
 * Both consumers read the SAME wire input and must agree: the message-area
 * Agent capsule (`agent-tool-call.tsx`) and the aux panel's sub-agents rows
 * (`native-subagent-row.tsx`). If the two parsed the launch payload
 * separately they would drift on which field names count — exactly the
 * failure this module exists to prevent.
 */

import {
  extractJsonField,
  tryParseJson,
} from "@/components/message/content-parts-renderer"
import type { AgentExecutionStats, AgentType } from "@/lib/types"

// A parsed JSON field is only usable when it's a non-empty STRING. Some
// hosts (e.g. CodeBuddy) hand us inputs where `subagent_type` / `description`
// arrive as objects (or empty `{}`); `as string` casts let those leak
// straight into rendered text, crashing React with "Objects are not valid as
// a React child". Coerce so a non-string field is treated as absent.
function asText(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null
}

/** Every displayable field the launch capsule derives from `rawInput`,
 *  parsed once. The `extractJsonField` fallbacks rescue truncated live
 *  inputs the full parse can't handle. */
export interface SubAgentLaunchFields {
  subagentType: string | null
  description: string | null
  prompt: string | null
  model: string | null
  agentId: string | null
  /** Cursor's live task payload carries `_toolName:"task"` as its identity
   *  stamp; the completion envelope folds in only for a call so marked. */
  isCursorTask: boolean
  /**
   * codex 0.147's native team-of-agents marks its capsules as LAUNCH-only
   * (`CODEX_SUBAGENT_LAUNCH_KEY`, written by both the live path and the
   * rollout parser). The card settles when codex acknowledges the spawn,
   * which is not when the child finishes — an asynchronous child can still
   * be working long after.
   */
  isCodexSubagentLaunch: boolean
  /**
   * How the codex child itself ended (`SubAgentActivity{kind}`, both paths
   * stamp `__codegCodexSubagentState`). Present only once that has been
   * heard; while it is absent the child's fate is genuinely unknown.
   */
  codexSubagentState: string | null
}

function field(
  parsed: Record<string, unknown> | null,
  input: string | null,
  ...keys: string[]
): string | null {
  for (const key of keys) {
    const direct = asText(parsed?.[key])
    if (direct) return direct
  }
  if (!input) return null
  for (const key of keys) {
    const rescued = extractJsonField(input, key)
    if (rescued) return rescued
  }
  return null
}

export function parseSubAgentLaunchFields(
  input: string | null
): SubAgentLaunchFields {
  const parsed = input ? tryParseJson(input) : null
  return {
    subagentType:
      // Codex's live `spawn_agent` labels the agent `agent_type`; Cursor's
      // live task carries `subagentType` as a protobuf-es oneof object
      // ({case: …}) its history parser emits as a plain string. Read all
      // spellings so the prefix shows during streaming too.
      asText(parsed?.subagent_type) ??
      asText(parsed?.agent_type) ??
      asText(parsed?.subagentType) ??
      asText((parsed?.subagentType as { case?: unknown } | undefined)?.case) ??
      field(parsed, input, "subagent_type", "agent_type"),
    // codex's native team-of-agents spawn carries the child's assignment in
    // `message` (no description field at all); fall back to it so the row/card
    // titles itself by the task text instead of the generic fallback.
    description:
      field(parsed, input, "description") ?? field(parsed, input, "message"),
    prompt: field(parsed, input, "prompt"),
    model: field(parsed, input, "model"),
    // codex spawn capsules carry the sub-agent's UUID (`agent_id`); the pill
    // and the codex child-session key are the same string.
    agentId: field(parsed, input, "agent_id"),
    isCursorTask: parsed?._toolName === "task",
    isCodexSubagentLaunch: parsed?.__codegCodexSubagentLaunch === true,
    codexSubagentState: asText(parsed?.__codegCodexSubagentState),
  }
}

/**
 * The child's own session, when the sub-agent ran as a standalone session on
 * disk. TWO agents do this, and for the same reason: the child is a full
 * session that streams its transcript to disk while none of it is forwarded
 * over ACP, so opening that session is the ONLY way to see the child's work.
 *
 * Grok, live, arrives as `meta.grokSubagentSession.childSessionId`
 * (`connection.rs::grok_subagent_meta`, re-sent on every progress tick because
 * meta is replaced wholesale); in history it comes off the parsed
 * `agent_stats.child_session_id` (`parsers/grok.rs::subagent_stats`).
 *
 * Codex needs neither, because its child's thread id IS its rollout's id and
 * the card already carries it as `agent_id` — the badge and the session key are
 * the same string. Both of its paths already write it
 * (`connection.rs::classify_codex_subagent_activity` live,
 * `parsers/codex.rs::inject_agent_id_into_input` on reload), alongside the
 * launch marker that identifies the producer.
 *
 * The agent type follows the PARENT when the caller knows it (the aux-panel
 * row passes the conversation's own type — a parent's child is a session of
 * the parent's kind). Without it the branch pins its own producer: grok for
 * the meta/stats handles, codex for the agent-id handle — the message-area
 * capsule has no conversation-level agent type of its own and relies on these.
 * New per-agent handles land on the same three branches.
 */
export function parseChildSessionId(
  meta: Record<string, unknown> | null | undefined,
  statsChildSessionId: string | null | undefined,
  codexSubagentId: string | null,
  parentAgentType?: AgentType | null
): { sessionId: string; agentType: AgentType } | null {
  const raw = meta?.grokSubagentSession
  if (raw && typeof raw === "object" && !Array.isArray(raw)) {
    const live = (raw as Record<string, unknown>).childSessionId
    if (typeof live === "string" && live.length > 0) {
      return { sessionId: live, agentType: parentAgentType ?? "grok" }
    }
  }
  if (statsChildSessionId && statsChildSessionId.length > 0) {
    return {
      sessionId: statsChildSessionId,
      agentType: parentAgentType ?? "grok",
    }
  }
  return codexSubagentId
    ? { sessionId: codexSubagentId, agentType: parentAgentType ?? "codex" }
    : null
}

/** Convenience over the two primitives: the child session for a launch whose
 *  parsed fields you already hold. `parentAgentType` — the owning
 *  conversation's agent — makes the handle's kind authoritative when known. */
export function childSessionOfLaunch(
  fields: Pick<SubAgentLaunchFields, "agentId" | "isCodexSubagentLaunch">,
  meta: Record<string, unknown> | null | undefined,
  agentStats: AgentExecutionStats | null | undefined,
  parentAgentType?: AgentType | null
): { sessionId: string; agentType: AgentType } | null {
  return parseChildSessionId(
    meta,
    agentStats?.child_session_id,
    fields.isCodexSubagentLaunch ? fields.agentId : null,
    parentAgentType
  )
}
