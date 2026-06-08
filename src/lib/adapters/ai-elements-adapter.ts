import type {
  MessageTurn,
  ContentBlock,
  MessageRole,
  TurnUsage,
  AgentExecutionStats,
  ToolCallStatus,
} from "@/lib/types"
import {
  isAgentLikeToolName,
  isDelegationStatusToolName,
} from "@/lib/adapters/tool-kind-classifier"
import { normalizeToolName } from "@/lib/tool-call-normalization"

/**
 * Adapted content part types for AI SDK Elements components
 */
export type ToolCallState =
  | "input-streaming"
  | "input-available"
  | "output-available"
  | "output-error"

export type AdaptedToolCallPart = {
  type: "tool-call"
  toolCallId: string
  toolName: string
  displayTitle?: string | null
  input: string | null
  state: ToolCallState
  output?: string | null
  errorText?: string
  agentStats?: AgentExecutionStats | null
  /**
   * ACP extensibility metadata forwarded from `ContentBlock.tool_use.meta`.
   * Opaque pass-through; the only consumer today is `<DelegatedSubThread>`
   * which reads `meta["codeg.delegation"]` as a binding fallback when the
   * live DelegationContext entry is missing (page refresh, late mount).
   */
  meta?: Record<string, unknown> | null
}

/**
 * Inline rendering of codex-acp v0.14+ image generation. Mirrors the
 * `ContentBlock::ImageGeneration` data shape. Distinct from regular tool
 * calls so it never folds into a `tool-group` — each image stands alone
 * with its own labeled card. One image per part — multi-image turns are
 * already split into multiple consecutive blocks at the runtime layer.
 *
 * `status` lets the renderer distinguish "still generating" from "the call
 * failed without producing an image" when `image` is null. `null` means
 * the source didn't carry status (Rust JSONL replay) — by definition such
 * blocks always have an image, so the status is irrelevant there.
 */
export type AdaptedGeneratedImagePart = {
  type: "generated-image"
  revisedPrompt: string | null
  /** `null` while the agent has emitted the ToolCall but no image yet. */
  image: UserImageDisplay | null
  status: ToolCallStatus | null
}

export type AdaptedGoalRunPart = {
  type: "goal-run"
  start: AdaptedToolCallPart
  end: AdaptedToolCallPart | null
  items: AdaptedContentPart[]
  isRunning: boolean
}

export type AdaptedContentPart =
  | { type: "text"; text: string }
  | AdaptedToolCallPart
  | {
      type: "tool-result"
      toolCallId: string
      output: string | null
      errorText?: string
      state: "output-available" | "output-error"
    }
  | { type: "reasoning"; content: string; isStreaming: boolean }
  | {
      type: "tool-group"
      items: AdaptedToolCallPart[]
      isStreaming: boolean
    }
  /**
   * A run of consecutive `get_delegation_status` poll cards, merged into one
   * card. When a delegated task runs longer than the 60s status-wait cap, the
   * agent re-polls repeatedly; rather than stack N near-identical cards, the
   * renderer collapses the run and (grouping by `task_id`) shows the latest
   * poll per task — so parallel waits surface as one row each. Non-consecutive
   * polls are NOT merged (text / other tools break the run).
   */
  | {
      type: "delegation-status-group"
      polls: AdaptedToolCallPart[]
    }
  | AdaptedGoalRunPart
  | AdaptedGeneratedImagePart

export interface UserResourceDisplay {
  name: string
  uri: string
  mime_type?: string | null
}

export interface UserImageDisplay {
  name: string
  data: string
  mime_type: string
  uri?: string | null
}

const BLOCKED_RESOURCE_MENTION_RE = /@([^\s@]+)\s*\[blocked[^\]]*\]/gi
const MARKDOWN_LINK_RE = /\[([^\]]+)\]\(([^)]+)\)/g

/**
 * Adapted message format for AI SDK Elements
 */
export interface AdaptedMessage {
  id: string
  role: MessageRole
  content: AdaptedContentPart[]
  userResources?: UserResourceDisplay[]
  userImages?: UserImageDisplay[]
  timestamp: string
  usage?: TurnUsage | null
  duration_ms?: number | null
  model?: string | null
  /** Wall-clock completion time as ISO string (parsed once at the Rust layer). */
  completed_at?: string | null
}

export interface AdapterMessageText {
  attachedResources: string
  toolCallFailed: string
}

type InlineToolSegment =
  | { kind: "text"; value: string }
  | { kind: "tool_call" | "tool_result"; value: string }

const INLINE_TOOL_TAG_RE = /<(tool_call|tool_result)>\s*([\s\S]*?)\s*<\/\1>/gi
const GOAL_UPDATE_MARKER_RE = /Goal updated \(([^)]+)\):\s*/g

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null
  }
  return value as Record<string, unknown>
}

function toInlinePayloadString(value: unknown): string | null {
  if (value === null || value === undefined) return null
  if (typeof value === "string") {
    const trimmed = value.trim()
    return trimmed.length > 0 ? trimmed : null
  }
  try {
    return JSON.stringify(value)
  } catch {
    return String(value)
  }
}

