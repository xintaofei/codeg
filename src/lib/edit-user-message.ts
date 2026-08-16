import type { ContentBlock, MessageTurn, PromptInputBlock } from "@/lib/types"

/**
 * Client-side edit of a previous user message.
 *
 * ACP has no `message/edit` and `session/fork` cannot yet fork from a
 * midpoint (the RFD reserves `messageId` for that). Native harnesses still
 * let you rewrite a prompt and continue from there. We do the same on the
 * surfaces we own:
 *
 *   1. Restore the chosen user turn into the composer.
 *   2. Hide that turn and every later turn from the transcript we display
 *      (and persist the hidden timestamps so a reload stays truncated).
 *   3. Send the replacement as a normal `session/prompt` on the SAME session
 *      so every agent — Claude, Codex, Grok, custom ACP — uses the path it
 *      already understands.
 *
 * The agent still has the discarded turns in its own store (we never rewrite
 * a CLI session file). The replacement is the latest user instruction, which
 * is how a follow-up "I meant this instead" already works in those CLIs.
 */

/** Milliseconds since epoch for a turn's timestamp, or null if unparseable. */
export function turnTimestampMs(turn: Pick<MessageTurn, "timestamp">): number | null {
  const ms = Date.parse(turn.timestamp)
  return Number.isFinite(ms) ? ms : null
}

/**
 * Timestamps of `fromTurnId` and every turn after it, in the given order.
 * Empty when the id is missing — the caller must not persist an empty hide
 * (that would be a no-op hide of "nothing", not "everything").
 */
export function timestampsToHideFrom(
  turns: Pick<MessageTurn, "id" | "timestamp">[],
  fromTurnId: string
): number[] {
  const start = turns.findIndex((turn) => turn.id === fromTurnId)
  if (start < 0) return []
  const hidden: number[] = []
  for (let i = start; i < turns.length; i++) {
    const ms = turnTimestampMs(turns[i])
    if (ms != null) hidden.push(ms)
  }
  return hidden
}

/** Drop turns whose timestamp is in the hidden set. Order is preserved. */
export function filterHiddenTurns<T extends Pick<MessageTurn, "timestamp">>(
  turns: T[],
  hiddenMs: Iterable<number>
): T[] {
  const hidden = hiddenMs instanceof Set ? hiddenMs : new Set(hiddenMs)
  if (hidden.size === 0) return turns
  return turns.filter((turn) => {
    const ms = turnTimestampMs(turn)
    return ms == null || !hidden.has(ms)
  })
}

/**
 * Restore a stored user turn into the composer. Only text and image blocks
 * are sendable; tool/thinking/plan blocks never appear on a user turn and
 * are dropped if they do.
 */
export function contentBlocksToPromptInput(
  blocks: ContentBlock[]
): PromptInputBlock[] {
  const out: PromptInputBlock[] = []
  for (const block of blocks) {
    if (block.type === "text") {
      if (block.text.length > 0) {
        out.push({ type: "text", text: block.text })
      }
    } else if (block.type === "image") {
      out.push({
        type: "image",
        data: block.data,
        mime_type: block.mime_type,
        uri: block.uri ?? null,
      })
    }
  }
  return out
}

export function canEditUserTurn(options: {
  role: string
  phase: "persisted" | "optimistic" | "streaming"
  readOnly?: boolean
}): boolean {
  return (
    options.role === "user" &&
    options.phase === "persisted" &&
    !options.readOnly
  )
}
