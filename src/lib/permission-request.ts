import { normalizeToolName } from "@/lib/tool-call-normalization"
import {
  countUnifiedDiffLineChanges,
  estimateChangedLineStats,
  splitNormalizedLines,
} from "@/lib/line-change-stats"

type ObjectLike = Record<string, unknown>

export interface PermissionFileChange {
  path: string
  oldText: string
  newText: string
  unifiedDiff?: string
  startLine?: number
}

export interface PermissionPlanEntry {
  text: string
  status: string | null
}

export interface PermissionAllowedPrompt {
  prompt: string
  tool: string
}

export interface ParsedPermissionToolCall {
  title: string
  /**
   * Human-readable description from `_meta.claudeCode.title`
   * (claude-agent-acp ≥0.63; for Bash it is the model-authored `description`
   * input). The permission path carries the RAW serde spelling `_meta` —
   * unlike tool parts, whose `AcpEvent` field is named `meta`. Null when the
   * agent supplied none; the dialog then falls back to `title`.
   */
  description: string | null
  /**
   * The agent's stated REASON for asking, from `_meta.permission.description`
   * (codex-acp ≥1.7.0, hoisted onto the tool call by the backend). Distinct
   * from {@link description}, which names the ACTION; this says why it needs
   * approval. Null for every agent that sends no such block.
   */
  reason: string | null
  normalizedKind: string
  command: string | null
  cwd: string | null
  fileChanges: PermissionFileChange[]
  additions: number
  deletions: number
  diffPreview: string | null
  planEntries: PermissionPlanEntry[]
  planExplanation: string | null
  planMarkdown: string | null
  allowedPrompts: PermissionAllowedPrompt[]
  modeTarget: string | null
  url: string | null
  query: string | null
  prompt: string | null
  contentText: string | null
  jsonPreview: string
}

function asObject(value: unknown): ObjectLike | null {
  if (!value) return null
  if (typeof value === "object" && !Array.isArray(value)) {
    return value as ObjectLike
  }
  if (typeof value !== "string") return null
  try {
    const parsed: unknown = JSON.parse(value)
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as ObjectLike)
      : null
  } catch {
    return null
  }
}

function asArray(value: unknown): unknown[] | null {
  return Array.isArray(value) ? value : null
}

function pickValue(record: ObjectLike | null, keys: string[]): unknown {
  if (!record) return null
  for (const key of keys) {
    if (!(key in record)) continue
    const value = record[key]
    if (value !== undefined && value !== null) return value
  }
  return null
}

function pickString(record: ObjectLike | null, keys: string[]): string | null {
  const value = pickValue(record, keys)
  if (typeof value !== "string") return null
  const trimmed = value.trim()
  return trimmed.length > 0 ? trimmed : null
}

function joinStringArray(values: unknown): string | null {
  if (!Array.isArray(values)) return null
  const parts = values.filter(
    (item): item is string => typeof item === "string" && item.trim().length > 0
  )
  return parts.length > 0 ? parts.join(" ") : null
}

function unescapeInlineEscapes(text: string): string {
  return text
    .replace(/\\r\\n/g, "\n")
    .replace(/\\n/g, "\n")
    .replace(/\\t/g, "\t")
}

function looksLikeDiffPayload(input: string): boolean {
  const normalized = unescapeInlineEscapes(input)
  return (
    normalized.includes("*** Begin Patch") ||
    normalized.includes("*** Update File:") ||
    /^diff --git /m.test(normalized) ||
    (/^--- .+/m.test(normalized) && /^\+\+\+ .+/m.test(normalized)) ||
    /^@@ /m.test(normalized)
  )
}