function splitInlineToolSegments(text: string): InlineToolSegment[] | null {
  INLINE_TOOL_TAG_RE.lastIndex = 0
  const segments: InlineToolSegment[] = []
  let cursor = 0
  let foundTag = false

  for (const match of text.matchAll(INLINE_TOOL_TAG_RE)) {
    const full = match[0]
    const tag = match[1]
    const body = match[2]
    const start = match.index ?? -1
    if (start < 0) continue

    foundTag = true
    if (start > cursor) {
      segments.push({
        kind: "text",
        value: text.slice(cursor, start),
      })
    }

    if (tag === "tool_call" || tag === "tool_result") {
      segments.push({
        kind: tag,
        value: body ?? "",
      })
    }

    cursor = start + full.length
  }

  if (!foundTag) return null

  if (cursor < text.length) {
    segments.push({
      kind: "text",
      value: text.slice(cursor),
    })
  }

  return segments
}

function parseInlineToolCallPayload(payload: string): {
  toolName: string
  toolCallId: string | null
  input: string | null
} {
  const trimmed = payload.trim()
  if (trimmed.length === 0) {
    return { toolName: "tool", toolCallId: null, input: null }
  }

  try {
    const parsed: unknown = JSON.parse(trimmed)
    const obj = asRecord(parsed)
    if (!obj) {
      return {
        toolName: "tool",
        toolCallId: null,
        input: toInlinePayloadString(parsed),
      }
    }

    const nameCandidates = [
      obj.name,
      obj.tool_name,
      obj.tool,
      obj.kind,
      obj.type,
    ]
    const toolName =
      nameCandidates
        .find((value): value is string => typeof value === "string")
        ?.trim() || "tool"

    const idCandidates = [
      obj.id,
      obj.tool_call_id,
      obj.tool_use_id,
      obj.call_id,
      obj.callId,
    ]
    const toolCallId =
      idCandidates.find(
        (value): value is string => typeof value === "string"
      ) ?? null

    const directInput =
      obj.arguments ?? obj.input ?? obj.params ?? obj.payload ?? null
    if (directInput !== null) {
      return {
        toolName,
        toolCallId,
        input: toInlinePayloadString(directInput),
      }
    }

    const passthroughEntries = Object.entries(obj).filter(
      ([key]) =>
        ![
          "name",
          "tool_name",
          "tool",
          "kind",
          "type",
          "id",
          "tool_call_id",
          "tool_use_id",
          "call_id",
          "callId",
        ].includes(key)
    )
    const fallbackInput =
      passthroughEntries.length > 0
        ? Object.fromEntries(passthroughEntries)
        : null

    return {
      toolName,
      toolCallId,
      input: toInlinePayloadString(fallbackInput),
    }
  } catch {
    return {
      toolName: "tool",
      toolCallId: null,
      input: trimmed,
    }
  }
}

function parseInlineToolResultPayload(payload: string): {
  output: string | null
  isError: boolean
} {
  const trimmed = payload.trim()
  if (trimmed.length === 0) {
    return { output: null, isError: false }
  }

  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (typeof parsed === "string") {
      return { output: parsed, isError: false }
    }

    const obj = asRecord(parsed)
    if (!obj) {
      return { output: toInlinePayloadString(parsed), isError: false }
    }

    const isError =
      obj.is_error === true ||
      obj.error === true ||
      (typeof obj.status === "string" && obj.status.toLowerCase() === "error")

    const outputCandidates = [
      obj.output,
      obj.result,
      obj.text,
      obj.content,
      obj.stdout,
      obj.stderr,
      obj.message,
    ]
    const output = outputCandidates
      .map((value) => toInlinePayloadString(value))
      .find((value): value is string => typeof value === "string")

    return {
      output: output ?? toInlinePayloadString(parsed),
      isError,
    }
  } catch {
    return {
      output: trimmed,
      isError: false,
    }
  }
}

function expandInlineToolText(
  text: string,
  messageId: string,
  blockIndex: number,
  toolCallFailedText: string
): AdaptedContentPart[] | null {
  const segments = splitInlineToolSegments(text)
  if (!segments) return null

  const parts: AdaptedContentPart[] = []
  let inlineCounter = 0

  for (let index = 0; index < segments.length; index += 1) {
    const segment = segments[index]

    if (segment.kind === "text") {
      if (segment.value.trim().length > 0) {
        parts.push({
          type: "text",
          text: segment.value,
        })
      }
      continue
    }

    if (segment.kind === "tool_call") {
      const parsedCall = parseInlineToolCallPayload(segment.value)
      const fallbackId = `${messageId}-inline-tool-${blockIndex}-${inlineCounter}`
      const toolCallId = parsedCall.toolCallId ?? fallbackId

      let output: string | null = null
      let errorText: string | undefined
      let state: ToolCallState = "output-available"

      let lookahead = index + 1
      while (
        lookahead < segments.length &&
        segments[lookahead].kind === "text" &&
        segments[lookahead].value.trim().length === 0
      ) {
        lookahead += 1
      }

      if (
        lookahead < segments.length &&
        segments[lookahead].kind === "tool_result"
      ) {
        const parsedResult = parseInlineToolResultPayload(
          segments[lookahead].value
        )
        output = parsedResult.output
        if (parsedResult.isError) {
          state = "output-error"
          errorText = output ?? toolCallFailedText
        }
        index = lookahead
      }

      parts.push({
        type: "tool-call",
        toolCallId,
        toolName: parsedCall.toolName,
        input: parsedCall.input,
        state,
        output,
        errorText,
      })
      inlineCounter += 1
      continue
    }

    const parsedResult = parseInlineToolResultPayload(segment.value)
    const toolCallId = `${messageId}-inline-tool-result-${blockIndex}-${inlineCounter}`
    parts.push({
      type: "tool-result",
      toolCallId,
      output: parsedResult.output,
      errorText: parsedResult.isError
        ? (parsedResult.output ?? toolCallFailedText)
        : undefined,
      state: parsedResult.isError ? "output-error" : "output-available",
    })
    inlineCounter += 1
  }

  return parts
}

