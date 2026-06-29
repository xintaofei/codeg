import type { PromptInputBlock } from "@/lib/types"

/**
 * Sentinel that brackets the auto-injected live-feedback reminder inside an
 * OUTGOING prompt. It serves two jobs at once:
 *
 *  1. Strip anchor. codeg keeps no copy of a sent prompt, so on reload the user
 *     turn is reparsed from the agent's own session file — the reminder rides
 *     back with it. `stripFeedbackReminder` finds this sentinel and removes
 *     everything from it onward, so the reminder never surfaces in the UI.
 *  2. Recognizability. The agent sees the sentinel too; it labels the trailing
 *     block as a Codeg system note rather than the user's own prose.
 *
 * It is a CODE constant, never part of the localized reminder string, so the
 * strip is locale-independent (a reminder sent in any language still strips).
 * The Unicode brackets are not interpreted by Markdown/HTML and make an
 * accidental collision with real user text effectively impossible.
 */
export const FEEDBACK_REMINDER_SENTINEL = "⟦codeg:live-feedback⟧"

/**
 * Append the live-feedback reminder to the end of a prompt's OUTGOING text,
 * bracketed by {@link FEEDBACK_REMINDER_SENTINEL}.
 *
 * Returns a NEW block array (never mutates the input). The reminder is joined to
 * the LAST text block (so it reads as the final line of the prose); when the
 * message carries no text at all (an attachments-only send) it is added as a
 * trailing text block. Joining to the last text block — rather than assuming the
 * composer emits text at index 0 — keeps this self-contained and correct for any
 * block ordering, not just the current text-first composer output.
 *
 * Why this lives at the send chokepoint (`ConversationTabView.handleSend`) and
 * NOT in the composer's `buildDraft`: the reminder must ride the wire to the
 * agent but stay out of every *stored* draft. A draft parked in the message
 * queue is re-hydrated into the editor (from its `blocks`) when the user edits
 * it — so a reminder baked into `blocks` would leak into the visible composer
 * and get appended a second time on save. Injecting only at the moment of
 * transmission keeps queued/edited drafts pristine and guarantees exactly one
 * reminder per delivered prompt. `displayText` is never touched, so the
 * optimistic user bubble shows the user's own words; on reload the sentinel
 * lets {@link stripFeedbackReminder} hide the reminder again.
 */
export function appendFeedbackReminder(
  blocks: PromptInputBlock[],
  reminder: string
): PromptInputBlock[] {
  const marked = `${FEEDBACK_REMINDER_SENTINEL} ${reminder}`
  let lastTextIndex = -1
  for (let i = blocks.length - 1; i >= 0; i--) {
    if (blocks[i].type === "text") {
      lastTextIndex = i
      break
    }
  }
  if (lastTextIndex === -1) {
    return [...blocks, { type: "text", text: marked }]
  }
  return blocks.map((block, i) =>
    i === lastTextIndex && block.type === "text"
      ? { ...block, text: `${block.text}\n\n${marked}` }
      : block
  )
}

/**
 * Remove an appended live-feedback reminder from a piece of DISPLAY text.
 *
 * The reminder is always the trailing content (joined to the end of the prose,
 * preceded by {@link FEEDBACK_REMINDER_SENTINEL}), so everything from the
 * sentinel onward is dropped and the trailing whitespace that joined it is
 * trimmed. Text that never carried a reminder is returned unchanged; a string
 * that is *only* the reminder collapses to "" (callers drop the emptied part).
 */
export function stripFeedbackReminder(text: string): string {
  const idx = text.indexOf(FEEDBACK_REMINDER_SENTINEL)
  if (idx === -1) return text
  return text.slice(0, idx).replace(/\s+$/, "")
}