function buildCompactDiffFromTexts(
  path: string,
  oldText: string,
  newText: string,
  contextLines: number = 2,
  startLine: number = 1
): string | null {
  const oldLines = splitNormalizedLines(oldText)
  const newLines = splitNormalizedLines(newText)

  let prefix = 0
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  ) {
    prefix += 1
  }

  let suffix = 0
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] ===
      newLines[newLines.length - 1 - suffix]
  ) {
    suffix += 1
  }

  const removed = oldLines.slice(prefix, oldLines.length - suffix)
  const added = newLines.slice(prefix, newLines.length - suffix)
  if (removed.length === 0 && added.length === 0) return null

  const before = oldLines.slice(Math.max(0, prefix - contextLines), prefix)
  const after = oldLines.slice(
    oldLines.length - suffix,
    Math.min(oldLines.length, oldLines.length - suffix + contextLines)
  )

  const oldStart = Math.max(1, startLine + prefix - before.length)
  const oldCount = before.length + removed.length + after.length
  const newCount = before.length + added.length + after.length

  const parts: string[] = [
    `--- ${path}`,
    `+++ ${path}`,
    `@@ -${oldStart},${oldCount} +${oldStart},${newCount} @@`,
  ]
  for (const line of before) parts.push(` ${line}`)
  for (const line of removed) parts.push(`-${line}`)
  for (const line of added) parts.push(`+${line}`)
  for (const line of after) parts.push(` ${line}`)

  return parts.join("\n")
}

function buildDiffPreviewFromChanges(
  changes: PermissionFileChange[],
  maxFiles: number = 8,
  maxLines: number = 1200
): string | null {
  const meaningful = changes.filter((change) => {
    if (
      typeof change.unifiedDiff === "string" &&
      change.unifiedDiff.trim().length > 0
    ) {
      return true
    }
    return change.oldText.length > 0 || change.newText.length > 0
  })
  if (meaningful.length === 0) return null

  const limited = meaningful.slice(0, maxFiles)
  const lines: string[] = []
  let lineCount = 0
  let truncated = false

  const pushLine = (line: string) => {
    if (lineCount >= maxLines) {
      truncated = true
      return
    }
    lines.push(line)
    lineCount += 1
  }

  for (const change of limited) {
    const block =
      typeof change.unifiedDiff === "string" &&
      change.unifiedDiff.trim().length > 0
        ? change.unifiedDiff.trim()
        : buildCompactDiffFromTexts(
            change.path,
            change.oldText,
            change.newText,
            2,
            change.startLine ?? 1
          )
    if (!block) continue

    for (const line of block.split("\n")) {
      pushLine(line)
      if (truncated) break
    }
    if (truncated) break
    pushLine("")
  }

  if (meaningful.length > limited.length) {
    lines.push(`# ... ${meaningful.length - limited.length} more files omitted`)
  }
  if (truncated) {
    lines.push("# ... diff preview truncated")
  }

  const preview = lines.join("\n").trim()
  return preview.length > 0 ? preview : null
}

function stringifyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value ?? "")
  }
}

function extractCommandFromUnknownValue(
  value: unknown,
  depth: number = 0
): string | null {
  if (depth > 4 || value === null || value === undefined) return null
  if (typeof value === "string") {
    const trimmed = value.trim()
    if (!trimmed || looksLikeDiffPayload(trimmed)) return null
    if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) {
      return trimmed
    }
    try {
      const parsed: unknown = JSON.parse(trimmed)
      return extractCommandFromUnknownValue(parsed, depth + 1)
    } catch {
      return null
    }
  }

  if (Array.isArray(value)) {
    const joined = joinStringArray(value)
    return joined && joined.trim().length > 0 ? joined.trim() : null
  }

  if (typeof value !== "object") return null
  const obj = value as ObjectLike

  const directKeys = [
    "command",
    "cmd",
    "script",
    "args",
    "argv",
    "command_args",
  ]
  for (const key of directKeys) {
    const direct = extractCommandFromUnknownValue(obj[key], depth + 1)
    if (direct) return direct
  }

  const nestedKeys = [
    "rawInput",
    "raw_input",
    "input",
    "arguments",
    "params",
    "payload",
  ]
  for (const key of nestedKeys) {
    const nested = extractCommandFromUnknownValue(obj[key], depth + 1)
    if (nested) return nested
  }

  return null
}