function normalizeGoalStatusText(status: string): string {
  return status
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, "_")
}

function createSyntheticGoalToolPart(
  status: string,
  objective: string,
  messageId: string,
  blockIndex: number,
  goalIndex: number
): AdaptedToolCallPart {
  const normalizedStatus = normalizeGoalStatusText(status)
  const isActive = normalizedStatus === "active"
  const goal = {
    objective,
    status: normalizedStatus,
  }

  return {
    type: "tool-call",
    toolCallId: `${messageId}-goal-${blockIndex}-${goalIndex}`,
    toolName: isActive ? "create_goal" : "update_goal",
    input: JSON.stringify(
      isActive ? { objective } : { status: normalizedStatus }
    ),
    state: "output-available",
    output: JSON.stringify({ goal }),
  }
}

const GOAL_TRAILING_PROSE_START_PATTERNS: RegExp[] = [
  /我(?:也|会|先|已经|将|再|接下来|现在|继续|顺手|把|已)/g,
  /已(?:完成|分析|读取|检查|修复|更新)/g,
  /\bI(?:'ll| will| also| have| just| checked| read| updated| fixed)\b/g,
]

function inferObjectiveFromGoalPayload(payload: string): string {
  const firstLine = payload.split(/\r?\n/)[0]?.trim() ?? ""
  if (firstLine.length === 0) return ""

  let proseStart = firstLine.length
  for (const pattern of GOAL_TRAILING_PROSE_START_PATTERNS) {
    pattern.lastIndex = 0
    for (const match of firstLine.matchAll(pattern)) {
      const index = match.index ?? -1
      if (index > 0 && index < proseStart) {
        proseStart = index
      }
    }
  }

  return firstLine.slice(0, proseStart).trim()
}

function expandGoalUpdateText(
  text: string,
  messageId: string,
  blockIndex: number,
  toolCallFailedText: string,
  objectiveHints: readonly string[] = []
): AdaptedContentPart[] | null {
  type GoalUpdateMarker = {
    start: number
    payloadStart: number
    payloadEnd: number
    status: string
    payload: string
  }

  GOAL_UPDATE_MARKER_RE.lastIndex = 0
  const markers: GoalUpdateMarker[] = []
  for (const match of text.matchAll(GOAL_UPDATE_MARKER_RE)) {
    const start = match.index ?? -1
    const status = match[1]?.trim() ?? ""
    if (start < 0 || status.length === 0) continue
    markers.push({
      start,
      payloadStart: start + match[0].length,
      payloadEnd: text.length,
      status,
      payload: "",
    })
  }

  if (markers.length === 0) return null

  for (let index = 0; index < markers.length; index += 1) {
    const next = markers[index + 1]
    markers[index].payloadEnd = next ? next.start : text.length
    markers[index].payload = text.slice(
      markers[index].payloadStart,
      markers[index].payloadEnd
    )
  }

  const payloadFirstLines = markers
    .map((marker) => marker.payload.replace(/^\s+/, "").split(/\r?\n/)[0])
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .sort((a, b) => a.length - b.length)
  const sharedObjective =
    markers.length > 1
      ? (payloadFirstLines.find((candidate) =>
          payloadFirstLines.every((line) => line.startsWith(candidate))
        ) ?? null)
      : null
  const sortedObjectiveHints = objectiveHints
    .map((hint) => hint.trim())
    .filter((hint) => hint.length > 0)
    .sort((a, b) => b.length - a.length)

  const parts: AdaptedContentPart[] = []
  let textBuffer = ""
  let goalCounter = 0
  let textSegmentCounter = 0

  const flushText = () => {
    const cleaned = textBuffer.replace(/^\n+|\n+$/g, "")
    textBuffer = ""
    if (cleaned.trim().length === 0) return

    const expanded = expandInlineToolText(
      cleaned,
      messageId,
      blockIndex * 100 + textSegmentCounter,
      toolCallFailedText
    )
    textSegmentCounter += 1

    if (expanded) {
      parts.push(...expanded)
    } else {
      parts.push({ type: "text", text: cleaned })
    }
  }

  let cursor = 0
  for (const marker of markers) {
    if (marker.start > cursor) {
      textBuffer += text.slice(cursor, marker.start)
    }

    const payloadWithoutLeading = marker.payload.replace(/^\s+/, "")
    const fallbackObjective = inferObjectiveFromGoalPayload(
      payloadWithoutLeading
    )
    const hintedObjective =
      sortedObjectiveHints.find((hint) =>
        payloadWithoutLeading.startsWith(hint)
      ) ?? null
    const objective = sharedObjective ?? hintedObjective ?? fallbackObjective
    if (marker.status.length === 0 || objective.length === 0) {
      textBuffer += text.slice(marker.start, marker.payloadEnd)
      cursor = marker.payloadEnd
      continue
    }
    const trailingText = payloadWithoutLeading.startsWith(objective)
      ? payloadWithoutLeading.slice(objective.length)
      : ""

    flushText()
    parts.push(
      createSyntheticGoalToolPart(
        marker.status,
        objective,
        messageId,
        blockIndex,
        goalCounter
      )
    )
    goalCounter += 1
    textBuffer += trailingText
    cursor = marker.payloadEnd
  }

  if (cursor < text.length) {
    textBuffer += text.slice(cursor)
  }
  flushText()
  return parts
}

function sanitizeMentionName(raw: string): string {
  return raw.replace(/[),.;:!?]+$/g, "")
}

