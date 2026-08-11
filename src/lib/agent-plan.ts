import type {
  AdaptedContentPart,
  AdaptedMessage,
} from "@/lib/adapters/ai-elements-adapter"
import {
  isPlanLikeToolName,
  normalizePriority,
  normalizeStatus,
  parseTodosFromJson,
} from "@/lib/plan-parse"
import type { PlanEntryInfo } from "@/lib/types"

/**
 * Shared empty result. Returning the same reference for the (common) no-plan
 * case keeps the caller's `useMemo`/`React.memo` dependency stable across
 * streaming batches instead of re-rendering on a fresh `[]` every time.
 * Treated as read-only by all consumers.
 */
const EMPTY_PLAN_ENTRIES: PlanEntryInfo[] = []

/**
 * Per-message memo for plan extraction. The turn adapter returns a STABLE
 * `AdaptedMessage` reference for any unchanged (non-streaming) turn, so keying
 * on the message object means a streaming batch only re-parses the one message
 * still being streamed — without it, every 16ms batch re-ran the reasoning-text
 * regex over every message in the conversation. Entries are auto-GC'd with the
 * message objects (WeakMap), so memory tracks the live conversation.
 */
const messagePlanEntriesCache = new WeakMap<AdaptedMessage, PlanEntryInfo[]>()

function parseEntriesFromReasoningText(text: string): PlanEntryInfo[] {
  const lines = text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)

  if (lines.length === 0) return []

  const entries: PlanEntryInfo[] = []

  for (const line of lines) {
    const bracketMatch = line.match(
      /^-\s*\[([^\]]+)\]\s*(.*?)(?:\s*\(([^)]+)\))?$/i
    )
    if (bracketMatch) {
      const [, rawStatus, rawContent, rawPriority] = bracketMatch
      const content = rawContent.trim()
      if (!content) continue
      entries.push({
        content,
        status: normalizeStatus(rawStatus),
        priority: normalizePriority(rawPriority),
      })
      continue
    }

    const markdownMatch = line.match(/^[-*]\s*\[(x|\s)\]\s*(.+)$/i)
    if (markdownMatch) {
      const [, done, rawContent] = markdownMatch
      const content = rawContent.trim()
      if (!content) continue
      entries.push({
        content,
        status: done.toLowerCase() === "x" ? "completed" : "pending",
        priority: "medium",
      })
    }
  }

  return entries
}

function extractPlanEntriesFromPart(part: AdaptedContentPart): PlanEntryInfo[] {
  if (part.type === "plan") {
    return part.entries
  }

  if (part.type === "tool-call") {
    if (!isPlanLikeToolName(part.toolName)) return []
    if (!part.input) return []
    return parseTodosFromJson(part.input)
  }

  if (part.type === "tool-group") {
    // Non-agent tool calls now collapse into tool-group; recurse so
    // plan-like tools (TodoWrite, plan-update, etc.) are still discovered.
    // Iterate backwards to match the "latest entry wins" caller semantics.
    for (let i = part.items.length - 1; i >= 0; i -= 1) {
      const entries = extractPlanEntriesFromPart(part.items[i])
      if (entries.length > 0) return entries
    }
    return []
  }

  if (part.type === "goal-run") {
    for (let i = part.items.length - 1; i >= 0; i -= 1) {
      const entries = extractPlanEntriesFromPart(part.items[i])
      if (entries.length > 0) return entries
    }
    return []
  }

  if (part.type === "reasoning") {
    return parseEntriesFromReasoningText(part.content)
  }

  return []
}

/** Latest plan entries within a single message (empty if none), memoized by
 *  message identity. The expensive reasoning-text regex only re-runs when the
 *  message object itself is new (i.e. the turn still streaming). */
function latestPlanEntriesInMessage(message: AdaptedMessage): PlanEntryInfo[] {
  const cached = messagePlanEntriesCache.get(message)
  if (cached !== undefined) return cached

  let result: PlanEntryInfo[] = EMPTY_PLAN_ENTRIES
  for (let j = message.content.length - 1; j >= 0; j -= 1) {
    const entries = extractPlanEntriesFromPart(message.content[j])
    if (entries.length > 0) {
      result = entries
      break
    }
  }
  messagePlanEntriesCache.set(message, result)
  return result
}

export function extractLatestPlanEntriesFromMessages(
  messages: AdaptedMessage[]
): PlanEntryInfo[] {
  let planEntries: PlanEntryInfo[] = EMPTY_PLAN_ENTRIES
  let planMessageIndex = -1

  for (let i = messages.length - 1; i >= 0 && planMessageIndex === -1; i -= 1) {
    const entries = latestPlanEntriesInMessage(messages[i])
    if (entries.length > 0) {
      planEntries = entries
      planMessageIndex = i
    }
  }

  if (planMessageIndex === -1) return EMPTY_PLAN_ENTRIES

  // A fully completed plan that belongs to an earlier exchange is stale: once
  // the user has sent another message after it, a new turn has begun, so the
  // top-right overlay should only surface the plan of the latest agent reply.
  // Consecutive assistant messages (no user message in between) still count as
  // the same reply, matching how the UI merges adjacent assistant turns.
  const allCompleted = planEntries.every(
    (entry) => entry.status === "completed"
  )
  if (allCompleted) {
    const hasUserReplyAfterPlan = messages
      .slice(planMessageIndex + 1)
      .some((message) => message.role === "user")
    if (hasUserReplyAfterPlan) return EMPTY_PLAN_ENTRIES
  }

  return planEntries
}

export function buildPlanKey(entries: PlanEntryInfo[]): string | null {
  if (entries.length === 0) return null
  return entries
    .map((entry) => `${entry.status}:${entry.priority}:${entry.content}`)
    .join("|")
}