function extractDiffPreview(
  rawInput: unknown,
  rawInputObj: ObjectLike | null
): string | null {
  const candidates: unknown[] = [rawInput]
  if (rawInputObj) {
    candidates.push(
      rawInputObj.patch,
      rawInputObj.diff,
      rawInputObj.unified_diff,
      rawInputObj.unifiedDiff
    )
  }

  for (const candidate of candidates) {
    if (typeof candidate !== "string") continue
    const normalized = unescapeInlineEscapes(candidate).trim()
    if (!normalized) continue
    if (looksLikeDiffPayload(normalized)) return normalized
  }

  return null
}

function parseChangeRecord(
  path: string,
  value: unknown
): PermissionFileChange | null {
  const normalizedPath = path.trim()
  if (!normalizedPath) return null

  if (typeof value === "string") {
    return {
      path: normalizedPath,
      oldText: "",
      newText: value,
      unifiedDiff: undefined,
    }
  }

  const record = asObject(value)
  if (!record) {
    return {
      path: normalizedPath,
      oldText: "",
      newText: "",
    }
  }

  const oldText =
    pickString(record, [
      "old_string",
      "oldString",
      "old_text",
      "oldText",
      "old",
      "before",
    ]) ?? ""
  const newText =
    pickString(record, [
      "new_string",
      "newString",
      "new_text",
      "newText",
      "new",
      "after",
      "content",
      "text",
      "new_source",
      "newSource",
    ]) ?? ""
  const unifiedDiff =
    pickString(record, ["unifiedDiff", "unified_diff", "diff", "patch"]) ??
    undefined

  const rawStartLine = record._start_line ?? record.start_line
  const startLine =
    typeof rawStartLine === "number" && rawStartLine > 0
      ? rawStartLine
      : undefined

  return {
    path: normalizedPath,
    oldText,
    newText,
    unifiedDiff,
    startLine,
  }
}

function extractRawInputFileChanges(
  rawInputObj: ObjectLike | null
): PermissionFileChange[] {
  if (!rawInputObj) return []

  const changes: PermissionFileChange[] = []
  const byChangesObject = asObject(rawInputObj.changes)
  if (byChangesObject) {
    for (const [path, value] of Object.entries(byChangesObject)) {
      const parsed = parseChangeRecord(path, value)
      if (parsed) changes.push(parsed)
    }
  }

  const directPath =
    pickString(rawInputObj, [
      "file_path",
      "filePath",
      "path",
      "notebook_path",
      "target_file",
      "targetFile",
    ]) ?? null

  if (directPath) {
    const oldText =
      pickString(rawInputObj, [
        "old_string",
        "oldString",
        "old_text",
        "oldText",
      ]) ?? ""
    const newText =
      pickString(rawInputObj, [
        "new_string",
        "newString",
        "new_text",
        "newText",
        "content",
        "text",
        "new_source",
      ]) ?? ""

    if (oldText || newText || changes.length === 0) {
      const rawSl = rawInputObj._start_line ?? rawInputObj.start_line
      changes.push({
        path: directPath,
        oldText,
        newText,
        unifiedDiff: undefined,
        startLine: typeof rawSl === "number" && rawSl > 0 ? rawSl : undefined,
      })
    }
  }

  return changes
}

function extractContentDiffChanges(
  toolCallObj: ObjectLike | null
): PermissionFileChange[] {
  if (!toolCallObj) return []
  const content = asArray(toolCallObj.content)
  if (!content) return []

  const changes: PermissionFileChange[] = []
  for (const item of content) {
    const record = asObject(item)
    if (!record) continue
    const type = pickString(record, ["type"])?.toLowerCase()
    if (type !== "diff") continue

    const path = pickString(record, ["path"])
    if (!path) continue
    changes.push({
      path,
      oldText: pickString(record, ["old_text", "oldText"]) ?? "",
      newText: pickString(record, ["new_text", "newText"]) ?? "",
      unifiedDiff: undefined,
    })
  }
  return changes
}

