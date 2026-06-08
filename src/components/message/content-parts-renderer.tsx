import { memo, useMemo, useState, type ReactNode } from "react"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import {
  classifyToolKind,
  TOOL_KIND_ORDER,
  type ToolKindLabel,
} from "@/lib/adapters/tool-kind-classifier"
import type { MessageRole } from "@/lib/types"
import { normalizeToolName } from "@/lib/tool-call-normalization"
import { isDelegateToAgentToolName } from "@/lib/delegation-card"
import { useTranslations } from "next-intl"
import { cn } from "@/lib/utils"
import {
  countUnifiedDiffLineChanges,
  estimateChangedLineStats,
} from "@/lib/line-change-stats"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import {
  Tool,
  ToolHeader,
  ToolContent,
  ToolOutput,
} from "@/components/ai-elements/tool"
import { Terminal } from "@/components/ai-elements/terminal"
import { CodeBlock } from "@/components/ai-elements/code-block"
import { UnifiedDiffPreview } from "@/components/diff/unified-diff-preview"
import { generateUnifiedDiff } from "@/lib/unified-diff-generator"
import { FilePathLink } from "@/components/ai-elements/link-safety"
import {
  Reasoning,
  ReasoningTrigger,
  ReasoningContent,
} from "@/components/ai-elements/reasoning"
import { AgentToolCallPart } from "./agent-tool-call"
import { DelegatedSubThread } from "./delegated-sub-thread"
import { DelegationStatusCard } from "./delegation-status-card"
import { DelegationStatusGroupCard } from "./delegation-status-group-card"
import { GeneratedImagesBlock } from "./generated-images-block"
import { GoalRunPart, GoalToolCallPart } from "./goal-tool-call"
import {
  FileTextIcon,
  FilePenLineIcon,
  FilePlusIcon,
  TerminalIcon,
  SearchIcon,
  GlobeIcon,
  ListTodoIcon,
  SparklesIcon,
  CircleIcon,
  CircleDotIcon,
  CircleCheckIcon,
  CompassIcon,
  MapIcon,
  MinusIcon,
  PlusIcon,
  WrenchIcon,
  ChevronRightIcon,
  BrainIcon,
} from "lucide-react"

// ── helpers ────────────────────────────────────────────────────────────

/** Try JSON.parse; return null on failure. */
export function tryParseJson(s: string): Record<string, unknown> | null {
  try {
    const v = JSON.parse(s)
    return typeof v === "object" && v !== null && !Array.isArray(v) ? v : null
  } catch {
    return null
  }
}

/** Regex-extract a JSON string value for a given key (works on truncated JSON). */
export function extractJsonField(input: string, key: string): string | null {
  const re = new RegExp(`"${key}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"`)
  const m = input.match(re)
  return m?.[1]?.replace(/\\"/g, '"').replace(/\\\\/g, "\\") ?? null
}

function asObjectLike(value: unknown): Record<string, unknown> | null {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>
  }
  if (typeof value !== "string") return null
  const trimmed = value.trim()
  if (!trimmed.startsWith("{")) return null
  return tryParseJson(trimmed)
}

const NESTED_PAYLOAD_KEYS = ["input", "arguments", "params", "payload"]

function findStringFieldDeep(
  value: unknown,
  key: string,
  depth: number = 0
): string | null {
  if (depth > 4) return null
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findStringFieldDeep(item, key, depth + 1)
      if (found) return found
    }
    return null
  }
  const obj = asObjectLike(value)
  if (!obj) return null

  const direct = obj[key]
  if (typeof direct === "string" && direct.trim().length > 0) {
    return direct
  }

  for (const nestedKey of NESTED_PAYLOAD_KEYS) {
    const found = findStringFieldDeep(obj[nestedKey], key, depth + 1)
    if (found) return found
  }

  for (const nestedValue of Object.values(obj)) {
    const found = findStringFieldDeep(nestedValue, key, depth + 1)
    if (found) return found
  }

  return null
}

function findObjectFieldDeep(
  value: unknown,
  key: string,
  depth: number = 0
): Record<string, unknown> | null {
  if (depth > 4) return null
  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findObjectFieldDeep(item, key, depth + 1)
      if (found) return found
    }
    return null
  }
  const obj = asObjectLike(value)
  if (!obj) return null

  const direct = asObjectLike(obj[key])
  if (direct) return direct

  for (const nestedKey of NESTED_PAYLOAD_KEYS) {
    const found = findObjectFieldDeep(obj[nestedKey], key, depth + 1)
    if (found) return found
  }

  for (const nestedValue of Object.values(obj)) {
    const found = findObjectFieldDeep(nestedValue, key, depth + 1)
    if (found) return found
  }

  return null
}

function decodeJsonEscapedString(value: string): string {
  return value.replace(/\\"/g, '"').replace(/\\\//g, "/").replace(/\\\\/g, "\\")
}

function extractEditPathsFromChangesPayload(
  input: string,
  parsed: Record<string, unknown> | null
): string[] {
  const changes = findObjectFieldDeep(parsed, "changes")
  if (changes) {
    const paths = Object.keys(changes)
      .map((path) => path.trim())
      .filter((path) => path.length > 0)
    if (paths.length > 0) return paths
  }

  const firstPathMatch = input.match(/"changes"\s*:\s*\{\s*"((?:[^"\\]|\\.)+)"/)
  if (!firstPathMatch?.[1]) return []

  return [decodeJsonEscapedString(firstPathMatch[1])]
}

function extractPathFromDiffText(
  text: string | null | undefined
): string | null {
  if (!text) return null
  const match = text.match(/^(?:---|\+\+\+)\s+([^\n]+)$/m)
  if (!match?.[1]) return null
  const raw = match[1].trim()
  if (!raw || raw === "/dev/null") return null
  return raw.replace(/^[ab]\//, "")
}

function isLikelyIdField(key: string): boolean {
  const lower = key.toLowerCase()
  return (
    lower === "id" ||
    lower === "uuid" ||
    lower === "callid" ||
    lower === "call_id" ||
    lower === "tool_call_id" ||
    lower.endsWith("_id") ||
    lower.endsWith("id")
  )
}

/** Shorten an absolute path to its last 2 segments. */
function shortPath(p: string): string {
  return p.split("/").slice(-2).join("/")
}

/** Truncate text to maxLen, appending "…" if truncated. */
export function ellipsis(s: string, maxLen: number): string {
  return s.length > maxLen ? s.slice(0, maxLen - 1) + "…" : s
}

function unwrapQuotedCommand(command: string): string {
  const trimmed = command.trim()
  if (trimmed.length < 2) return trimmed

  if (trimmed.startsWith("'") && trimmed.endsWith("'")) {
    return trimmed.slice(1, -1)
  }

  if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed
      .slice(1, -1)
      .replace(/\\"/g, '"')
      .replace(/\\n/g, "\n")
      .replace(/\\\\/g, "\\")
  }

  return trimmed
}

function simplifyShellCommand(command: string): string {
  let current = command.trim()
  const wrapperRe =
    /^(?:\/usr\/bin\/env\s+)?(?:(?:\/[^\s]+\/)?(?:bash|zsh|sh))\s+-(?:l?c)\s+(.+)$/i

  // Strip nested shell wrappers like "/bin/zsh -lc bash -lc '<cmd>'".
  for (let i = 0; i < 6; i += 1) {
    const wrapped = current.match(wrapperRe)
    if (!wrapped) break
    const next = unwrapQuotedCommand(wrapped[1] ?? "").trim()
    if (!next || next === current) break
    current = next
  }

  return current
}

function extractDisplayCommandFromToolInput(
  input: string | null | undefined
): string | null {
  if (!input) return null
  const parsed = tryParseJson(input)
  const command =
    (parsed ? commandFromUnknownValue(parsed) : null) ??
    extractCommandFromUnknownInput(input)
  if (!command) return null
  const simplified = simplifyShellCommand(command).trim()
  return simplified.length > 0 ? simplified : null
}

function formatCommandPrompt(command: string): string {
  return command
    .split("\n")
    .map((line, index) => `${index === 0 ? "$" : ">"} ${line}`)
    .join("\n")
}

function buildCommandTerminalOutput(
  command: string | null,
  output: string | null,
  isStreaming: boolean = false
): string {
  if (!command) return output ?? ""
  const prompt = formatCommandPrompt(command)
  const terminalOutput = output ?? ""
  const withTrailingNewline = (text: string): string =>
    text.endsWith("\n") ? text : `${text}\n`
  if (!terminalOutput) {
    return isStreaming ? withTrailingNewline(prompt) : prompt
  }

  const lines = terminalOutput.split("\n")
  const firstNonEmptyLine = lines.find((line) => line.trim().length > 0)
  const commandFirstLine = command.split("\n")[0]?.trim() ?? ""

  if (firstNonEmptyLine) {
    const trimmedLine = firstNonEmptyLine.trim()
    const lineWithoutPrompt = trimmedLine.replace(/^\$\s*/, "")
    if (
      trimmedLine === commandFirstLine ||
      lineWithoutPrompt === commandFirstLine
    ) {
      if (isStreaming && !terminalOutput.includes("\n")) {
        return withTrailingNewline(terminalOutput)
      }
      return terminalOutput
    }
  }

  return `${prompt}\n${terminalOutput}`
}

function extractCommandFromUnknownInput(input: string): string | null {
  const trimmed = input.trim()
  if (!trimmed) return null

  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (typeof parsed === "string") {
      return parsed
    }
    if (Array.isArray(parsed)) {
      const parts = parsed.filter((p): p is string => typeof p === "string")
      if (parts.length > 0) return parts.join(" ")
    }
    if (parsed && typeof parsed === "object") {
      const obj = parsed as Record<string, unknown>
      const direct = obj.command ?? obj.cmd ?? obj.script
      if (typeof direct === "string") {
        return direct
      }
      if (Array.isArray(direct)) {
        const parts = direct.filter((p): p is string => typeof p === "string")
        if (parts.length > 0) return parts.join(" ")
      }
    }
  } catch {
    // Non-JSON command text is handled below.
  }

  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    return null
  }
  return trimmed
}