function normalizeResourceText(text: string): string {
  return text
    .replace(/[ \t]{2,}/g, " ")
    .replace(/\s+\n/g, "\n")
    .replace(/\n\s+/g, "\n")
    .trim()
}

function fileNameFromUri(uri: string): string {
  try {
    const url = new URL(uri)
    const segment = url.pathname.split("/").pop() || ""
    return decodeURIComponent(segment) || uri
  } catch {
    return uri
  }
}

function addResource(
  resources: UserResourceDisplay[],
  resource: UserResourceDisplay
) {
  if (
    resources.some(
      (item) => item.name === resource.name && item.uri === resource.uri
    )
  ) {
    return
  }
  resources.push(resource)
}

function addImage(images: UserImageDisplay[], image: UserImageDisplay) {
  const key = `${image.mime_type}:${image.data.length}:${image.data.slice(0, 64)}`
  if (
    images.some(
      (item) =>
        `${item.mime_type}:${item.data.length}:${item.data.slice(0, 64)}` ===
        key
    )
  ) {
    return
  }
  images.push(image)
}

export function extractUserResourcesFromText(text: string): {
  text: string
  resources: UserResourceDisplay[]
} {
  const resources: UserResourceDisplay[] = []
  const withoutBlocked = text.replace(
    BLOCKED_RESOURCE_MENTION_RE,
    (_match: string, mention: string) => {
      const name = sanitizeMentionName(mention)
      if (name.length > 0) {
        addResource(resources, {
          name,
          uri: name,
          mime_type: null,
        })
      }
      return ""
    }
  )
  const cleaned = withoutBlocked.replace(
    MARKDOWN_LINK_RE,
    (match: string, label: string, uri: string) => {
      const normalizedLabel = label.trim()
      const normalizedUri = uri.trim()
      const hasMentionLabel = normalizedLabel.startsWith("@")
      const isFileUri = normalizedUri.toLowerCase().startsWith("file://")
      if (!hasMentionLabel && !isFileUri) {
        return match
      }

      const candidateName = hasMentionLabel
        ? normalizedLabel.slice(1)
        : normalizedLabel
      const name = sanitizeMentionName(candidateName) || fileNameFromUri(uri)
      addResource(resources, {
        name,
        uri: normalizedUri,
        mime_type: null,
      })
      return ""
    }
  )

  return {
    text: normalizeResourceText(cleaned),
    resources,
  }
}

function splitUserTextAndResources(
  parts: AdaptedContentPart[],
  attachedResourcesText: string
): {
  parts: AdaptedContentPart[]
  resources: UserResourceDisplay[]
} {
  const resources: UserResourceDisplay[] = []
  const nextParts: AdaptedContentPart[] = []

  for (const part of parts) {
    if (part.type !== "text") {
      nextParts.push(part)
      continue
    }
    const extracted = extractUserResourcesFromText(part.text)
    if (extracted.resources.length > 0) {
      resources.push(...extracted.resources)
      if (extracted.text.length > 0) {
        nextParts.push({ type: "text", text: extracted.text })
      }
    } else {
      nextParts.push(part)
    }
  }

  if (nextParts.length === 0 && resources.length > 0) {
    nextParts.push({ type: "text", text: attachedResourcesText })
  }

  return { parts: nextParts, resources }
}

function deriveImageNameFromBlock(
  block: Extract<ContentBlock, { type: "image" }>
): string {
  if (block.uri && block.uri.trim().length > 0) {
    return fileNameFromUri(block.uri)
  }
  const ext = block.mime_type.split("/")[1]?.split("+")[0] ?? "image"
  return `image.${ext}`
}

function extractUserImagesFromBlocks(
  blocks: ContentBlock[]
): UserImageDisplay[] {
  const images: UserImageDisplay[] = []
  for (const block of blocks) {
    if (block.type !== "image") continue
    if (!block.data || !block.mime_type) continue
    addImage(images, {
      name: deriveImageNameFromBlock(block),
      data: block.data,
      mime_type: block.mime_type,
      uri: block.uri ?? null,
    })
  }
  return images
}

/**
 * Generate a stable tool call ID based on message ID and block index
 */
function generateToolCallId(messageId: string, blockIndex: number): string {
  return `${messageId}-tool-${blockIndex}`
}

/**
 * Transform a single ContentBlock to AdaptedContentPart
 */
function adaptContentBlock(
  block: ContentBlock,
  messageId: string,
  blockIndex: number,
  isStreaming: boolean = false
): AdaptedContentPart | null {
  switch (block.type) {
    case "text":
      return {
        type: "text",
        text: block.text,
      }

    case "tool_use":
      return {
        type: "tool-call",
        toolCallId:
          block.tool_use_id ?? generateToolCallId(messageId, blockIndex),
        toolName: block.tool_name,
        input: block.input_preview,
        state: "input-available",
        meta: block.meta ?? null,
      }

    case "tool_result":
      return {
        type: "tool-result",
        toolCallId: generateToolCallId(messageId, blockIndex),
        output: block.output_preview,
        errorText: block.is_error
          ? block.output_preview || undefined
          : undefined,
        state: block.is_error ? "output-error" : "output-available",
      }

    case "thinking":
      return {
        type: "reasoning",
        content: block.text,
        isStreaming,
      }

    case "image_generation": {
      const img = block.image ?? null
      const display: UserImageDisplay | null =
        img && img.data && img.mime_type
          ? {
              name: deriveImageNameFromImageData(img),
              data: img.data,
              mime_type: img.mime_type,
              uri: img.uri ?? null,
            }
          : null
      return {
        type: "generated-image",
        revisedPrompt: block.revised_prompt ?? null,
        image: display,
        status: block.status ?? null,
      }
    }

    default:
      return null
  }
}

