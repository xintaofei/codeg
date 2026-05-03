import type { ContentBlock, MessageTurn } from "@/lib/types"

const STORAGE_KEY_PREFIX = "codeg:conversation-recovery-optimistic-turns"

function buildStorageKey(conversationId: number): string {
  return `${STORAGE_KEY_PREFIX}:${conversationId}`
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null
}

function isOptimisticAnchorId(anchorId: unknown): anchorId is string {
  return typeof anchorId === "string" && anchorId.startsWith("optimistic:")
}

function isRecoverableContentBlock(block: unknown): block is ContentBlock {
  if (!isRecord(block) || typeof block.type !== "string") {
    return false
  }

  switch (block.type) {
    case "text":
      return typeof block.text === "string"
    case "image":
      return (
        typeof block.data === "string" && typeof block.mime_type === "string"
      )
    default:
      return false
  }
}

function normalizeRecoverableTurn(turn: unknown): MessageTurn | null {
  if (!isRecord(turn)) return null
  if (turn.role !== "user") return null
  if (typeof turn.id !== "string" || typeof turn.timestamp !== "string") {
    return null
  }
  if (!Array.isArray(turn.blocks)) return null

  const blocks = turn.blocks.filter(isRecoverableContentBlock)
  if (blocks.length !== turn.blocks.length) return null

  if (!isOptimisticAnchorId(turn.anchor_id)) {
    return null
  }

  return {
    id: turn.id,
    anchor_id: turn.anchor_id,
    role: "user",
    blocks,
    timestamp: turn.timestamp,
  }
}

export function loadRecoverableOptimisticTurns(
  conversationId: number
): MessageTurn[] {
  if (!Number.isFinite(conversationId) || conversationId <= 0) {
    return []
  }
  if (typeof window === "undefined") return []

  try {
    const raw = localStorage.getItem(buildStorageKey(conversationId))
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed
      .map((turn) => normalizeRecoverableTurn(turn))
      .filter((turn): turn is MessageTurn => turn !== null)
  } catch {
    return []
  }
}

export function saveRecoverableOptimisticTurns(
  conversationId: number,
  turns: MessageTurn[]
): void {
  if (!Number.isFinite(conversationId) || conversationId <= 0) {
    return
  }
  if (typeof window === "undefined") return

  const recoverableTurns = turns.filter(
    (turn) => turn.role === "user" && isOptimisticAnchorId(turn.anchor_id)
  )

  try {
    const key = buildStorageKey(conversationId)
    if (recoverableTurns.length === 0) {
      localStorage.removeItem(key)
      return
    }
    localStorage.setItem(key, JSON.stringify(recoverableTurns))
  } catch {
    /* ignore */
  }
}

export function clearRecoverableOptimisticTurns(conversationId: number): void {
  if (!Number.isFinite(conversationId) || conversationId <= 0) {
    return
  }
  if (typeof window === "undefined") return

  try {
    localStorage.removeItem(buildStorageKey(conversationId))
  } catch {
    /* ignore */
  }
}