function commandFromUnknownValue(value: unknown): string | null {
  if (typeof value === "string") {
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : null
  }

  if (Array.isArray(value)) {
    const parts = value
      .map((item) => (typeof item === "string" ? item : null))
      .filter((item): item is string => item !== null && item.length > 0)
    if (parts.length > 0) {
      return parts.join(" ")
    }
    return null
  }

  if (!value || typeof value !== "object") {
    return null
  }

  const obj = value as Record<string, unknown>
  const directKeys = [
    "command",
    "cmd",
    "script",
    "args",
    "argv",
    "command_args",
  ]
  for (const key of directKeys) {
    const found = commandFromUnknownValue(obj[key])
    if (found) return found
  }

  const nestedKeys = ["input", "arguments", "params", "payload"]
  for (const key of nestedKeys) {
    const found = commandFromUnknownValue(obj[key])
    if (found) return found
  }

  return null
}

/** Get string field from parsed object */
function str(obj: Record<string, unknown>, key: string): string | undefined {
  const v = obj[key]
  return typeof v === "string" ? v : undefined
}

/** Get number field from parsed object */
function num(obj: Record<string, unknown>, key: string): number | undefined {
  const v = obj[key]
  return typeof v === "number" ? v : undefined
}

type ApplyPatchOp = "add" | "update" | "delete" | "move"

type ApplyPatchFile = {
  op: ApplyPatchOp
  path: string
  from?: string
  to?: string
}

type LineChangeStats = {
  additions: number
  deletions: number
}

function parseApplyPatchInput(input: string): {
  files: ApplyPatchFile[]
  additions: number
  deletions: number
} {
  const files: ApplyPatchFile[] = []
  let currentFileIndex = -1
  let additions = 0
  let deletions = 0

  for (const line of input.split("\n")) {
    if (line.startsWith("*** Add File: ")) {
      files.push({ op: "add", path: line.slice(14).trim() })
      currentFileIndex = files.length - 1
      continue
    }
    if (line.startsWith("*** Update File: ")) {
      files.push({ op: "update", path: line.slice(17).trim() })
      currentFileIndex = files.length - 1
      continue
    }
    if (line.startsWith("*** Delete File: ")) {
      files.push({ op: "delete", path: line.slice(17).trim() })
      currentFileIndex = files.length - 1
      continue
    }
    if (line.startsWith("*** Move to: ")) {
      const to = line.slice(13).trim()
      if (currentFileIndex >= 0) {
        const current = files[currentFileIndex]
        files[currentFileIndex] = {
          op: "move",
          path: `${current.path} -> ${to}`,
          from: current.path,
          to,
        }
      }
      continue
    }
    if (line.startsWith("+") && !line.startsWith("+++")) {
      additions += 1
      continue
    }
    if (line.startsWith("-") && !line.startsWith("---")) {
      deletions += 1
    }
  }

  return { files, additions, deletions }
}

function hasLineChanges(
  stats: LineChangeStats | null | undefined
): stats is LineChangeStats {
  return !!stats && (stats.additions > 0 || stats.deletions > 0)
}

function looksLikeDiffPayload(input: string): boolean {
  if (!input.trim()) return false
  const normalized = unescapeInlineEscapes(input)

  return (
    normalized.includes("*** Begin Patch") ||
    normalized.includes("*** Update File:") ||
    /^diff --git /m.test(normalized) ||
    (/^--- .+/m.test(normalized) && /^\+\+\+ .+/m.test(normalized)) ||
    /^@@ /m.test(normalized)
  )
}

function extractEditLineChangeStats(
  input: string | null | undefined
): LineChangeStats | null {
  if (!input || input.trim().length === 0) return null

  const parsed = tryParseJson(input)
  const patchInput = extractApplyPatchTextFromUnknownInput(input, parsed)
  if (patchInput) {
    const patchStats = parseApplyPatchInput(patchInput)
    const stats = {
      additions: patchStats.additions,
      deletions: patchStats.deletions,
    }
    if (hasLineChanges(stats)) return stats
  }

  if (parsed) {
    const changesPayload = extractEditChangesPayload(parsed)
    if (changesPayload.length > 0) {
      let additions = 0
      let deletions = 0

      for (const change of changesPayload) {
        if (change.unifiedDiff && change.unifiedDiff.trim().length > 0) {
          const diffStats = countUnifiedDiffLineChanges(change.unifiedDiff)
          additions += diffStats.additions
          deletions += diffStats.deletions
          continue
        }

        const estimated = estimateChangedLineStats(
          change.oldText,
          change.newText
        )
        additions += estimated.additions
        deletions += estimated.deletions
      }

      const stats = { additions, deletions }
      if (hasLineChanges(stats)) return stats
    }

    if (isCanonicalEditPayload(parsed)) {
      const oldString =
        str(parsed, "old_string") ?? str(parsed, "old_text") ?? ""
      const newString =
        str(parsed, "new_string") ?? str(parsed, "new_text") ?? ""
      const stats = estimateChangedLineStats(oldString, newString)
      if (hasLineChanges(stats)) return stats
    }

    const parsedDiff =
      findStringFieldDeep(parsed, "unified_diff") ??
      findStringFieldDeep(parsed, "unifiedDiff") ??
      findStringFieldDeep(parsed, "patch") ??
      findStringFieldDeep(parsed, "diff")
    if (parsedDiff && looksLikeDiffPayload(parsedDiff)) {
      const stats = countUnifiedDiffLineChanges(
        unescapeInlineEscapes(parsedDiff)
      )
      if (hasLineChanges(stats)) return stats
    }
  }

  if (looksLikeDiffPayload(input)) {
    const stats = countUnifiedDiffLineChanges(unescapeInlineEscapes(input))
    if (hasLineChanges(stats)) return stats
  }

  return null
}

function unescapeInlineEscapes(text: string): string {
  return text
    .replace(/\\r\\n/g, "\n")
    .replace(/\\n/g, "\n")
    .replace(/\\t/g, "\t")
}

function extractApplyPatchTextFromUnknownInput(
  input: string,
  parsed: Record<string, unknown> | null
): string | null {
  const candidates: string[] = [input]
  const parsedCommand = parsed ? commandFromUnknownValue(parsed) : null
  if (parsedCommand) candidates.push(parsedCommand)

  const fallbackCommand = extractCommandFromUnknownInput(input)
  if (fallbackCommand) candidates.push(fallbackCommand)

  const seen = new Set<string>()

  for (const rawCandidate of candidates) {
    const candidate = rawCandidate.trim()
    if (!candidate || seen.has(candidate)) continue
    seen.add(candidate)

    const variants = [candidate]
    const unescaped = unescapeInlineEscapes(candidate)
    if (unescaped !== candidate) variants.push(unescaped)

    for (const variant of variants) {
      if (!variant.includes("*** Begin Patch")) continue

      const block = variant.match(
        /(\*\*\* Begin Patch[\s\S]*?\*\*\* End Patch(?:\n|$))/m
      )?.[1]

      if (block) return block.trim()
      return variant.trim()
    }
  }

  return null
}

function parseApplyPatchFilesFromUnknownInput(
  input: string,
  parsed: Record<string, unknown> | null
): ApplyPatchFile[] {
  const patchText = extractApplyPatchTextFromUnknownInput(input, parsed)
  if (patchText) {
    const fromPatchText = parseApplyPatchInput(patchText)
    if (fromPatchText.files.length > 0) return fromPatchText.files
  }

  const direct = parseApplyPatchInput(input)
  if (direct.files.length > 0) return direct.files

  const unescaped = unescapeInlineEscapes(input)
  if (unescaped !== input) {
    const normalized = parseApplyPatchInput(unescaped)
    if (normalized.files.length > 0) return normalized.files
  }

  return []
}

function isCanonicalEditPayload(parsed: Record<string, unknown>): boolean {
  return (
    typeof parsed.file_path === "string" ||
    typeof parsed.path === "string" ||
    typeof parsed.old_string === "string" ||
    typeof parsed.new_string === "string" ||
    parsed.replace_all === true
  )
}

type EditChangePreview = {
  path: string
  oldText: string
  newText: string
  unifiedDiff?: string
}

const EDIT_CHANGE_OLD_KEYS = [
  "old_string",
  "oldString",
  "old_text",
  "oldText",
  "old",
  "previous",
  "before",
  "source",
  "original",
]

const EDIT_CHANGE_NEW_KEYS = [
  "new_string",
  "newString",
  "new_text",
  "newText",
  "new_content",
  "newContent",
  "new",
  "new_value",
  "newValue",
  "replacement",
  "after",
  "after_text",
  "afterText",
  "updated",
  "updated_text",
  "updatedText",
  "content",
  "new_source",
  "newSource",
  "text",
]

const EDIT_CHANGE_DIFF_KEYS = ["diff", "patch", "unified_diff", "unifiedDiff"]

function collectLikelyChangeStrings(value: Record<string, unknown>): string[] {
  const entries = Object.entries(value).filter(
    ([, v]) => typeof v === "string" && v.length > 0
  ) as Array<[string, string]>
  if (entries.length === 0) return []

  const preferred = entries
    .filter(([key]) =>
      /(old|new|before|after|content|text|source|replace|value)/i.test(key)
    )
    .map(([, v]) => v)

  if (preferred.length > 0) return preferred

  return entries
    .filter(
      ([key]) =>
        !/^(id|status|type|call_id|callId|source|auto_approved)$/i.test(key)
    )
    .map(([, v]) => v)
}

