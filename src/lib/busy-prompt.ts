import type { ContentBlock, PromptDraft, PromptInputBlock } from "@/lib/types"

/**
 * What the client should do with a turn the backend settled because a
 * non-steering agent absorbed the prompt instead of running it.
 *
 * `requeue` (`busy`) — nothing acknowledged the text. Put it back on the
 * message queue. `keep` (`deferred`) — the agent accepted it into the live
 * turn or its own queue; leave the user message in place and do not send it
 * again.
 *
 * Any other stop reason is a real turn end.
 */
export type BusyPromptStop = "requeue" | "keep"

export function busyPromptStop(stopReason: string): BusyPromptStop | null {
  if (stopReason === "busy") return "requeue"
  if (stopReason === "deferred") return "keep"
  return null
}

/** Rebuild a composer draft from the optimistic user turn a busy absorb
 *  rolled back. Text and images round-trip; other block kinds are not part
 *  of a prompt draft. */
export function draftFromOptimisticUserTurn(turn: {
  role: string
  blocks: ContentBlock[]
}): PromptDraft | null {
  if (turn.role !== "user") return null
  const blocks: PromptInputBlock[] = []
  for (const block of turn.blocks) {
    if (block.type === "text") {
      if (block.text.length > 0) {
        blocks.push({ type: "text", text: block.text })
      }
    } else if (block.type === "image") {
      blocks.push({
        type: "image",
        data: block.data,
        mime_type: block.mime_type,
        uri: block.uri ?? null,
      })
    }
  }
  const displayText = blocks
    .filter((block): block is { type: "text"; text: string } => {
      return block.type === "text"
    })
    .map((block) => block.text)
    .join("")
  if (!displayText.trim() && !blocks.some((block) => block.type === "image")) {
    return null
  }
  return { blocks, displayText }
}