function deriveImageNameFromImageData(img: {
  data: string
  mime_type: string
  uri?: string | null
}): string {
  if (img.uri && img.uri.trim().length > 0) {
    return fileNameFromUri(img.uri)
  }
  const ext = img.mime_type.split("/")[1]?.split("+")[0] ?? "image"
  return `image.${ext}`
}

/**
 * Merge adjacent tool-group parts in a parts array into a single tool-group.
 * Used for cross-turn merging when concatenated content from consecutive
 * assistant turns lands two tool-groups next to each other.
 */
export function mergeAdjacentToolGroups(
  parts: AdaptedContentPart[]
): AdaptedContentPart[] {
  const result: AdaptedContentPart[] = []
  for (const part of parts) {
    const last = result[result.length - 1]
    if (part.type === "tool-group" && last?.type === "tool-group") {
      const mergedItems = [...last.items, ...part.items]
      result[result.length - 1] = {
        type: "tool-group",
        items: mergedItems,
        isStreaming: mergedItems.some(
          (item) =>
            item.state === "input-streaming" || item.state === "input-available"
        ),
      }
    } else {
      result.push(part)
    }
  }
  return result
}

/**
 * Wrap any consecutive run of tool-call parts into a single tool-group.
 * Text, reasoning, tool-result and any other part types break the run.
 * Even a single tool call is wrapped, so the renderer can present a uniform
 * collapsed summary across history.
 */
export function groupConsecutiveToolCalls(
  parts: AdaptedContentPart[]
): AdaptedContentPart[] {
  const result: AdaptedContentPart[] = []
  let buffer: AdaptedToolCallPart[] = []

  const flush = () => {
    if (buffer.length === 0) return
    const items = buffer
    buffer = []
    const isStreaming = items.some(
      (item) =>
        item.state === "input-streaming" || item.state === "input-available"
    )
    result.push({ type: "tool-group", items, isStreaming })
  }

  for (const part of parts) {
    if (part.type === "tool-call" && !isAgentLikeToolName(part.toolName)) {
      buffer.push(part)
      continue
    }
    flush()
    result.push(part)
  }
  flush()

  return result
}

/**
 * Wrap each run of consecutive `get_delegation_status` poll parts into a single
 * `delegation-status-group` part. Runs after `groupConsecutiveToolCalls`, which
 * leaves delegation (agent-like) tool calls standalone — so the status polls
 * arrive here as bare `tool-call` parts. Any non-status part (text, reasoning,
 * tool-group, the `delegate_to_agent` / `cancel_delegation` cards, …) breaks
 * the run, so only genuinely consecutive polls collapse. Even a single poll is
 * wrapped, so the merged-card status resolution (a returned "running" poll
 * reads as a settled snapshot, not a spinner) applies uniformly.
 */
export function groupConsecutiveDelegationStatus(
  parts: AdaptedContentPart[]
): AdaptedContentPart[] {
  const result: AdaptedContentPart[] = []
  let buffer: AdaptedToolCallPart[] = []

  const flush = () => {
    if (buffer.length === 0) return
    const polls = buffer
    buffer = []
    result.push({ type: "delegation-status-group", polls })
  }

  for (const part of parts) {
    if (
      part.type === "tool-call" &&
      isDelegationStatusToolName(part.toolName)
    ) {
      buffer.push(part)
      continue
    }
    flush()
    result.push(part)
  }
  flush()

  return result
}

/**
 * Merge adjacent `delegation-status-group` parts into one. Mirrors
 * `mergeAdjacentToolGroups`: used for cross-turn merging, where each polling
 * round is its own assistant turn and the concatenated parts land two
 * single-poll groups next to each other.
 */
export function mergeAdjacentDelegationStatusGroups(
  parts: AdaptedContentPart[]
): AdaptedContentPart[] {
  const result: AdaptedContentPart[] = []
  for (const part of parts) {
    const last = result[result.length - 1]
    if (
      part.type === "delegation-status-group" &&
      last?.type === "delegation-status-group"
    ) {
      result[result.length - 1] = {
        type: "delegation-status-group",
        polls: [...last.polls, ...part.polls],
      }
    } else {
      result.push(part)
    }
  }
  return result
}

function isGoalStartPart(
  part: AdaptedContentPart
): part is AdaptedToolCallPart {
  return (
    part.type === "tool-call" &&
    normalizeToolName(part.toolName) === "create_goal"
  )
}

function isGoalEndPart(part: AdaptedContentPart): part is AdaptedToolCallPart {
  return (
    part.type === "tool-call" &&
    normalizeToolName(part.toolName) === "update_goal"
  )
}

function isRunningToolCall(part: AdaptedToolCallPart): boolean {
  return part.state === "input-streaming" || part.state === "input-available"
}

function parseJsonRecord(
  raw: string | null | undefined
): Record<string, unknown> | null {
  if (!raw) return null
  try {
    return asRecord(JSON.parse(raw))
  } catch {
    return null
  }
}