function firstStringField(
  value: Record<string, unknown>,
  keys: string[]
): string | null {
  for (const key of keys) {
    const field = value[key]
    if (typeof field === "string") {
      return field
    }
  }
  return null
}

function parseEditChangeValue(
  path: string,
  value: unknown
): EditChangePreview | null {
  if (typeof value === "string") {
    return {
      path,
      oldText: "",
      newText: value,
    }
  }

  const record = asObjectLike(value)
  if (!record) return null

  const oldText =
    firstStringField(record, EDIT_CHANGE_OLD_KEYS) ??
    findStringFieldDeep(record, "old_string") ??
    findStringFieldDeep(record, "old_text") ??
    findStringFieldDeep(record, "before_text") ??
    findStringFieldDeep(record, "old") ??
    ""
  const newText =
    firstStringField(record, EDIT_CHANGE_NEW_KEYS) ??
    findStringFieldDeep(record, "new_string") ??
    findStringFieldDeep(record, "new_text") ??
    findStringFieldDeep(record, "after_text") ??
    findStringFieldDeep(record, "new") ??
    ""
  const unifiedDiff =
    firstStringField(record, EDIT_CHANGE_DIFF_KEYS) ??
    findStringFieldDeep(record, "diff") ??
    ""

  if (unifiedDiff) {
    return {
      path,
      oldText,
      newText,
      unifiedDiff,
    }
  }

  if (oldText || newText) {
    return {
      path,
      oldText,
      newText,
    }
  }

  const fallbackStrings = collectLikelyChangeStrings(record)
  if (fallbackStrings.length >= 2) {
    return {
      path,
      oldText: fallbackStrings[0],
      newText: fallbackStrings[1],
    }
  }

  if (fallbackStrings.length === 1) {
    return {
      path,
      oldText: "",
      newText: fallbackStrings[0],
    }
  }

  return {
    path,
    oldText: "",
    newText: "",
  }
}

function extractEditChangesPayload(
  parsed: Record<string, unknown>
): EditChangePreview[] {
  const changes = findObjectFieldDeep(parsed, "changes")
  if (!changes) return []

  const items: EditChangePreview[] = []
  for (const [path, value] of Object.entries(changes)) {
    const normalizedPath = path.trim()
    if (!normalizedPath) continue
    const parsedItem = parseEditChangeValue(normalizedPath, value)
    if (parsedItem) {
      items.push(parsedItem)
    }
  }

  return items
}

// ── tool icon mapping ────────────────────────────────────────────────

const ICON_CLASS = "size-4 text-muted-foreground"

function getTaskToolIcon(input: string | null): ReactNode {
  if (!input) return <ListTodoIcon className={ICON_CLASS} />
  const t = extractJsonField(input, "subagent_type")?.toLowerCase()
  if (!t) return <ListTodoIcon className={ICON_CLASS} />
  if (t.includes("explore")) return <CompassIcon className={ICON_CLASS} />
  if (t.includes("plan")) return <MapIcon className={ICON_CLASS} />
  if (t.includes("bash")) return <TerminalIcon className={ICON_CLASS} />
  return <WrenchIcon className={ICON_CLASS} />
}

function getToolIcon(
  toolName: string,
  input?: string | null
): ReactNode | undefined {
  const name = toolName.toLowerCase()
  if (name === "read" || name === "read file")
    return <FileTextIcon className={ICON_CLASS} />
  if (name === "edit") return <FilePenLineIcon className={ICON_CLASS} />
  if (name === "write" || name === "notebookedit")
    return <FilePlusIcon className={ICON_CLASS} />
  if (name === "bash" || name === "exec_command")
    return <TerminalIcon className={ICON_CLASS} />
  if (name === "apply_patch") return <FilePenLineIcon className={ICON_CLASS} />
  if (name === "glob" || name === "grep")
    return <SearchIcon className={ICON_CLASS} />
  if (name === "memory_recall") return <BrainIcon className={ICON_CLASS} />
  if (name === "webfetch" || name === "websearch")
    return <GlobeIcon className={ICON_CLASS} />
  if (name === "todowrite") return <ListTodoIcon className={ICON_CLASS} />
  if (name === "task") return getTaskToolIcon(input ?? null)
  if (name === "taskcreate" || name === "taskupdate" || name === "tasklist")
    return <ListTodoIcon className={ICON_CLASS} />
  if (name === "agent") return getTaskToolIcon(input ?? null)
  if (name === "skill") return <SparklesIcon className={ICON_CLASS} />
  if (
    name === "enterplanmode" ||
    name === "exitplanmode" ||
    name === "switch_mode"
  )
    return <ListTodoIcon className={ICON_CLASS} />
  if (name === "attempt_completion")
    return <CircleCheckIcon className={ICON_CLASS} />
  return undefined
}

// ── title derivation ──────────────────────────────────────────────────