/**
 * Pull the agent's human-readable description out of the ACP `content` array.
 *
 * Some agents (e.g. Kimi Code) populate nothing in `rawInput` and instead carry
 * the request description as a `ToolCallContent::Content` item — shaped
 * `{ type: "content", content: { type: "text", text: "..." } }`. Without this,
 * `parsePermissionToolCall` finds no structured fields and the dialog falls back
 * to dumping raw JSON. ACP says clients SHOULD render this text as Markdown.
 *
 * Defensive about shape: also accepts a flattened `{ type: "text", text }` item
 * and a directly-stringified `content`. Diff-like payloads are skipped here —
 * they are handled by `extractContentDiffChanges`.
 */
function extractContentText(toolCallObj: ObjectLike | null): string | null {
  if (!toolCallObj) return null
  const content = asArray(toolCallObj.content)
  if (!content) return null

  const parts: string[] = []
  for (const item of content) {
    const record = asObject(item)
    if (!record) continue
    const type = pickString(record, ["type"])?.toLowerCase()
    // Only consider text-bearing items; skip diff / terminal / image / audio.
    if (type && type !== "content" && type !== "text") continue

    let text: string | null = null
    const inner = asObject(record.content)
    if (inner) {
      const innerType = pickString(inner, ["type"])?.toLowerCase()
      if (!innerType || innerType === "text") {
        text = pickString(inner, ["text"])
      }
    } else if (typeof record.content === "string") {
      const trimmed = record.content.trim()
      text = trimmed.length > 0 ? trimmed : null
    }
    if (!text) text = pickString(record, ["text"])
    if (!text || looksLikeDiffPayload(text)) continue
    parts.push(text)
  }

  if (parts.length === 0) return null
  const joined = parts.join("\n\n").trim()
  return joined.length > 0 ? joined : null
}

function collectLocationPaths(toolCallObj: ObjectLike | null): string[] {
  if (!toolCallObj) return []
  const locations = asArray(toolCallObj.locations)
  if (!locations) return []

  const paths: string[] = []
  for (const item of locations) {
    const record = asObject(item)
    if (!record) continue
    const path = pickString(record, ["path"])
    if (path) paths.push(path)
  }
  return paths
}

