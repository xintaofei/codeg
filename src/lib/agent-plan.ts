import type {
  AdaptedContentPart,
  AdaptedMessage,
} from "@/lib/adapters/ai-elements-adapter"
import type { PlanEntryInfo } from "@/lib/types"

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null
  }
  return value as Record<string, unknown>
}

function normalizeStatus(raw: string | null | undefined): string {
  const normalized = (raw ?? "").trim().toLowerCase()
  if (normalized === "completed" || normalized === "done") return "completed"
  if (
    normalized === "in_progress" ||
    normalized === "in-progress" ||
    normalized === "in progress" ||
    normalized === "running" ||
    normalized === "active"
  ) {
    return "in_progress"
  }
  return "pending"
}

function normalizePriority(raw: string | null | undefined): string {
  const normalized = (raw ?? "").trim().toLowerCase()
  if (normalized === "high" || normalized === "urgent") return "high"
  if (normalized === "low") return "low"
  return "medium"
}

function parseTodosFromJson(input: string): PlanEntryInfo[] {
  try {
    const parsed: unknown = JSON.parse(input)
    const obj = asRecord(parsed)
    if (!obj) return []

    const candidateLists: unknown[][] = []
    if (Array.isArray(obj.todos)) {
      candidateLists.push(obj.todos)
    }
    if (Array.isArray(obj.entries)) {
      candidateLists.push(obj.entries)
    }
    if (Array.isArray(obj.plan)) {
      candidateLists.push(obj.plan)
    }

    for (const list of candidateLists) {
      const parsedEntries = parsePlanEntriesArray(list)
      if (parsedEntries.length > 0) {
        return parsedEntries
      }
    }

    return []
  } catch {
    return []
  }
}

function parsePlanEntriesArray(items: unknown[]): PlanEntryInfo[] {
  const entries: PlanEntryInfo[] = []

  for (const item of items) {
    const record = asRecord(item)
    if (!record) continue

    const contentCandidate =
      typeof record.content === "string"
        ? record.content
        : typeof record.step === "string"
          ? record.step
          : typeof record.title === "string"
            ? record.title
            : typeof record.name === "string"
              ? record.name
              : ""
    const content = contentCandidate.trim()
    if (!content) continue

    entries.push({
      content,
      status: normalizeStatus(
        typeof record.status === "string" ? record.status : undefined
      ),
      priority: normalizePriority(
        typeof record.priority === "string" ? record.priority : undefined
      ),
    })
  }

  return entries
}

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

function normalizeToolName(toolName: string): string {
  return toolName.toLowerCase().replace(/[^a-z0-9]/g, "")
}

function isPlanLikeToolName(toolName: string): boolean {
  const normalized = normalizeToolName(toolName)
  if (normalized === "todowrite") return true
  return normalized.includes("plan")
}

function extractPlanEntriesFromPart(part: AdaptedContentPart): PlanEntryInfo[] {
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

export function extractLatestPlanEntriesFromMessages(
  messages: AdaptedMessage[]
): PlanEntryInfo[] {
  let planEntries: PlanEntryInfo[] = []
  let planMessageIndex = -1

  for (let i = messages.length - 1; i >= 0 && planMessageIndex === -1; i -= 1) {
    const message = messages[i]
    for (let j = message.content.length - 1; j >= 0; j -= 1) {
      const entries = extractPlanEntriesFromPart(message.content[j])
      if (entries.length > 0) {
        planEntries = entries
        planMessageIndex = i
        break
      }
    }
  }

  if (planMessageIndex === -1) return []

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
    if (hasUserReplyAfterPlan) return []
  }

  return planEntries
}

export function buildPlanKey(entries: PlanEntryInfo[]): string | null {
  if (entries.length === 0) return null
  return entries
    .map((entry) => `${entry.status}:${entry.priority}:${entry.content}`)
    .join("|")
}