function nestedRecord(
  obj: Record<string, unknown> | null,
  key: string
): Record<string, unknown> | null {
  return asRecord(obj?.[key])
}

function stringProperty(
  obj: Record<string, unknown> | null,
  key: string
): string | null {
  const value = obj?.[key]
  return typeof value === "string" && value.trim().length > 0
    ? value.trim()
    : null
}

function goalObjectiveKeyFromTool(part: AdaptedToolCallPart): string | null {
  const input = parseJsonRecord(part.input)
  const output = parseJsonRecord(part.output ?? part.errorText)
  const outputGoal = nestedRecord(output, "goal")
  const objective =
    stringProperty(outputGoal, "objective") ??
    stringProperty(input, "objective")
  return objective ? objective.trim() : null
}

function goalObjectiveKeyFromRun(part: AdaptedGoalRunPart): string | null {
  return (
    (part.end ? goalObjectiveKeyFromTool(part.end) : null) ??
    goalObjectiveKeyFromTool(part.start)
  )
}

function collectGoalObjectives(parts: AdaptedContentPart[]): string[] {
  const objectives: string[] = []
  const seen = new Set<string>()

  const add = (objective: string | null) => {
    if (!objective || seen.has(objective)) return
    seen.add(objective)
    objectives.push(objective)
  }

  for (const part of parts) {
    if (part.type === "goal-run") {
      add(goalObjectiveKeyFromRun(part))
      for (const item of collectGoalObjectives(part.items)) {
        add(item)
      }
    } else if (part.type === "tool-call") {
      add(goalObjectiveKeyFromTool(part))
    }
  }

  return objectives
}

function mergeGoalObjectiveHints(
  existing: readonly string[] | undefined,
  incoming: readonly string[]
): string[] {
  const merged = new Set(existing ?? [])
  for (const objective of incoming) {
    if (objective.trim().length > 0) merged.add(objective.trim())
  }
  return Array.from(merged).sort((a, b) => b.length - a.length)
}

/**
 * Wrap a Codex `/goal` lifecycle into one card-style part:
 * `create_goal` starts the run, every intervening adapted part becomes card
 * body content, and `update_goal` closes the run. An unfinished run remains
 * wrapped with `isRunning=true` so the renderer can shimmer the title while the
 * agent is still working.
 */
export function groupGoalRuns(
  parts: AdaptedContentPart[]
): AdaptedContentPart[] {
  const result: AdaptedContentPart[] = []
  let active: {
    start: AdaptedToolCallPart
    items: AdaptedContentPart[]
  } | null = null
  const completedGoalObjectives = new Set<string>()

  const rememberCompletedGoal = (
    start: AdaptedToolCallPart,
    end: AdaptedToolCallPart | null
  ) => {
    const objective =
      (end ? goalObjectiveKeyFromTool(end) : null) ??
      goalObjectiveKeyFromTool(start)
    if (objective) completedGoalObjectives.add(objective)
  }

  const isStaleActiveGoal = (part: AdaptedToolCallPart): boolean => {
    const objective = goalObjectiveKeyFromTool(part)
    return Boolean(objective && completedGoalObjectives.has(objective))
  }

  const flushActive = () => {
    if (!active) return
    result.push({
      type: "goal-run",
      start: active.start,
      end: null,
      items: [...active.items],
      isRunning: true,
    })
    active = null
  }

  for (const part of parts) {
    if (part.type === "goal-run") {
      const objective = goalObjectiveKeyFromRun(part)
      const isStaleUnfinished =
        part.end === null &&
        Boolean(objective && completedGoalObjectives.has(objective))

      if (!active) {
        if (isStaleUnfinished) {
          result.push(...part.items)
        } else if (part.end === null) {
          active = { start: part.start, items: [...part.items] }
        } else {
          result.push({
            ...part,
            items: [...part.items],
          })
          rememberCompletedGoal(part.start, part.end)
        }
        continue
      }

      if (isStaleUnfinished) {
        active.items.push(...part.items)
        continue
      }

      active.start = part.start
      if (part.end === null) {
        active.items.push(...part.items)
      } else {
        result.push({
          type: "goal-run",
          start: active.start,
          end: part.end,
          items: [...active.items, ...part.items],
          isRunning: part.isRunning,
        })
        rememberCompletedGoal(active.start, part.end)
        active = null
      }
      continue
    }

    if (isGoalStartPart(part)) {
      if (!active && isStaleActiveGoal(part)) {
        continue
      }
      if (active) {
        active.start = part
        continue
      }
      flushActive()
      active = { start: part, items: [] }
      continue
    }

    if (active && isGoalEndPart(part)) {
      result.push({
        type: "goal-run",
        start: active.start,
        end: part,
        items: [...active.items],
        isRunning: isRunningToolCall(part),
      })
      rememberCompletedGoal(active.start, part)
      active = null
      continue
    }

    if (active) {
      active.items.push(part)
    } else {
      result.push(part)
    }
  }

  flushActive()
  return result
}

/**
 * Build a map of tool_use_id → tool_result ContentBlock from content blocks.
 * Used to correlate tool calls with their results.
 */
function buildToolResultMap(
  blocks: ContentBlock[]
): Map<string, ContentBlock & { type: "tool_result" }> {
  const map = new Map<string, ContentBlock & { type: "tool_result" }>()
  for (const block of blocks) {
    if (block.type === "tool_result" && block.tool_use_id) {
      map.set(block.tool_use_id, block)
    }
  }
  return map
}