function collectDiffPaths(diffText: string | null): string[] {
  if (!diffText) return []
  const paths = new Set<string>()
  for (const line of diffText.split("\n")) {
    if (line.startsWith("*** Add File: ")) {
      paths.add(line.slice(14).trim())
      continue
    }
    if (line.startsWith("*** Update File: ")) {
      paths.add(line.slice(17).trim())
      continue
    }
    if (line.startsWith("*** Delete File: ")) {
      paths.add(line.slice(17).trim())
      continue
    }
    if (line.startsWith("+++ ")) {
      const path = line.slice(4).replace(/^b\//, "").trim()
      if (path && path !== "/dev/null") paths.add(path)
    }
  }
  return Array.from(paths)
}

function mergeFileChanges(
  changes: PermissionFileChange[]
): PermissionFileChange[] {
  const merged = new Map<string, PermissionFileChange>()
  for (const change of changes) {
    const path = change.path.trim()
    if (!path) continue
    const prev = merged.get(path)
    if (!prev) {
      merged.set(path, { ...change, path })
      continue
    }

    const oldText = prev.oldText || change.oldText
    const newText = prev.newText || change.newText
    const unifiedDiff = prev.unifiedDiff || change.unifiedDiff
    const startLine = prev.startLine ?? change.startLine
    merged.set(path, { path, oldText, newText, unifiedDiff, startLine })
  }
  return Array.from(merged.values())
}

function parsePlanEntries(
  rawInputObj: ObjectLike | null
): PermissionPlanEntry[] {
  if (!rawInputObj) return []

  const candidates = [
    pickValue(rawInputObj, ["plan"]),
    pickValue(rawInputObj, ["entries"]),
    pickValue(rawInputObj, ["steps"]),
    pickValue(rawInputObj, ["todos"]),
  ]

  for (const candidate of candidates) {
    const list = asArray(candidate)
    if (!list || list.length === 0) continue
    const entries: PermissionPlanEntry[] = []
    for (const item of list) {
      const record = asObject(item)
      if (!record) continue
      const text =
        pickString(record, [
          "step",
          "content",
          "title",
          "task",
          "description",
        ]) ?? null
      if (!text) continue
      entries.push({
        text,
        status: pickString(record, ["status", "state"]),
      })
    }
    if (entries.length > 0) return entries
  }

  return []
}

function parseAllowedPrompts(
  rawInputObj: ObjectLike | null
): PermissionAllowedPrompt[] {
  if (!rawInputObj) return []
  const list = asArray(
    pickValue(rawInputObj, ["allowedPrompts", "allowed_prompts"])
  )
  if (!list || list.length === 0) return []

  const prompts: PermissionAllowedPrompt[] = []
  for (const item of list) {
    const record = asObject(item)
    if (!record) continue
    const prompt = pickString(record, ["prompt", "description", "text"])
    const tool = pickString(record, ["tool", "toolName", "tool_name"])
    if (prompt) {
      prompts.push({ prompt, tool: tool ?? "" })
    }
  }
  return prompts
}

function formatFallbackTitle(kind: string): string {
  const normalized = kind.replace(/_/g, " ").trim()
  if (!normalized) return "Permission Request"
  return normalized
    .split(/\s+/)
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join(" ")
}

export function parsePermissionToolCall(
  toolCall: unknown
): ParsedPermissionToolCall {
  const toolCallObj = asObject(toolCall)
  const rawKind =
    pickString(toolCallObj, [
      "kind",
      "tool_name",
      "toolName",
      "name",
      "type",
    ]) ?? "tool"
  const normalizedKind = normalizeToolName(rawKind)

  const rawInputValue =
    pickValue(toolCallObj, [
      "rawInput",
      "raw_input",
      "input",
      "arguments",
      "params",
      "payload",
    ]) ?? null
  const rawInputObj = asObject(rawInputValue)

  const command =
    extractCommandFromUnknownValue(rawInputValue) ??
    extractCommandFromUnknownValue(toolCallObj)

  const cwd =
    pickString(rawInputObj, [
      "cwd",
      "workdir",
      "working_directory",
      "workingDirectory",
    ]) ??
    pickString(toolCallObj, [
      "cwd",
      "workdir",
      "working_directory",
      "workingDirectory",
    ])

  const explicitDiffPreview = extractDiffPreview(rawInputValue, rawInputObj)
  const rawInputFileChanges = extractRawInputFileChanges(rawInputObj)
  const contentDiffChanges = extractContentDiffChanges(toolCallObj)
  const locationPaths = collectLocationPaths(toolCallObj)
  const diffPaths = collectDiffPaths(explicitDiffPreview)

  const combinedChanges = mergeFileChanges([
    ...rawInputFileChanges,
    ...contentDiffChanges,
    ...locationPaths.map((path) => ({
      path,
      oldText: "",
      newText: "",
      unifiedDiff: undefined,
    })),
    ...diffPaths.map((path) => ({
      path,
      oldText: "",
      newText: "",
      unifiedDiff: undefined,
    })),
  ])
  const diffPreview =
    explicitDiffPreview ?? buildDiffPreviewFromChanges(combinedChanges)

  let additions = 0
  let deletions = 0
  if (diffPreview) {
    const stats = countUnifiedDiffLineChanges(diffPreview)
    additions = stats.additions
    deletions = stats.deletions
  } else {
    for (const change of combinedChanges) {
      if (
        typeof change.unifiedDiff === "string" &&
        change.unifiedDiff.trim().length > 0
      ) {
        const stats = countUnifiedDiffLineChanges(change.unifiedDiff)
        additions += stats.additions
        deletions += stats.deletions
        continue
      }
      const stats = estimateChangedLineStats(change.oldText, change.newText)
      additions += stats.additions
      deletions += stats.deletions
    }
  }

  const planEntries = parsePlanEntries(rawInputObj)
  const planExplanation = pickString(rawInputObj, ["explanation"])

  const rawPlan = rawInputObj ? pickValue(rawInputObj, ["plan"]) : null
  const planMarkdown =
    typeof rawPlan === "string" && rawPlan.trim().length > 0 ? rawPlan : null

  const allowedPrompts = parseAllowedPrompts(rawInputObj)

  const modeTarget =
    pickString(rawInputObj, [
      "mode_id",
      "modeId",
      "target_mode",
      "targetMode",
    ]) ?? null

  const url =
    pickString(rawInputObj, ["url"]) ?? pickString(toolCallObj, ["url"])
  const query =
    pickString(rawInputObj, ["query"]) ?? pickString(toolCallObj, ["query"])
  const prompt =
    pickString(rawInputObj, ["prompt"]) ?? pickString(toolCallObj, ["prompt"])

  const contentText = extractContentText(toolCallObj)

  const title =
    pickString(toolCallObj, ["title", "tool_name", "toolName", "name"]) ??
    formatFallbackTitle(normalizedKind)

  // NOTE the underscore key: the permission request forwards the raw ACP
  // ToolCallUpdate serialization (serde renames `meta` → `_meta`), unlike tool
  // parts whose event field is plain `meta`.
  const metaObj = asObject(pickValue(toolCallObj, ["_meta"]))

  // `_meta.claudeCode.title` (claude-agent-acp ≥0.63), else `_meta.permission
  // .title` — the backend hoists the latter off the REQUEST for codex-acp
  // ≥1.7.0, whose four fixed titles ("Run command?", "Make edits?", …) read
  // better as a heading than the generic `toolCall.title` beside them
  // ("Run command", "Edit files").
  const description =
    pickString(asObject(pickValue(metaObj, ["claudeCode"])), ["title"]) ??
    pickString(asObject(pickValue(metaObj, ["permission"])), ["title"]) ??
    null

  // Why the agent needs this approval, in its own words. codex-acp ≥1.7.0 puts
  // Codex's `reason` here; before 1.7.0 the same sentence WAS `toolCall.title`,
  // so leaving it unread would quietly drop the most decision-relevant line on
  // the card. Only `version: 1` is honoured — a reshaped revision is better
  // shown as nothing than half-understood.
  const reason = parsePermissionMetaDescription(
    pickValue(metaObj, ["permission"])
  )

  return {
    title,
    description,
    reason,
    normalizedKind,
    command,
    cwd,
    fileChanges: combinedChanges,
    additions,
    deletions,
    diffPreview,
    planEntries,
    planExplanation,
    planMarkdown,
    allowedPrompts,
    modeTarget,
    url,
    query,
    prompt,
    contentText,
    jsonPreview: stringifyJson(toolCallObj ?? toolCall),
  }
}

/**
 * Maximum permission-change descriptions rendered for one option, and the
 * per-description character cap. The wire list is agent-authored and rides the
 * broadcast event + snapshot, so a bound keeps a pathological (or simply very
 * broad) grant from turning the permission card into a wall of text. Anything
 * past the cap is dropped rather than summarized — the option's own name still
 * states what it does.
 */
const MAX_PERMISSION_CHANGES = 6
const MAX_PERMISSION_CHANGE_CHARS = 200

/**
 * How long a permission change lasts, and where it is written — normalized from
 * the wire's `lifetime: {scope, storage?}` pair into one flat token the card can
 * label directly.
 *
 * `persistent` is the deliberate catch-all for a `scope: "persistent"` whose
 * `storage` we do not recognize: the destination is unknown but the fact that it
 * OUTLIVES the session is the part the user must not miss, so it degrades to a
 * weaker warning rather than to silence.
 */
export type PermissionChangeScope =
  | "session"
  | "process"
  | "user"
  | "project"
  | "project_local"
  | "persistent"

export interface PermissionOptionChange {
  /** The agent's own rendered sentence for this change. */
  description: string
  /**
   * `null` when the change carries no lifetime, reports `scope: "unknown"`, or
   * uses a scope this build does not know — the card then says nothing about
   * duration rather than guessing at one.
   */
  scope: PermissionChangeScope | null
}

/** Wire `lifetime` → {@link PermissionChangeScope}; see the type's doc. */
function parseChangeScope(lifetime: unknown): PermissionChangeScope | null {
  const record = asObject(lifetime)
  const scope = pickString(record, ["scope"])
  if (scope === "session") return "session"
  if (scope === "process") return "process"
  if (scope !== "persistent") return null
  switch (pickString(record, ["storage"])) {
    case "user":
      return "user"
    case "project":
      return "project"
    case "project_local":
      return "project_local"
    default:
      return "persistent"
  }
}

/**
 * The plain `description` of a version-1 `_meta.permission` block, trimmed to the
 * card's budget. Shared by the request level and the flat option form; the
 * `changes[]` form is parsed by {@link parsePermissionOptionChanges} instead.
 *
 * Only `version: 1` is honoured — a future revision may reshape the block, and
 * showing it half-understood is worse than showing nothing.
 */
function parsePermissionMetaDescription(permission: unknown): string | null {
  const record = asObject(permission)
  if (!record || record.version !== 1) return null
  const description = pickString(record, ["description"])
  return description ? description.slice(0, MAX_PERMISSION_CHANGE_CHARS) : null
}

/**
 * What picking a permission option would change: the agent's own sentence for
 * each change, plus how long it lasts.
 *
 * Two shapes, both under `_meta.permission` on a `PermissionOption`:
 *
 * - `{version: 1, changes: [...]}` — claude-agent-acp 0.64.1–0.72.0 (#930) and
 *   codex-acp 1.1.8–1.6.2 (#342). Every change carries a rendered English
 *   sentence ("Allow access to api.example.com for this session", "Allow all
 *   Bash calls").
 * - `{version: 1, description}` — codex-acp ≥1.7.0, which dropped `changes[]`
 *   entirely. It sends this only on MCP elicitation approvals ("Run the tool and
 *   remember this choice for this session."); command and file-change options
 *   now spell the grant out in the option NAME instead, which the button already
 *   renders. Read as a single scope-less change so those cards keep their
 *   per-option explanation.
 *
 * BOTH producers are now historical: claude-agent-acp 0.73.0 rebuilt its
 * permission layer the way codex 1.7.0 did and its options carry NO `_meta` at
 * all, so on the pinned versions this returns `[]` every time. It is kept
 * because the adapter version is user-selectable (`supports_custom_version()`
 * is true for every npx agent), so a pin anywhere in the ranges above still
 * feeds it.
 *
 * `lifetime` is read (in the `changes[]` form) because `description` alone does
 * NOT always answer "for how long": codex wrote the duration into its sentences,
 * claude (through 0.72.0) did not — it reported `{scope: "session"}` vs
 * `{scope: "persistent", storage: "project"}` structurally instead. Left unread,
 * claude's most common card would pair an "Always Allow" button with "Allow all
 * Bash calls" and never reveal that the grant expires with the session (or,
 * worse, that it is about to be written into settings the repo commits). 0.73.0
 * closes that gap on its own by naming the grant AND its duration in the button
 * ("Yes, and always allow access to <paths> from this project", "Yes, during
 * this session"), which is why losing the scope chip there is acceptable rather
 * than a regression to repair. The remaining structural fields (`targets`,
 * `ruleBehavior`) stay ignored: those `description` really does summarize.
 * codex's flat form carries no lifetime at all, hence `scope: null`.
 */
export function parsePermissionOptionChanges(
  meta: Record<string, unknown> | null | undefined
): PermissionOptionChange[] {
  const permission = asObject(pickValue(asObject(meta), ["permission"]))
  if (!permission || permission.version !== 1) return []
  const changes = permission.changes
  if (!Array.isArray(changes)) {
    const description = parsePermissionMetaDescription(permission)
    return description ? [{ description, scope: null }] : []
  }
  const out: PermissionOptionChange[] = []
  for (const change of changes) {
    if (out.length >= MAX_PERMISSION_CHANGES) break
    const record = asObject(change)
    const description = pickString(record, ["description"])
    if (!description) continue
    out.push({
      description: description.slice(0, MAX_PERMISSION_CHANGE_CHARS),
      scope: parseChangeScope(pickValue(record, ["lifetime"])),
    })
  }
  return out
}
