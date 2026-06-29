import type { PlanEntryInfo } from "@/lib/types"

/**
 * Pure plan/TodoWrite parsing helpers.
 *
 * Kept in a dependency-free module (no imports from the adapter or React
 * components) so both `agent-plan.ts` (overlay extraction) and
 * `ai-elements-adapter.ts` (turning a persisted TodoWrite tool_use into a
 * first-class `plan` part) can share them without an import cycle.
 */

export function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null
  }
  return value as Record<string, unknown>
}

export function normalizeStatus(raw: string | null | undefined): string {
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

export function normalizePriority(raw: string | null | undefined): string {
  const normalized = (raw ?? "").trim().toLowerCase()
  if (normalized === "high" || normalized === "urgent") return "high"
  if (normalized === "low") return "low"
  return "medium"
}

export function parsePlanEntriesArray(items: unknown[]): PlanEntryInfo[] {
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

export function parseTodosFromJson(input: string): PlanEntryInfo[] {
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

export function normalizeToolName(toolName: string): string {
  return toolName.toLowerCase().replace(/[^a-z0-9]/g, "")
}

export function isPlanLikeToolName(toolName: string): boolean {
  const normalized = normalizeToolName(toolName)
  if (normalized === "todowrite") return true
  // Kimi Code's todo tool is named `TodoList` (normalizes to "todolist", which
  // also covers "todo_list" since `normalizeToolName` strips separators). Like
  // `TodoWrite`, conversion to a <PlanCard> still requires non-empty parsed
  // entries at the call site, so read/clear forms stay normal tool cards.
  if (normalized === "todolist") return true
  return normalized.includes("plan")
}

/**
 * Statuses accepted by Kimi Code's `TodoListInputSchema`
 * (`status: enum(["pending","in_progress","done"])`). Used as a structural
 * identity signal for a live Kimi todo *write*.
 */
const KIMI_TODO_STATUSES = new Set(["pending", "in_progress", "done"])

/**
 * Parse the `{ todos: [...] }` payload of a Kimi Code `TodoList` *write* into
 * plan entries, or return `null` when the input is not a genuine Kimi todo
 * write.
 *
 * This is the live-path identity signal for Kimi's todo tool. Over ACP, Kimi's
 * tool call carries only a localized `title` ("Updating todo list") and a coarse
 * `kind: "other"` — the real tool name "TodoList" is never serialized — so the
 * exact input shape per Kimi's `TodoListInputSchema`
 * (`{ todos: [{ title: <non-empty>, status: "pending"|"in_progress"|"done" }] }`)
 * is the strongest available identity. It is deliberately stricter than
 * `parseTodosFromJson` (which also accepts `entries`/`plan` arrays and
 * `content`/`step`/`name` item keys) so an unrelated tool that merely carries a
 * `todos`-shaped argument cannot be mistaken for a todo write. Field trimming is
 * intentional — a whitespace-only title is rejected.
 *
 * Returns `null` for read (`{}`) / clear (`{ todos: [] }`) forms, for any
 * non-Kimi shape, and for non-object / non-JSON input.
 */
export function kimiTodoWriteEntries(
  input: string | null | undefined
): PlanEntryInfo[] | null {
  if (!input) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    return null
  }
  const obj = asRecord(parsed)
  if (!obj || !Array.isArray(obj.todos) || obj.todos.length === 0) return null

  const everyItemIsKimiTodo = obj.todos.every((item) => {
    const record = asRecord(item)
    return (
      !!record &&
      typeof record.title === "string" &&
      record.title.trim().length > 0 &&
      typeof record.status === "string" &&
      KIMI_TODO_STATUSES.has(record.status.trim().toLowerCase())
    )
  })
  if (!everyItemIsKimiTodo) return null

  // Every item was validated to carry a non-empty title, so no entry is dropped:
  // `parsePlanEntriesArray` yields exactly one entry per todo.
  return parsePlanEntriesArray(obj.todos)
}