/**
 * Transform a MessageTurn (from backend) to AdaptedMessage format.
 * Same correlation logic as adaptUnifiedMessage but operates on turn.blocks.
 *
 * `inProgressToolCallIds` lets streaming consumers expose partial tool output
 * (e.g. terminal stdout streamed during execution) without flipping the tool
 * into a "completed" visual state. When a tool_use's id is in this set, the
 * adapter emits state="input-available" with the partial output attached, so
 * the renderer can keep showing the running spinner while the live output
 * streams in.
 */
export function adaptMessageTurn(
  turn: MessageTurn,
  text: AdapterMessageText,
  isStreaming: boolean = false,
  inProgressToolCallIds?: Set<string>,
  goalObjectiveHints?: readonly string[]
): AdaptedMessage {
  const adaptedContent: AdaptedContentPart[] = []
  const resultMap = buildToolResultMap(turn.blocks)
  const matchedResultIds = new Set<string>()

  // Track indices of tool_result blocks consumed by position-based matching
  const positionMatchedIndices = new Set<number>()

  for (let index = 0; index < turn.blocks.length; index++) {
    const block = turn.blocks[index]

    if (turn.role === "assistant" && block.type === "text") {
      const goalExpandedParts = expandGoalUpdateText(
        block.text,
        turn.id,
        index,
        text.toolCallFailed,
        goalObjectiveHints
      )
      if (goalExpandedParts) {
        adaptedContent.push(...goalExpandedParts)
        continue
      }

      const expandedParts = expandInlineToolText(
        block.text,
        turn.id,
        index,
        text.toolCallFailed
      )
      if (expandedParts) {
        adaptedContent.push(...expandedParts)
        continue
      }
    }

    if (block.type === "tool_use") {
      const toolCallId = block.tool_use_id || generateToolCallId(turn.id, index)
      const matchedResult = block.tool_use_id
        ? resultMap.get(block.tool_use_id)
        : undefined

      const isToolStillRunning =
        !!block.tool_use_id && !!inProgressToolCallIds?.has(block.tool_use_id)

      if (matchedResult) {
        matchedResultIds.add(block.tool_use_id!)
        adaptedContent.push({
          type: "tool-call",
          toolCallId,
          toolName: block.tool_name,
          input: block.input_preview,
          state: isToolStillRunning
            ? "input-available"
            : matchedResult.is_error
              ? "output-error"
              : "output-available",
          output: matchedResult.output_preview,
          errorText: matchedResult.is_error
            ? matchedResult.output_preview || undefined
            : undefined,
          agentStats: matchedResult.agent_stats ?? undefined,
          meta: block.meta ?? null,
        })
      } else {
        // Position-based matching: if this tool_use has no ID, check next block
        const nextBlock = turn.blocks[index + 1]
        const positionalResult =
          !block.tool_use_id &&
          nextBlock?.type === "tool_result" &&
          !nextBlock.tool_use_id
            ? nextBlock
            : undefined

        if (positionalResult) {
          positionMatchedIndices.add(index + 1)
          adaptedContent.push({
            type: "tool-call",
            toolCallId,
            toolName: block.tool_name,
            input: block.input_preview,
            state: positionalResult.is_error
              ? "output-error"
              : "output-available",
            output: positionalResult.output_preview,
            errorText: positionalResult.is_error
              ? positionalResult.output_preview || undefined
              : undefined,
            agentStats: positionalResult.agent_stats ?? undefined,
            meta: block.meta ?? null,
          })
        } else {
          // For live streaming, unmatched tools are still running.
          // For DB historical data, default to "completed" since the
          // conversation has already ended.
          adaptedContent.push({
            type: "tool-call",
            toolCallId,
            toolName: block.tool_name,
            input: block.input_preview,
            state: isStreaming ? "input-available" : "output-available",
            meta: block.meta ?? null,
          })
        }
      }
      continue
    }

    // Skip tool_result blocks already matched by ID or position
    if (
      block.type === "tool_result" &&
      ((block.tool_use_id && matchedResultIds.has(block.tool_use_id)) ||
        positionMatchedIndices.has(index))
    ) {
      continue
    }

    const adapted = adaptContentBlock(block, turn.id, index, false)
    if (adapted) {
      adaptedContent.push(adapted)
    }
  }

  // Mark the last reasoning block as streaming if the turn is actively streaming
  if (isStreaming) {
    const last = adaptedContent[adaptedContent.length - 1]
    if (last?.type === "reasoning") {
      last.isStreaming = true
    }
  }

  const groupedContent =
    turn.role === "assistant"
      ? groupGoalRuns(
          groupConsecutiveDelegationStatus(
            groupConsecutiveToolCalls(adaptedContent)
          )
        )
      : adaptedContent

  const userSplit =
    turn.role === "user"
      ? splitUserTextAndResources(groupedContent, text.attachedResources)
      : { parts: groupedContent, resources: [] as UserResourceDisplay[] }
  // Only user-uploaded images surface as top-of-message attachments.
  // Assistant-side image_generation flows through the inline
  // `generated-image` part, rendered in-position.
  const userImages =
    turn.role === "user" ? extractUserImagesFromBlocks(turn.blocks) : []

  return {
    id: turn.id,
    role: turn.role,
    content: userSplit.parts,
    userResources:
      userSplit.resources.length > 0 ? userSplit.resources : undefined,
    userImages: userImages.length > 0 ? userImages : undefined,
    timestamp: turn.timestamp,
    usage: turn.usage,
    duration_ms: turn.duration_ms,
    model: turn.model,
    completed_at: turn.completed_at,
  }
}

