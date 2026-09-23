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

/** Desktop "turn complete" and the chat-channel completion post. Absorb
 *  settlements are not a turn ending. */
export function shouldNotifyTurnComplete(stopReason: string): boolean {
  return busyPromptStop(stopReason) == null
}

export interface SubmittedPrompt {
  draft: PromptDraft
  modeId: string | null
}

export type BusyAbsorbPlan =
  | { action: "requeue"; draft: PromptDraft; modeId: string | null }
  | { action: "keep" }
  | { action: "ignore" }

/**
 * One `turn_complete` for a non-steering absorb.
 *
 * `requeue` uses the draft that was actually sent (resource blocks and the
 * mode from that send). The optimistic bubble is only a fallback: it stored
 * images and display text.
 */
export function planBusyAbsorb(input: {
  stopReason: string
  submitted: SubmittedPrompt | null
  optimisticTurns?: { role: string; blocks: ContentBlock[] }[]
}): BusyAbsorbPlan {
  const disposition = busyPromptStop(input.stopReason)
  if (disposition === "keep") return { action: "keep" }
  if (disposition !== "requeue") return { action: "ignore" }
  if (input.submitted) {
    return {
      action: "requeue",
      draft: input.submitted.draft,
      modeId: input.submitted.modeId,
    }
  }
  for (const turn of [...(input.optimisticTurns ?? [])].reverse()) {
    const draft = draftFromOptimisticUserTurn(turn)
    if (draft) return { action: "requeue", draft, modeId: null }
  }
  return { action: "ignore" }
}

/** Rebuild a composer draft from the optimistic user turn a busy absorb
 *  rolled back. The bubble only kept images and display text, so a caller
 *  that still has the submitted `PromptDraft` should requeue that instead. */
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