function deriveToolTitle(
  toolName: string,
  input: string | null,
  output?: string | null
): string | null {
  const name = toolName.toLowerCase()
  const titleSource = input ?? output ?? null
  if (!titleSource) return null
  const parsedInput = input ? tryParseJson(input) : null
  const parsedOutput = output ? tryParseJson(output) : null
  const parsed = parsedInput ?? parsedOutput

  const getField = (key: string): string | null => {
    const nested = findStringFieldDeep(parsed, key)
    if (nested) return nested
    if (input) {
      const fromInput = extractJsonField(input, key)
      if (fromInput) return fromInput
    }
    if (output) {
      const fromOutput = extractJsonField(output, key)
      if (fromOutput) return fromOutput
    }
    return null
  }

  // Cline: attempt_completion — show result summary as title
  if (name === "attempt_completion") {
    const result = getField("result")
    if (result) {
      const firstLine = result.split("\n")[0].trim()
      return `${ellipsis(firstLine, 80)}`
    }
    return "Completion"
  }

  // File-based tools
  const filePath =
    getField("file_path") ??
    getField("filePath") ??
    getField("target_file") ??
    getField("targetFile") ??
    getField("filename") ??
    getField("path") ??
    getField("notebook_path")
  if (filePath) {
    const sp = shortPath(filePath)
    if (name === "read" || name === "read file") return `Read ${sp}`
    if (name === "edit") return `Edit ${sp}`
    if (name === "write") return `Write ${sp}`
    if (name === "notebookedit") return `NotebookEdit ${sp}`
  }

  // Command tools
  if (name === "bash" || name === "exec_command") {
    const description = getField("description")
    if (description) {
      return ellipsis(description, 80)
    }
    const direct = getField("command") ?? getField("cmd") ?? getField("script")
    const parsedCommand = commandFromUnknownValue(parsed)
    const fallback = extractCommandFromUnknownInput(titleSource)
    const command = direct ?? parsedCommand ?? fallback
    if (command) {
      return ellipsis(simplifyShellCommand(command).split("\n")[0], 80)
    }
    return null
  }

  if (name === "apply_patch") {
    const files = parseApplyPatchFilesFromUnknownInput(titleSource, parsed)
    if (files.length === 0) return "Edit"
    if (files.length === 1) {
      const file = files[0]
      const targetPath =
        file.op === "move" && file.to
          ? file.to
          : (file.from ?? file.to ?? file.path)
      return `Edit ${shortPath(targetPath)}`
    }
    return `Edit (${files.length} files)`
  }

  if (name === "edit") {
    const patchFiles = parseApplyPatchFilesFromUnknownInput(titleSource, parsed)
    if (patchFiles.length === 1) {
      const file = patchFiles[0]
      const targetPath =
        file.op === "move" && file.to
          ? file.to
          : (file.from ?? file.to ?? file.path)
      return `Edit ${shortPath(targetPath)}`
    }
    if (patchFiles.length > 1) return `Edit (${patchFiles.length} files)`

    const changedPaths = extractEditPathsFromChangesPayload(titleSource, parsed)
    if (changedPaths.length === 1) return `Edit ${shortPath(changedPaths[0])}`
    if (changedPaths.length > 1) return `Edit (${changedPaths.length} files)`

    const diffPath = extractPathFromDiffText(output)
    if (diffPath) return `Edit ${shortPath(diffPath)}`
    return "Edit"
  }

  // Command-like fallback: if input looks like a shell command payload,
  // keep title behavior consistent with historical command tool rendering.
  const commandLike =
    (parsed ? commandFromUnknownValue(parsed) : null) ??
    extractCommandFromUnknownInput(titleSource)
  if (commandLike && commandLike.trim().length > 0) {
    return ellipsis(simplifyShellCommand(commandLike).split("\n")[0], 80)
  }

  // Search tools
  if (name === "glob") {
    const p = getField("pattern")
    if (p) return `Glob ${p}`
  }
  if (name === "grep") {
    const p = getField("pattern")
    if (p) return `Grep ${ellipsis(p, 50)}`
  }

  // Task / agent tools
  if (name === "task") {
    const subagent = getField("subagent_type")
    const desc = getField("description")
    const prefix = subagent ? `${subagent}: ` : ""
    if (desc) return `${prefix}${ellipsis(desc, 60 - prefix.length)}`
    if (subagent) return subagent
  }
  if (name === "agent") {
    const subagent = getField("subagent_type")
    const desc = getField("description")
    const prefix = subagent ? `${subagent}: ` : ""
    if (desc) return `${prefix}${ellipsis(desc, 60 - prefix.length)}`
    if (subagent) return subagent
  }
  if (name === "taskcreate") {
    const subj = getField("subject")
    if (subj) return `TaskCreate: ${ellipsis(subj, 50)}`
  }
  if (name === "taskupdate") {
    const id = getField("taskId")
    const status = getField("status")
    if (id) return `TaskUpdate #${id}${status ? ` → ${status}` : ""}`
  }

  // Web tools
  if (name === "webfetch") {
    const url = getField("url")
    if (url) return `WebFetch ${ellipsis(url, 60)}`
  }
  if (name === "websearch") {
    const q = getField("query")
    if (q) return `WebSearch: ${ellipsis(q, 50)}`
  }

  // TodoWrite
  if (name === "todowrite") {
    if (parsed) {
      const todos = parsed.todos
      if (Array.isArray(todos)) {
        const count = todos.length
        const done = todos.filter(
          (t: Record<string, unknown>) => t.status === "completed"
        ).length
        return `Todos (${done}/${count})`
      }
    }
    return "TodoWrite"
  }

  // Skill
  if (name === "skill") {
    const sk = getField("skill")
    if (sk) return `Skill: ${sk}`
  }

  // EnterPlanMode / ExitPlanMode / SwitchMode
  if (
    name === "enterplanmode" ||
    name === "exitplanmode" ||
    name === "switch_mode"
  ) {
    const plan = getField("plan")
    if (plan) {
      const firstLine = plan
        .split("\n")
        .map((l) => l.replace(/^#+\s*/, "").trim())
        .find((l) => l.length > 0)
      if (firstLine) return `Plan · ${ellipsis(firstLine, 60)}`
    }
    const title = getField("title")
    if (title) return `Plan · ${title}`
    return "Plan"
  }

  // Generic: try to show the first string field as context
  if (parsed) {
    for (const [k, v] of Object.entries(parsed)) {
      if (isLikelyIdField(k)) {
        continue
      }
      if (typeof v === "string" && v.length > 0) {
        return `${toolName}: ${ellipsis(v, 50)}`
      }
    }
  }

  return null
}

function sanitizeLiveTitle(title: string | null | undefined): string | null {
  const trimmed = title?.trim()
  if (!trimmed) return null

  const callIdTitle = trimmed.match(
    /^[:：'"`“”‘’\s]*([a-z0-9_.-]+)(?:\s*[:：])?\s*call[\w-]*['"`“”‘’\s]*$/i
  )
  const source = callIdTitle?.[1] ?? trimmed
  const normalized = normalizeToolName(source)
  if (normalized === "apply_patch" || normalized === "edit") {
    return "Edit"
  }
  if (
    /\b(?:functions\.)?(?:edit|apply[_\s-]?patch)\b/i.test(trimmed) &&
    /\bcall[\w-]*\b/i.test(trimmed)
  ) {
    return "Edit"
  }
  if (normalized === "bash" || normalized === "exec_command") {
    return "Command"
  }
  return trimmed
}

function localizeDerivedToolTitle(
  title: string | null,
  t: (key: string, values?: Record<string, unknown>) => string
): string | null {
  if (!title) return null

  if (title === "Edit") return t("title.edit")
  if (title === "Command") return t("title.command")
  if (title === "TodoWrite") return t("title.todoWrite")
  if (title === "Read") return t("title.read")
  if (title === "Write") return t("title.write")
  if (title === "NotebookEdit") return t("title.notebookEdit")

  const editFilesMatch = title.match(/^Edit \((\d+) files\)$/)
  if (editFilesMatch) {
    return t("title.editFiles", { count: Number(editFilesMatch[1]) })
  }

  const editWithTarget = title.match(/^Edit (.+)$/)
  if (editWithTarget) {
    return t("title.editWithTarget", { target: editWithTarget[1] })
  }

  const readWithTarget = title.match(/^Read (.+)$/)
  if (readWithTarget) {
    return t("title.readWithTarget", { target: readWithTarget[1] })
  }

  const writeWithTarget = title.match(/^Write (.+)$/)
  if (writeWithTarget) {
    return t("title.writeWithTarget", { target: writeWithTarget[1] })
  }

  const notebookEditWithTarget = title.match(/^NotebookEdit (.+)$/)
  if (notebookEditWithTarget) {
    return t("title.notebookEditWithTarget", {
      target: notebookEditWithTarget[1],
    })
  }

  const globWithPattern = title.match(/^Glob (.+)$/)
  if (globWithPattern) {
    return t("title.globWithPattern", { pattern: globWithPattern[1] })
  }

  const grepWithPattern = title.match(/^Grep (.+)$/)
  if (grepWithPattern) {
    return t("title.grepWithPattern", { pattern: grepWithPattern[1] })
  }

  const taskCreateWithSubject = title.match(/^TaskCreate: (.+)$/)
  if (taskCreateWithSubject) {
    return t("title.taskCreateWithSubject", {
      subject: taskCreateWithSubject[1],
    })
  }

  const taskUpdateWithStatus = title.match(/^TaskUpdate #([^ ]+)(?: → (.+))?$/)
  if (taskUpdateWithStatus) {
    const id = taskUpdateWithStatus[1]
    const status = taskUpdateWithStatus[2]
    if (status) {
      return t("title.taskUpdateWithStatus", { id, status })
    }
    return t("title.taskUpdate", { id })
  }

  const webFetchWithUrl = title.match(/^WebFetch (.+)$/)
  if (webFetchWithUrl) {
    return t("title.webFetchWithUrl", { url: webFetchWithUrl[1] })
  }

  const webSearchWithQuery = title.match(/^WebSearch: (.+)$/)
  if (webSearchWithQuery) {
    return t("title.webSearchWithQuery", { query: webSearchWithQuery[1] })
  }

  const todosProgress = title.match(/^Todos \((\d+)\/(\d+)\)$/)
  if (todosProgress) {
    return t("title.todosProgress", {
      done: Number(todosProgress[1]),
      total: Number(todosProgress[2]),
    })
  }

  const skillWithName = title.match(/^Skill: (.+)$/)
  if (skillWithName) {
    return t("title.skillWithName", { name: skillWithName[1] })
  }

  const genericWithContext = title.match(/^([^:]+): (.+)$/)
  if (genericWithContext) {
    return t("title.genericWithContext", {
      tool: genericWithContext[1],
      context: genericWithContext[2],
    })
  }

  return title
}

// ── Specialized tool input renderers ─────────────────────────────────

/** Edit tool: file path + unified diff view */
function EditToolInput({ input }: { input: Record<string, unknown> }) {
  const filePath = str(input, "file_path")
  const oldString = str(input, "old_string") ?? ""
  const newString = str(input, "new_string") ?? ""
  const startLine = num(input, "_start_line")

  const diffCode = useMemo(() => {
    const diff = generateUnifiedDiff(
      oldString,
      newString,
      filePath ?? undefined
    )
    if (!diff || !startLine || startLine <= 1) return diff ?? ""
    // Replace line numbers in hunk headers with real start line
    return diff.replace(
      /^@@ -(\d+),(\d+) \+(\d+),(\d+) @@/gm,
      (_, _o, oc, _n, nc) => `@@ -${startLine},${oc} +${startLine},${nc} @@`
    )
  }, [oldString, newString, filePath, startLine])

  return diffCode ? (
    <UnifiedDiffPreview diffText={diffCode} clickableFilePath />
  ) : null
}

/** Edit tool (changes payload): combined diff view */
function EditChangesToolInput({ changes }: { changes: EditChangePreview[] }) {
  const diffCode = useMemo(() => {
    const diffParts: string[] = []

    for (const change of changes) {
      if (change.unifiedDiff && change.unifiedDiff.trim().length > 0) {
        diffParts.push(change.unifiedDiff.trim())
        diffParts.push("")
        continue
      }

      const generated = generateUnifiedDiff(
        change.oldText,
        change.newText,
        change.path
      )
      if (generated) {
        diffParts.push(generated)
        diffParts.push("")
      }
    }

    return diffParts.join("\n").trim()
  }, [changes])

  return diffCode ? (
    <UnifiedDiffPreview diffText={diffCode} clickableFilePath />
  ) : null
}

/** Bash / exec_command: terminal-style command display */
function BashToolInput({ input }: { input: Record<string, unknown> }) {
  const t = useTranslations("Folder.chat.contentParts")
  const command =
    commandFromUnknownValue(input) ??
    str(input, "command") ??
    str(input, "cmd") ??
    str(input, "script")
  const description = str(input, "description")
  const timeout = num(input, "timeout")
  const background = input.run_in_background === true
  const displayCommand = command ? simplifyShellCommand(command) : null

  return (
    <div className="space-y-2">
      {description && (
        <div className="flex items-center gap-2 text-xs">
          <TerminalIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="text-muted-foreground">{description}</span>
        </div>
      )}
      {displayCommand && <CodeBlock code={displayCommand} language="bash" />}
      {(timeout || background) && (
        <div className="flex flex-wrap gap-3 text-xs text-muted-foreground">
          {timeout && <span>{t("timeoutMs", { timeout })}</span>}
          {background && <span>{t("backgroundTrue")}</span>}
        </div>
      )}
    </div>
  )
}

/**
 * Parse structured read output from backend: `{"start_line":N,"content":"..."}`.
 * Falls back to raw text with startLine=1 if not structured.
 */
function parseReadOutput(raw: string): { startLine: number; content: string } {
  try {
    const parsed = JSON.parse(raw)
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      typeof parsed.start_line === "number" &&
      typeof parsed.content === "string"
    ) {
      return { startLine: parsed.start_line, content: parsed.content }
    }
  } catch {
    // not JSON
  }
  return { startLine: 1, content: raw }
}

/** Lightweight file content viewer with line numbers */
function FileContentLines({
  content,
  startLine = 1,
  highlight,
}: {
  content: string
  startLine?: number
  /** "added" tints every line green to indicate new content (e.g. Write tool). */
  highlight?: "added"
}) {
  const lines = useMemo(() => content.split("\n"), [content])
  const rowClass =
    highlight === "added"
      ? "flex bg-green-500/10 text-green-900 dark:text-green-300"
      : "flex"

  return (
    <div className="inline-block min-w-full font-mono text-[12px] leading-[20px]">
      {lines.map((line, i) => (
        <div key={i} className={rowClass}>
          <span className="w-[3.5rem] shrink-0 select-none pr-1 text-right text-muted-foreground/40">
            {startLine + i}
          </span>
          <span className="flex-1 whitespace-pre pr-3">{line}</span>
        </div>
      ))}
    </div>
  )
}

/** Read / Write / NotebookEdit: file-focused display */
function FileToolInput({
  toolName,
  input,
  output,
}: {
  toolName: string
  input: Record<string, unknown>
  output?: string | null
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const name = toolName.toLowerCase()
  const filePath =
    str(input, "file_path") ?? str(input, "path") ?? str(input, "notebook_path")
  const content = str(input, "content")
  const newSource = str(input, "new_source")
  const offset = num(input, "offset")
  const limit = num(input, "limit")
  const pages = str(input, "pages")
  const cellType = str(input, "cell_type")
  const editMode = str(input, "edit_mode")
  const isRead = name === "read" || name === "read file"

  const badges: string[] = []
  if (offset != null) badges.push(t("offset", { offset }))
  if (limit != null) badges.push(t("limit", { limit }))
  if (pages) badges.push(t("pages", { pages }))
  if (editMode) badges.push(t("mode", { mode: editMode }))
  if (cellType) badges.push(t("cell", { cell: cellType }))

  const { displayContent, startLine } = useMemo(() => {
    if (isRead && output) {
      const parsed = parseReadOutput(output)
      return { displayContent: parsed.content, startLine: parsed.startLine }
    }
    return {
      displayContent: content ?? newSource ?? null,
      startLine: 1,
    }
  }, [isRead, output, content, newSource])

  return (
    <section className="flex max-h-[420px] flex-col rounded-lg border border-border bg-background">
      <header className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/40 px-3 py-2 text-[11px]">
        <span className="shrink-0 rounded border border-border bg-background px-1.5 py-0.5 text-[10px] text-muted-foreground">
          {isRead ? "READ" : "WRITE"}
        </span>
        {filePath ? (
          <FilePathLink
            filePath={filePath}
            className="min-w-0 flex-1 font-mono text-foreground"
          >
            {filePath}
          </FilePathLink>
        ) : (
          <span className="min-w-0 flex-1 truncate font-mono text-foreground">
            {t("unknown")}
          </span>
        )}
        {badges.length > 0 && (
          <span className="ml-auto inline-flex shrink-0 items-center gap-2 text-[10px] text-muted-foreground">
            {badges.map((b) => (
              <span key={b}>{b}</span>
            ))}
          </span>
        )}
      </header>
      {displayContent && (
        <div className="overflow-auto">
          <FileContentLines
            content={displayContent}
            startLine={startLine}
            highlight={isRead ? undefined : "added"}
          />
        </div>
      )}
    </section>
  )
}

/** Glob / Grep: search-focused display */
function SearchToolInput({
  toolName,
  input,
}: {
  toolName: string
  input: Record<string, unknown>
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const name = toolName.toLowerCase()
  const pattern = str(input, "pattern")
  const path = str(input, "path")
  const glob = str(input, "glob")
  const outputMode = str(input, "output_mode")
  const fileType = str(input, "type")
  const caseInsensitive = input["-i"] === true
  const multiline = input.multiline === true

  return (
    <div className="space-y-2">
      {pattern && (
        <div className="flex items-center gap-2 rounded-md bg-muted/50 px-3 py-2">
          <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <code className="break-all text-xs text-foreground">{pattern}</code>
        </div>
      )}
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        {path && (
          <span>
            {t("pathLabel")}{" "}
            <span className="font-mono text-foreground">{path}</span>
          </span>
        )}
        {glob && (
          <span>
            {t("globLabel")}{" "}
            <span className="font-mono text-foreground">{glob}</span>
          </span>
        )}
        {fileType && (
          <span>
            {t("typeLabel")}{" "}
            <span className="font-mono text-foreground">{fileType}</span>
          </span>
        )}
        {name === "grep" && outputMode && (
          <span>
            {t("outputLabel")}{" "}
            <span className="font-mono text-foreground">{outputMode}</span>
          </span>
        )}
        {caseInsensitive && <span>{t("caseInsensitive")}</span>}
        {multiline && <span>{t("multiline")}</span>}
      </div>
    </div>
  )
}

/** Web tools: URL / query focused */
function WebToolInput({
  toolName,
  input,
}: {
  toolName: string
  input: Record<string, unknown>
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const name = toolName.toLowerCase()
  const url = str(input, "url")
  const query = str(input, "query")
  const prompt = str(input, "prompt")

  return (
    <div className="space-y-2">
      {name === "websearch" && query && (
        <div className="flex items-center gap-2 rounded-md bg-muted/50 px-3 py-2">
          <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="break-all text-xs font-medium text-foreground">
            {query}
          </span>
        </div>
      )}
      {name === "webfetch" && url && (
        <div className="flex items-center gap-2 rounded-md bg-muted/50 px-3 py-2">
          <GlobeIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="break-all font-mono text-xs text-foreground">
            {url}
          </span>
        </div>
      )}
      {prompt && (
        <div className="space-y-1">
          <span className="text-xs font-medium text-muted-foreground">
            {t("promptLabel")}
          </span>
          <div className="rounded-md bg-muted/50 p-3 text-xs prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
            <MessageResponse>{prompt}</MessageResponse>
          </div>
        </div>
      )}
    </div>
  )
}

/** Task tools: description / subject focused */
function TaskToolInput({ input }: { input: Record<string, unknown> }) {
  const t = useTranslations("Folder.chat.contentParts")
  const subject = str(input, "subject")
  const taskId = str(input, "taskId")
  const status = str(input, "status")
  const agentName = str(input, "name")

  const hasFields = subject || taskId || agentName
  if (!hasFields) return null

  return (
    <div className="space-y-2">
      {subject && (
        <div className="flex items-baseline gap-2 text-xs">
          <span className="shrink-0 font-medium text-muted-foreground">
            {t("subjectLabel")}
          </span>
          <span className="text-foreground">{subject}</span>
        </div>
      )}
      {taskId && (
        <div className="flex items-baseline gap-2 text-xs">
          <span className="shrink-0 font-medium text-muted-foreground">
            {t("taskLabel")}
          </span>
          <span className="font-mono text-foreground">
            #{taskId}
            {status ? ` → ${status}` : ""}
          </span>
        </div>
      )}
      {agentName && (
        <div className="text-xs text-muted-foreground">
          {t("nameLabel")}{" "}
          <span className="font-mono text-foreground">{agentName}</span>
        </div>
      )}
    </div>
  )
}

/** TodoWrite: checklist-style display */
function TodoWriteToolInput({ input }: { input: Record<string, unknown> }) {
  const todos = Array.isArray(input.todos) ? input.todos : []

  if (todos.length === 0) return null

  const statusIcon = (status: string) => {
    if (status === "completed")
      return <CircleCheckIcon className="size-3.5 shrink-0 text-green-500" />
    if (status === "in_progress")
      return <CircleDotIcon className="size-3.5 shrink-0 text-blue-500" />
    return <CircleIcon className="size-3.5 shrink-0 text-muted-foreground" />
  }

  const priorityBadge = (priority: string) => {
    const colors: Record<string, string> = {
      high: "bg-red-500/15 text-red-500",
      medium: "bg-yellow-500/15 text-yellow-600",
      low: "bg-muted text-muted-foreground",
    }
    return (
      <span
        className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${colors[priority] ?? colors.low}`}
      >
        {priority}
      </span>
    )
  }

  return (
    <div className="space-y-1">
      {todos.map((todo: Record<string, unknown>, i: number) => {
        const id = str(todo, "id") ?? String(i + 1)
        const content = str(todo, "content") ?? ""
        const status = str(todo, "status") ?? "pending"
        const priority = str(todo, "priority")

        return (
          <div
            key={id}
            className="flex items-start gap-2 rounded-md px-2 py-1.5 text-xs"
          >
            {statusIcon(status)}
            <span
              className={
                status === "completed"
                  ? "text-muted-foreground line-through"
                  : "text-foreground"
              }
            >
              {content}
            </span>
            {priority && priorityBadge(priority)}
          </div>
        )
      })}
    </div>
  )
}

function ApplyPatchToolInput({ input }: { input: string }) {
  return <UnifiedDiffPreview diffText={input} clickableFilePath />
}

// ── Switch mode (plan) input ──────────────────────────────────────────

function extractPlanMarkdown(input: Record<string, unknown>): string | null {
  const direct = input.plan ?? input.Plan
  if (typeof direct === "string" && direct.trim().length > 0) return direct

  const nested =
    typeof input.rawInput === "object" && input.rawInput !== null
      ? (input.rawInput as Record<string, unknown>)
      : typeof input.raw_input === "object" && input.raw_input !== null
        ? (input.raw_input as Record<string, unknown>)
        : null
  if (nested) {
    const nestedPlan = nested.plan ?? nested.Plan
    if (typeof nestedPlan === "string" && nestedPlan.trim().length > 0) {
      return nestedPlan
    }
  }

  return null
}

function SwitchModeToolInput({ input }: { input: Record<string, unknown> }) {
  const planMarkdown = extractPlanMarkdown(input)
  if (!planMarkdown) return null

  return (
    <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
      <MessageResponse>{planMarkdown}</MessageResponse>
    </div>
  )
}

// ── Generic structured input (fallback) ──────────────────────────────

/** Fields that typically contain code / long text → render in code blocks */
const CODE_FIELDS = new Set([
  "command",
  "old_string",
  "new_string",
  "content",
  "new_source",
  "prompt",
])

/** Fields to hide */
const HIDDEN_FIELDS = new Set(["dangerouslyDisableSandbox"])

function GenericToolInput({ input }: { input: string }) {
  const t = useTranslations("Folder.chat.contentParts")
  const parsed = useMemo(() => tryParseJson(input), [input])

  if (!parsed) {
    return (
      <pre className="whitespace-pre-wrap break-all rounded-md bg-muted/50 p-3 text-xs text-muted-foreground">
        {input}
      </pre>
    )
  }

  const entries = Object.entries(parsed).filter(([k]) => !HIDDEN_FIELDS.has(k))

  if (entries.length === 0) return null

  return (
    <div className="space-y-3">
      {entries.map(([key, value]) => {
        const labelKey = fieldLabelKey(key)
        const label = labelKey ? t(labelKey) : key

        if (CODE_FIELDS.has(key) && typeof value === "string") {
          const lang =
            key === "command"
              ? ("bash" as const)
              : key === "prompt"
                ? ("log" as const)
                : ("log" as const)
          return (
            <FieldBlock key={key} label={label}>
              <CodeBlock code={value} language={lang} />
            </FieldBlock>
          )
        }

        if (typeof value === "string") {
          if (value.length > 200) {
            return (
              <FieldBlock key={key} label={label}>
                <pre className="whitespace-pre-wrap break-all rounded-md bg-muted/50 p-3 text-xs">
                  {value}
                </pre>
              </FieldBlock>
            )
          }
          return <FieldInline key={key} label={label} value={value} />
        }

        if (typeof value === "number" || typeof value === "boolean") {
          return <FieldInline key={key} label={label} value={String(value)} />
        }

        if (value !== null && value !== undefined) {
          return (
            <FieldBlock key={key} label={label}>
              <CodeBlock
                code={JSON.stringify(value, null, 2)}
                language="json"
              />
            </FieldBlock>
          )
        }

        return null
      })}
    </div>
  )
}

// ── Dispatcher ───────────────────────────────────────────────────────

function isTruncatedInput(input: string): boolean {
  return input.endsWith('..."') || input.endsWith("...")
}

function StructuredToolInput({
  toolName,
  input,
  output,
}: {
  toolName: string
  input: string
  output?: string | null
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const name = toolName.toLowerCase()
  const parsed = useMemo(() => tryParseJson(input), [input])
  const truncated =
    (name === "edit" || name === "write" || name === "apply_patch") &&
    isTruncatedInput(input)

  const truncationBanner = truncated ? (
    <div className="rounded-md bg-yellow-500/10 px-2.5 py-1.5 text-[11px] text-yellow-700 dark:text-yellow-400">
      {t("inputTruncated")}
    </div>
  ) : null

  if (name === "apply_patch") {
    const patchInput =
      extractApplyPatchTextFromUnknownInput(input, parsed) ?? input
    return (
      <>
        {truncationBanner}
        <ApplyPatchToolInput input={patchInput} />
      </>
    )
  }

  if (name === "bash" || name === "exec_command") {
    if (parsed) {
      return <BashToolInput input={parsed} />
    }
    const plainCommand = extractCommandFromUnknownInput(input)
    if (plainCommand) {
      return <BashToolInput input={{ command: plainCommand }} />
    }
  }

  if (!parsed) {
    return (
      <pre className="whitespace-pre-wrap break-all rounded-md bg-muted/50 p-3 text-xs text-muted-foreground">
        {input}
      </pre>
    )
  }

  if (name === "edit") {
    const patchInput = extractApplyPatchTextFromUnknownInput(input, parsed)
    if (patchInput) {
      return (
        <>
          {truncationBanner}
          <ApplyPatchToolInput input={patchInput} />
        </>
      )
    }
    if (parsed) {
      const changesPayload = extractEditChangesPayload(parsed)
      if (changesPayload.length > 0) {
        return (
          <>
            {truncationBanner}
            <EditChangesToolInput changes={changesPayload} />
          </>
        )
      }
    }
    // Prefer tool output if it contains a structured diff with real line numbers
    // (injected by backend from toolUseResult.structuredPatch)
    if (output && typeof output === "string" && /^@@ /m.test(output)) {
      return (
        <>
          {truncationBanner}
          <UnifiedDiffPreview diffText={output} clickableFilePath />
        </>
      )
    }
    if (isCanonicalEditPayload(parsed)) {
      return (
        <>
          {truncationBanner}
          <EditToolInput input={parsed} />
        </>
      )
    }
    return <GenericToolInput input={input} />
  }
  if (name === "bash" || name === "exec_command")
    return <BashToolInput input={parsed} />
  if (
    name === "read" ||
    name === "read file" ||
    name === "write" ||
    name === "notebookedit"
  )
    return <FileToolInput toolName={toolName} input={parsed} output={output} />
  if (name === "glob" || name === "grep")
    return <SearchToolInput toolName={toolName} input={parsed} />
  if (name === "webfetch" || name === "websearch")
    return <WebToolInput toolName={toolName} input={parsed} />
  if (name === "todowrite") return <TodoWriteToolInput input={parsed} />
  if (
    name === "task" ||
    name === "taskcreate" ||
    name === "taskupdate" ||
    name === "tasklist"
  )
    return <TaskToolInput input={parsed} />
  if (
    name === "switch_mode" ||
    name === "enterplanmode" ||
    name === "exitplanmode"
  ) {
    if (extractPlanMarkdown(parsed)) {
      return <SwitchModeToolInput input={parsed} />
    }
  }

  return <GenericToolInput input={input} />
}

// ── Shared field components ──────────────────────────────────────────

function FieldInline({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2 text-xs">
      <span className="shrink-0 font-medium text-muted-foreground">
        {label}
      </span>
      <span className="break-all font-mono text-foreground">{value}</span>
    </div>
  )
}

function FieldBlock({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <div className="space-y-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      <div className="rounded-md bg-muted/50">{children}</div>
    </div>
  )
}

const FIELD_LABEL_KEYS = {
  file_path: "field.file",
  notebook_path: "field.notebook",
  command: "field.command",
  cmd: "field.command",
  old_string: "field.old",
  new_string: "field.new",
  pattern: "field.pattern",
  path: "field.path",
  query: "field.query",
  url: "field.url",
  description: "field.description",
  content: "field.content",
  new_source: "field.source",
  prompt: "field.prompt",
  subject: "field.subject",
  taskId: "field.taskId",
  status: "field.status",
  skill: "field.skill",
  args: "field.args",
  offset: "field.offset",
  limit: "field.limit",
  glob: "field.glob",
  type: "field.type",
  output_mode: "field.output",
  replace_all: "field.replaceAll",
  language: "field.language",
  timeout: "field.timeout",
  run_in_background: "field.background",
  subagent_type: "field.agentType",
  libraryName: "field.library",
  libraryId: "field.libraryId",
} as const

function fieldLabelKey(
  key: string
): (typeof FIELD_LABEL_KEYS)[keyof typeof FIELD_LABEL_KEYS] | null {
  const translationKey = FIELD_LABEL_KEYS[key as keyof typeof FIELD_LABEL_KEYS]
  return translationKey ?? null
}

function commandOutputFromJsonString(output: string): string | null {
  try {
    const parsed: unknown = JSON.parse(output)
    if (typeof parsed === "string") {
      return parsed
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return null
    }

    const obj = parsed as Record<string, unknown>
    const isCommandEnvelope =
      "command" in obj ||
      "parsed_cmd" in obj ||
      "cwd" in obj ||
      "exit_code" in obj ||
      "stdout" in obj ||
      "stderr" in obj ||
      "formatted_output" in obj ||
      "aggregated_output" in obj
    // Prefer raw stdout/stderr when present (more likely to preserve ANSI colors).
    const stdout = typeof obj.stdout === "string" ? obj.stdout : ""
    const stderr = typeof obj.stderr === "string" ? obj.stderr : ""
    if (stdout.length > 0 || stderr.length > 0) {
      if (stdout.length > 0 && stderr.length > 0) {
        return `${stdout}\n[stderr]\n${stderr}`
      }
      return stdout || stderr
    }

    const preferredKeys = [
      "formatted_output",
      "aggregated_output",
      "output",
      "text",
      "result",
    ]
    for (const key of preferredKeys) {
      const value = obj[key]
      if (typeof value === "string" && value.length > 0) {
        return value
      }
    }

    // Some command results are metadata-only envelopes (command/cwd/exit_code).
    // Returning empty string avoids rendering raw JSON as terminal output.
    if (isCommandEnvelope) {
      return ""
    }

    return null
  } catch {
    return null
  }
}

function stripMarkdownCodeFence(text: string): string {
  let result = text
  // Remove leading fenced-code line like ```sh / ```bash / ```
  result = result.replace(/^\s*```[\w-]*\s*\n?/, "")
  // Remove trailing closing fence if present
  result = result.replace(/\n?\s*```\s*$/, "")
  return result
}

/** Regex matching metadata lines in CLI execution output envelopes. */
const CLI_META_LINE_RE =
  /^(exit code\s*[:=]|wall time\s*[:=]|chunk id\s*[:=]|original token count\s*[:=]|total output lines\s*[:=]|process exited with code\s)/i

/**
 * Parse a CLI execution envelope, stripping all metadata and the "Output:"
 * separator, returning only the actual command output and the wall time.
 *
 * Handles formats like:
 *   Chunk ID: 065b2b
 *   Wall time: 0.05s
 *   Process exited with code 0
 *   Original token count: 27006
 *   Output:
 *   Total output lines: 1134
 *   <actual output here>
 */
function parseCliExecutionEnvelope(text: string): {
  output: string
  wallTime: string | null
} {
  const lines = text.split("\n")
  let wallTime: string | null = null

  // Look for "Output:" separator and extract wall time from header
  let outputSepIndex = -1
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim()
    const wtMatch = trimmed.match(/^wall time\s*:\s*(.+)/i)
    if (wtMatch) wallTime = wtMatch[1].trim()
    if (/^output:\s*$/i.test(trimmed)) {
      outputSepIndex = i
      break
    }
    // Stop scanning if we hit a non-metadata, non-blank line (actual content)
    if (!CLI_META_LINE_RE.test(trimmed) && trimmed.length > 0) break
  }

  // If "Output:" separator found, skip everything before it plus any
  // remaining metadata/blank lines after it
  if (outputSepIndex >= 0) {
    let start = outputSepIndex + 1
    while (start < lines.length) {
      const trimmed = lines[start].trim()
      if (CLI_META_LINE_RE.test(trimmed) || trimmed.length === 0) {
        start++
        continue
      }
      break
    }
    return { output: lines.slice(start).join("\n"), wallTime }
  }

  // No "Output:" separator — strip leading metadata lines
  let index = 0
  let sawMeta = false
  while (index < lines.length) {
    const trimmed = lines[index].trim()
    if (CLI_META_LINE_RE.test(trimmed)) {
      sawMeta = true
      if (!wallTime) {
        const wtMatch = trimmed.match(/^wall time\s*:\s*(.+)/i)
        if (wtMatch) wallTime = wtMatch[1].trim()
      }
      index++
      continue
    }
    if (sawMeta && trimmed.length === 0) {
      index++
      continue
    }
    break
  }

  if (!sawMeta) return { output: text, wallTime: null }

  while (index < lines.length && lines[index].trim().length === 0) index++
  return { output: lines.slice(index).join("\n"), wallTime }
}

// ── Part components ───────────────────────────────────────────────────

const TextPart = memo(function TextPart({
  text,
  softBreaks = false,
}: {
  text: string
  // User-authored text opts in so single newlines survive as line breaks
  // (chat input commonly relies on them) while Markdown still renders.
  softBreaks?: boolean
}) {
  return (
    <div className='break-words text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside [&_[data-streamdown="code-block-body"]]:max-h-96 [&_[data-streamdown="code-block-body"]]:overflow-auto'>
      <MessageResponse softBreaks={softBreaks}>{text}</MessageResponse>
    </div>
  )
})

const ToolCallPart = memo(function ToolCallPart({
  part,
}: {
  part: Extract<AdaptedContentPart, { type: "tool-call" }>
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const [manualOpen, setManualOpen] = useState(false)
  const normalizedToolName = useMemo(
    () => normalizeToolName(part.toolName),
    [part.toolName]
  )
  const toolNameLower = normalizedToolName.toLowerCase()
  const isCommandTool =
    toolNameLower === "bash" || toolNameLower === "exec_command"
  const isCommandLikeTool = isCommandTool || toolNameLower === "apply_patch"
  const isRunning =
    part.state === "input-available" || part.state === "input-streaming"
  const title = useMemo(() => {
    const rawTitle =
      deriveToolTitle(
        normalizedToolName,
        part.input,
        part.output ?? part.errorText ?? null
      ) ??
      sanitizeLiveTitle(part.displayTitle) ??
      null
    return localizeDerivedToolTitle(rawTitle, ((key, values) =>
      t(key as never, values as never)) as (
      key: string,
      values?: Record<string, unknown>
    ) => string)
  }, [
    normalizedToolName,
    part.input,
    part.output,
    part.errorText,
    part.displayTitle,
    t,
  ])
  const lineChangeStats = useMemo(() => {
    if (toolNameLower !== "edit" && toolNameLower !== "apply_patch") {
      return null
    }

    // Prefer finalized tool output, then the declared input.
    // Keep error text as last fallback because permission wrappers can include
    // verbose envelopes that inflate +/- counts before approval.
    const prioritizedCandidates = [
      part.output ?? null,
      part.input,
      part.errorText ?? null,
    ]
    for (const candidate of prioritizedCandidates) {
      const stats = extractEditLineChangeStats(candidate)
      if (!stats) continue
      return stats
    }
    return null
  }, [toolNameLower, part.input, part.output, part.errorText])
  const wallTime = useMemo(() => {
    const source = part.output ?? part.errorText
    if (!source) return null
    const normalized = commandOutputFromJsonString(source) ?? source
    const match = normalized.match(/^wall time\s*:\s*(.+)/im)
    if (!match) return null
    const raw = match[1].trim()
    // Parse "0.0519 seconds" → "52ms", "1.234 seconds" → "1.2s"
    const numMatch = raw.match(/^([\d.]+)\s*s/)
    if (!numMatch) return raw
    const sec = parseFloat(numMatch[1])
    if (Number.isNaN(sec)) return raw
    if (sec < 0.001) return "<1ms"
    if (sec < 1) return `${Math.round(sec * 1000)}ms`
    if (sec < 60) return `${sec.toFixed(1)}s`
    return `${(sec / 60).toFixed(1)}m`
  }, [part.output, part.errorText])
  const titleSuffix = useMemo(() => {
    const hasStats =
      lineChangeStats &&
      (lineChangeStats.additions > 0 || lineChangeStats.deletions > 0)
    if (!hasStats && !wallTime) return null

    return (
      <span className="flex items-center gap-1.5 text-xs font-medium">
        {hasStats && lineChangeStats.additions > 0 && (
          <span className="inline-flex items-center gap-0.5 text-green-600 dark:text-green-400">
            <PlusIcon className="size-3" />
            {lineChangeStats.additions}
          </span>
        )}
        {hasStats && lineChangeStats.deletions > 0 && (
          <span className="inline-flex items-center gap-0.5 text-red-600 dark:text-red-400">
            <MinusIcon className="size-3" />
            {lineChangeStats.deletions}
          </span>
        )}
        {wallTime && (
          <span className="text-muted-foreground/60 font-normal">
            {wallTime}
          </span>
        )}
      </span>
    )
  }, [lineChangeStats, wallTime])

  const icon = useMemo(
    () => getToolIcon(normalizedToolName, part.input),
    [normalizedToolName, part.input]
  )
  const displayCommand = useMemo(() => {
    if (!isCommandTool) return null
    return (
      extractDisplayCommandFromToolInput(part.input) ??
      extractDisplayCommandFromToolInput(part.output) ??
      extractDisplayCommandFromToolInput(part.errorText)
    )
  }, [isCommandTool, part.input, part.output, part.errorText])
  const commandOutput = useMemo(() => {
    if (!isCommandLikeTool) return null
    const source =
      typeof part.output === "string"
        ? part.output
        : typeof part.errorText === "string"
          ? part.errorText
          : null
    if (!source) return null
    const normalized = commandOutputFromJsonString(source) ?? source
    const envelope = parseCliExecutionEnvelope(normalized)
    return stripMarkdownCodeFence(envelope.output)
  }, [isCommandLikeTool, part.output, part.errorText])
  const hasLiveOutput =
    isRunning && isCommandTool && typeof commandOutput === "string"
  const liveOutput = useMemo(() => {
    if (!hasLiveOutput || typeof commandOutput !== "string") {
      return null
    }
    const maxChars = 24000
    return commandOutput.length > maxChars
      ? commandOutput.slice(-maxChars)
      : commandOutput
  }, [hasLiveOutput, commandOutput])
  const liveOutputTruncated =
    hasLiveOutput &&
    typeof commandOutput === "string" &&
    typeof liveOutput === "string" &&
    liveOutput.length < commandOutput.length
  const shouldRenderCommandTerminal =
    isCommandTool &&
    (isRunning ||
      (typeof commandOutput === "string" && commandOutput.length > 0) ||
      (typeof displayCommand === "string" && displayCommand.length > 0))
  const terminalOutput = useMemo(() => {
    if (!shouldRenderCommandTerminal) return ""
    const output = hasLiveOutput ? (liveOutput ?? "") : (commandOutput ?? "")
    return buildCommandTerminalOutput(displayCommand, output, isRunning)
  }, [
    shouldRenderCommandTerminal,
    hasLiveOutput,
    liveOutput,
    commandOutput,
    displayCommand,
    isRunning,
  ])
  const isFileTool =
    toolNameLower === "read" ||
    toolNameLower === "read file" ||
    toolNameLower === "write" ||
    toolNameLower === "notebookedit"
  const shouldHideDuplicateResult =
    (toolNameLower === "edit" ||
      toolNameLower === "apply_patch" ||
      toolNameLower === "switch_mode" ||
      toolNameLower === "enterplanmode" ||
      toolNameLower === "exitplanmode" ||
      isFileTool) &&
    !part.errorText
  // Agent/subagent tools get a dedicated container rendering
  if (toolNameLower === "agent") {
    return (
      <AgentToolCallPart
        part={part}
        renderToolCall={(p, key) => (
          // Strip agentStats to prevent recursive Agent nesting
          <ToolCallPart key={key} part={{ ...p, agentStats: undefined }} />
        )}
      />
    )
  }

  if (toolNameLower === "create_goal" || toolNameLower === "update_goal") {
    return <GoalToolCallPart part={{ ...part, toolName: normalizedToolName }} />
  }

  // Multi-agent delegation tool: surfaces an inline DelegatedSubThread
  // bound to the child sub-session via parent_tool_use_id. Matches the
  // bare `delegate_to_agent` (post-normalization) plus any host-specific
  // server-prefixed form (`mcp__<server>__delegate_to_agent`,
  // `<server>/delegate_to_agent`, `<server>.delegate_to_agent`, etc.)
  // as a defensive fallback in case the value reaches the renderer
  // un-normalized. Falls through to the normal renderer when no
  // toolCallId is available (snapshot replays without a live binding)
  // so the user still sees the tool input/output.
  if (isDelegateToAgentToolName(normalizedToolName) && part.toolCallId) {
    return (
      <DelegatedSubThread
        parentToolUseId={part.toolCallId}
        input={part.input ?? null}
        output={part.output ?? null}
        errorText={part.errorText ?? null}
        state={part.state}
        meta={part.meta ?? null}
      />
    )
  }

  // Multi-agent delegation companion tools: render compact status cards
  // consistent with DelegatedSubThread instead of the generic tool shell.
  // `normalizeToolName` has already collapsed any host-specific server prefix
  // (`mcp__<server>__…`) to these canonical names.
  if (toolNameLower === "get_delegation_status") {
    return (
      <DelegationStatusCard
        kind="status"
        input={part.input ?? null}
        output={part.output ?? null}
        errorText={part.errorText ?? null}
        state={part.state}
      />
    )
  }
  if (toolNameLower === "cancel_delegation") {
    return (
      <DelegationStatusCard
        kind="cancel"
        input={part.input ?? null}
        output={part.output ?? null}
        errorText={part.errorText ?? null}
        state={part.state}
      />
    )
  }

  // Cline: attempt_completion — render as an expanded card with result + progress
  if (toolNameLower === "attempt_completion") {
    const parsedCompletion = tryParseJson(part.input ?? "")
    const completionResult =
      (parsedCompletion?.result as string | undefined)?.trim() ?? null
    const taskProgress =
      (parsedCompletion?.task_progress as string | undefined)?.trim() ?? null
    return (
      <Tool open onOpenChange={setManualOpen}>
        <ToolHeader
          type="dynamic-tool"
          state={part.state}
          toolName={normalizedToolName}
          title={title ?? "Completion"}
          icon={icon}
        />
        <ToolContent>
          {completionResult && (
            <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
              <MessageResponse>{completionResult}</MessageResponse>
            </div>
          )}
          {taskProgress && (
            <div className="mt-2 rounded-md border bg-muted/30 px-3 py-2">
              <div className="text-[11px] font-medium text-muted-foreground mb-1">
                Progress
              </div>
              <div className="text-xs prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
                <MessageResponse>{taskProgress}</MessageResponse>
              </div>
            </div>
          )}
        </ToolContent>
      </Tool>
    )
  }

  const open = (isRunning && (isCommandTool || hasLiveOutput)) || manualOpen

  return (
    <Tool open={open} onOpenChange={setManualOpen}>
      <ToolHeader
        type="dynamic-tool"
        state={part.state}
        toolName={normalizedToolName}
        title={title ?? undefined}
        titleSuffix={titleSuffix ?? undefined}
        icon={icon}
      />
      <ToolContent>
        {part.input && (!isCommandTool || !shouldRenderCommandTerminal) && (
          <StructuredToolInput
            toolName={normalizedToolName}
            input={part.input}
            output={part.output}
          />
        )}
        {toolNameLower === "task" && part.output ? (
          <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
            <MessageResponse>{part.output}</MessageResponse>
          </div>
        ) : (
          <>
            {shouldRenderCommandTerminal ? (
              <div>
                <Terminal
                  output={terminalOutput}
                  isStreaming={isRunning}
                  className="max-h-80"
                />
                {liveOutputTruncated && (
                  <div className="text-[11px] text-muted-foreground">
                    {t("showingTailOutput")}
                  </div>
                )}
              </div>
            ) : (
              !shouldHideDuplicateResult &&
              (part.output || part.errorText) && (
                <ToolOutput output={part.output} errorText={part.errorText} />
              )
            )}
          </>
        )}
      </ToolContent>
    </Tool>
  )
})

const ToolResultPart = memo(function ToolResultPart({
  part,
}: {
  part: Extract<AdaptedContentPart, { type: "tool-result" }>
}) {
  const t = useTranslations("Folder.chat.contentParts")
  return (
    <Tool>
      <ToolHeader
        type="dynamic-tool"
        state={part.state}
        toolName={t("result")}
      />
      <ToolContent>
        <ToolOutput output={part.output} errorText={part.errorText} />
      </ToolContent>
    </Tool>
  )
})

const ReasoningPart = memo(function ReasoningPart({
  part,
}: {
  part: Extract<AdaptedContentPart, { type: "reasoning" }>
}) {
  const hasContent = part.content.trim().length > 0
  const expandable = hasContent || part.isStreaming
  return (
    <Reasoning isStreaming={part.isStreaming} expandable={expandable}>
      <ReasoningTrigger />
      {expandable && <ReasoningContent>{part.content}</ReasoningContent>}
    </Reasoning>
  )
})

const ToolGroupPart = memo(function ToolGroupPart({
  part,
}: {
  part: Extract<AdaptedContentPart, { type: "tool-group" }>
}) {
  const t = useTranslations("Folder.chat.contentParts.toolGroup")
  const [open, setOpen] = useState(false)

  const { phrases, errorPhrase } = useMemo(() => {
    const counts = TOOL_KIND_ORDER.reduce(
      (acc, kind) => {
        acc[kind] = 0
        return acc
      },
      {} as Record<ToolKindLabel, number>
    )
    let errors = 0
    for (const item of part.items) {
      counts[classifyToolKind(item.toolName)] += 1
      if (item.state === "output-error" || item.errorText) errors += 1
    }
    const built: string[] = []
    for (const kind of TOOL_KIND_ORDER) {
      const count = counts[kind]
      if (count <= 0) continue
      built.push(t(kind, { count }))
    }
    if (built.length === 0) {
      built.push(t("other", { count: part.items.length }))
    }
    return {
      phrases: built,
      errorPhrase: errors > 0 ? t("errorSuffix", { count: errors }) : null,
    }
  }, [part, t])

  if (part.items.length === 0) return null

  const joiner = t("joiner")
  const titleText = phrases.join(joiner)

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="w-full">
      <CollapsibleTrigger
        className={cn(
          "group inline-flex max-w-full items-center gap-1.5 rounded-full bg-muted/60 px-3.5 py-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        )}
      >
        <ChevronRightIcon
          aria-hidden="true"
          className={cn(
            "size-3 shrink-0 opacity-60 transition-transform",
            open && "rotate-90"
          )}
        />
        <span className="min-w-0 truncate">
          {part.isStreaming ? (
            <Shimmer as="span" duration={1} shineColor="var(--primary)">
              {titleText}
            </Shimmer>
          ) : (
            titleText
          )}
          {errorPhrase && (
            <span className="text-destructive">
              {joiner}
              {errorPhrase}
            </span>
          )}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent
        className={cn(
          "w-full outline-none",
          "data-[state=open]:animate-in data-[state=closed]:animate-out",
          "data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
          "data-[state=closed]:slide-out-to-top-1 data-[state=open]:slide-in-from-top-1"
        )}
      >
        <div className="mt-3 w-full space-y-3">
          {part.items.map((item, idx) => (
            <ToolCallPart
              key={`grouped-tc-${item.toolCallId ?? idx}-${idx}`}
              part={item}
            />
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
})

// ── Main renderer ─────────────────────────────────────────────────────

interface ContentPartsRendererProps {
  parts: AdaptedContentPart[]
  role?: MessageRole
}

export const ContentPartsRenderer = memo(function ContentPartsRenderer({
  parts,
  role,
}: ContentPartsRendererProps) {
  const renderPart = (part: AdaptedContentPart, keyId: string): ReactNode => {
    if (part.type === "text") {
      return (
        <TextPart
          key={`text-${keyId}`}
          text={part.text}
          softBreaks={role === "user"}
        />
      )
    }

    if (part.type === "tool-call") {
      return <ToolCallPart key={`tc-${part.toolCallId ?? keyId}`} part={part} />
    }

    if (part.type === "tool-group") {
      return <ToolGroupPart key={`tg-${keyId}`} part={part} />
    }

    if (part.type === "goal-run") {
      return (
        <GoalRunPart
          key={`goal-${keyId}`}
          part={part}
          renderPart={(child, childKey) => renderPart(child, childKey)}
        />
      )
    }

    if (part.type === "delegation-status-group") {
      return (
        <DelegationStatusGroupCard key={`dsg-${keyId}`} polls={part.polls} />
      )
    }

    if (part.type === "tool-result") {
      return (
        <ToolResultPart key={`tr-${part.toolCallId ?? keyId}`} part={part} />
      )
    }

    if (part.type === "reasoning") {
      return <ReasoningPart key={`reasoning-${keyId}`} part={part} />
    }

    if (part.type === "generated-image") {
      return (
        <GeneratedImagesBlock
          key={`gimg-${keyId}`}
          revisedPrompt={part.revisedPrompt}
          image={part.image}
          status={part.status}
        />
      )
    }

    return null
  }

  return (
    <div className="space-y-4">
      {parts.map((part, i) => renderPart(part, `${i}`))}
    </div>
  )
})