/**
 * Transform all turns in a conversation to AdaptedMessage[].
 * Internally computes completedToolIds so callers don't need to.
 *
 * `inProgressToolCallIdsByIndex` carries the set of tool_call_ids that are
 * still streaming for each streaming-phase turn (keyed by turn index). The
 * adapter forwards this to adaptMessageTurn so partial output renders without
 * flipping the tool out of the running visual state.
 */
export function adaptMessageTurns(
  turns: MessageTurn[],
  text: AdapterMessageText,
  streamingIndices?: Set<number>,
  inProgressToolCallIdsByIndex?: Map<number, Set<string>>
): AdaptedMessage[] {
  return turns.map((turn, i) =>
    adaptMessageTurn(
      turn,
      text,
      streamingIndices?.has(i) ?? false,
      inProgressToolCallIdsByIndex?.get(i)
    )
  )
}

interface TurnCacheEntry {
  text: AdapterMessageText
  blocks: ContentBlock[]
  blocksLen: number
  timestamp: string
  role: MessageRole
  usage: TurnUsage | null | undefined
  duration_ms: number | null | undefined
  model: string | null | undefined
  completed_at: string | null | undefined
  adapted: AdaptedMessage
}

export interface MessageTurnAdapter {
  /**
   * Adapt all turns to messages, reusing previously computed `AdaptedMessage`
   * references for turns whose content hasn't changed. Streaming turns and
   * turns with in-progress tool calls are never cached so partial state always
   * re-flows through the adapter.
   */
  adapt(
    turns: MessageTurn[],
    text: AdapterMessageText,
    streamingIndices?: Set<number>,
    inProgressToolCallIdsByIndex?: Map<number, Set<string>>
  ): AdaptedMessage[]
  clear(): void
}

/**
 * Build a stateful adapter that caches per-turn results. Intended to live for
 * the lifetime of a chat view — instantiate once via `useRef` so the cache
 * survives across re-renders triggered by streaming deltas.
 *
 * Cache invalidation: an entry is reused only when `(text, blocks,
 * blocksLen, timestamp, role, usage, duration_ms, model)` all match. The
 * blocks reference catches whole-turn rewrites (e.g. detail refetch
 * replacing `detail.turns`) where blocksLen/timestamp may stay equal but
 * a tool's output_preview was updated; PATCH_TURN_METADATA preserves the
 * blocks reference, so it still hits. The usage trio is patched in by
 * `syncTurnMetadata` after a stream finishes (initial blocks land first,
 * token totals arrive on a later DB roundtrip), so excluding them would
 * freeze the turn at its pre-patch state and the post-stream stats row
 * would never appear. Turns no longer present are GC'd at the end of
 * every adapt() call so the cache size tracks the conversation.
 */
export function createMessageTurnAdapter(): MessageTurnAdapter {
  const cache = new Map<string, TurnCacheEntry>()
  const goalObjectiveHints = new Map<string, string[]>()

  return {
    adapt(turns, text, streamingIndices, inProgressToolCallIdsByIndex) {
      const seen = new Set<string>()
      const out: AdaptedMessage[] = new Array(turns.length)

      for (let i = 0; i < turns.length; i += 1) {
        const turn = turns[i]
        seen.add(turn.id)
        const isStreaming = streamingIndices?.has(i) ?? false
        const inProgress = inProgressToolCallIdsByIndex?.get(i)
        const cacheable = !isStreaming && !inProgress
        const blocksLen = turn.blocks.length

        if (cacheable) {
          const cached = cache.get(turn.id)
          if (
            cached &&
            cached.text === text &&
            cached.blocks === turn.blocks &&
            cached.blocksLen === blocksLen &&
            cached.timestamp === turn.timestamp &&
            cached.role === turn.role &&
            cached.usage === turn.usage &&
            cached.duration_ms === turn.duration_ms &&
            cached.model === turn.model &&
            cached.completed_at === turn.completed_at
          ) {
            out[i] = cached.adapted
            continue
          }
        }

        const adapted = adaptMessageTurn(
          turn,
          text,
          isStreaming,
          inProgress,
          goalObjectiveHints.get(turn.id)
        )
        out[i] = adapted

        const objectives = collectGoalObjectives(adapted.content)
        if (objectives.length > 0) {
          goalObjectiveHints.set(
            turn.id,
            mergeGoalObjectiveHints(goalObjectiveHints.get(turn.id), objectives)
          )
        }

        if (cacheable) {
          cache.set(turn.id, {
            text,
            blocks: turn.blocks,
            blocksLen,
            timestamp: turn.timestamp,
            role: turn.role,
            usage: turn.usage,
            duration_ms: turn.duration_ms,
            model: turn.model,
            completed_at: turn.completed_at,
            adapted,
          })
        } else {
          cache.delete(turn.id)
        }
      }

      if (cache.size > seen.size) {
        for (const id of cache.keys()) {
          if (!seen.has(id)) cache.delete(id)
        }
      }
      if (goalObjectiveHints.size > seen.size) {
        for (const id of goalObjectiveHints.keys()) {
          if (!seen.has(id)) goalObjectiveHints.delete(id)
        }
      }

      return out
    },
    clear() {
      cache.clear()
      goalObjectiveHints.clear()
    },
  }
}
