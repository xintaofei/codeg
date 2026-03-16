import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"

export type SessionLocatorPhase = "persisted" | "optimistic" | "streaming"

export type LocatorRole = "user" | "assistant"

export type LocatorPreviewKind =
  | "text"
  | "tool_only"
  | "attachment_only"
  | "pending_reply"
  | "empty"

export interface SessionLocatorRawTurn {
  turnId: string
  role: "user" | "assistant" | "system"
  phase: SessionLocatorPhase
  threadIndex: number
  parts: AdaptedContentPart[]
  resourceCount: number
  imageCount: number
}

export interface SessionLocatorPreview {
  text: string
  kind: LocatorPreviewKind
}

export interface SessionLocatorTarget {
  role: LocatorRole
  turnId: string
  threadIndex: number
  partIndex: number | null
  preview: SessionLocatorPreview
}

export interface SessionLocatorItem {
  id: string
  pairIndex: number
  status: "complete" | "pending_reply"
  user: SessionLocatorTarget
  assistant: SessionLocatorTarget | null
}

function normalizePreviewText(text: string): string {
  return text.replace(/\s+/g, " ").trim()
}

function extractTextParts(parts: AdaptedContentPart[]): string[] {
  return parts
    .flatMap((part) =>
      part.type === "text" ? [normalizePreviewText(part.text)] : []
    )
    .filter((text) => text.length > 0)
}

function hasToolContent(parts: AdaptedContentPart[]): boolean {
  return parts.some(
    (part) => part.type === "tool-call" || part.type === "tool-result"
  )
}

function getFirstTextPartIndex(parts: AdaptedContentPart[]): number | null {
  for (let i = 0; i < parts.length; i += 1) {
    const part = parts[i]
    if (part?.type === "text" && normalizePreviewText(part.text).length > 0) {
      return i
    }
  }

  return null
}

function getLastTextPartIndex(parts: AdaptedContentPart[]): number | null {
  for (let i = parts.length - 1; i >= 0; i -= 1) {
    const part = parts[i]
    if (part?.type === "text" && normalizePreviewText(part.text).length > 0) {
      return i
    }
  }

  return null
}

function getLastRenderablePartIndex(
  parts: AdaptedContentPart[]
): number | null {
  for (let i = parts.length - 1; i >= 0; i -= 1) {
    const part = parts[i]
    if (!part) continue

    if (part.type === "text") {
      if (normalizePreviewText(part.text).length > 0) {
        return i
      }
      continue
    }

    return i
  }

  return null
}

export function extractUserPreview(
  turn: SessionLocatorRawTurn
): SessionLocatorPreview {
  const [firstText] = extractTextParts(turn.parts)
  if (firstText) {
    return { text: firstText, kind: "text" }
  }

  if (turn.imageCount > 0 || turn.resourceCount > 0) {
    return { text: "", kind: "attachment_only" }
  }

  return { text: "", kind: "empty" }
}

export function extractAssistantFinalPreview(
  turn: SessionLocatorRawTurn
): SessionLocatorPreview {
  const textParts = extractTextParts(turn.parts)
  const lastText = textParts[textParts.length - 1]
  if (lastText) {
    return { text: lastText, kind: "text" }
  }

  if (hasToolContent(turn.parts)) {
    return { text: "", kind: "tool_only" }
  }

  return { text: "", kind: "empty" }
}

export function buildSessionLocatorItems(
  turns: SessionLocatorRawTurn[]
): SessionLocatorItem[] {
  const items: SessionLocatorItem[] = []
  let pendingUser: SessionLocatorTarget | null = null
  let pairIndex = 0

  const flushPendingUser = () => {
    if (!pendingUser) return

    items.push({
      id: `${pendingUser.turnId}-pending-${pairIndex}`,
      pairIndex,
      status: "pending_reply",
      user: pendingUser,
      assistant: null,
    })
    pairIndex += 1
    pendingUser = null
  }

  for (const turn of turns) {
    if (turn.role === "system") continue

    if (turn.role === "user") {
      flushPendingUser()
      pendingUser = {
        role: "user",
        turnId: turn.turnId,
        threadIndex: turn.threadIndex,
        partIndex: getFirstTextPartIndex(turn.parts),
        preview: extractUserPreview(turn),
      }
      continue
    }

    if (!pendingUser) continue

    const assistantTarget: SessionLocatorTarget = {
      role: "assistant",
      turnId: turn.turnId,
      threadIndex: turn.threadIndex,
      partIndex:
        getLastTextPartIndex(turn.parts) ??
        getLastRenderablePartIndex(turn.parts),
      preview: extractAssistantFinalPreview(turn),
    }

    items.push({
      id: `${pendingUser.turnId}-${turn.turnId}-${pairIndex}`,
      pairIndex,
      status: "complete",
      user: pendingUser,
      assistant: assistantTarget,
    })
    pairIndex += 1
    pendingUser = null
  }

  flushPendingUser()

  return items
}
