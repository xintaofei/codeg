import type { PromptDraft, PromptInputBlock } from "@/lib/types"

export const MIN_REPEAT_COUNT = 2
export const MAX_REPEAT_COUNT = 50

export interface RepeatIntent {
  baseText: string
  count: number
}

// Trailing multiplier: base + optional spaces + x/X + optional spaces + digits.
// DotAll so baseText may include newlines. Non-greedy base so the last xN is trailing.
const REPEAT_INTENT_RE =
  /^(?<base>[\s\S]+?)[ \t]*[xX][ \t]*(?<count>\d+)[ \t]*$/

export function parseRepeatIntent(text: string): RepeatIntent | null {
  const match = REPEAT_INTENT_RE.exec(text)
  if (!match?.groups) return null

  const baseText = match.groups.base.replace(/[ \t]+$/u, "")
  if (!baseText) return null

  const count = Number.parseInt(match.groups.count, 10)
  if (!Number.isFinite(count)) return null
  if (count < MIN_REPEAT_COUNT || count > MAX_REPEAT_COUNT) return null

  return { baseText, count }
}

function rewriteTrailingTextBlock(
  blocks: PromptInputBlock[],
  baseText: string
): PromptInputBlock[] {
  const next = blocks.map((block) => ({ ...block }))
  for (let i = next.length - 1; i >= 0; i -= 1) {
    if (next[i].type === "text") {
      next[i] = { type: "text", text: baseText }
      return next
    }
  }
  if (baseText) return [{ type: "text", text: baseText }, ...next]
  return next
}

export function applyRepeatBaseText(
  draft: PromptDraft,
  baseText: string
): PromptDraft {
  return {
    displayText: baseText,
    blocks: rewriteTrailingTextBlock(draft.blocks, baseText),
  }
}
