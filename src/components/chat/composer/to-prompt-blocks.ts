import type { Editor } from "@tiptap/core"
import type { Node as ProseMirrorNode } from "@tiptap/pm/model"

import type { PromptInputBlock } from "@/lib/types"

import { referenceToMarkdown } from "./reference-text"
import { isEmbeddedReferenceUri } from "./reference-uri"
import type { ReferenceAttrs } from "./types"
import {
  isAgentRuleSelectionAttrs,
  serializeAgentRuleSelection,
} from "./agent-rule-selection"

/**
 * Send serialization: turn the (plain-text) composer document into the prose
 * portion of a `PromptInputBlock[]`. (Out-of-band image / embedded-byte
 * attachments are appended by the host's `buildDraft`; this function owns only
 * the editor doc.)
 *
 * The composer holds only prose, hard breaks, and inline reference badges (no
 * Markdown formatting), so serialization is a single plain-text walk:
 *
 * - **text** nodes contribute their literal characters (a `# `, `**x**`, `- ` a
 *   user typed stays verbatim — the transcript renders it verbatim too).
 * - **reference** badges (EXCEPT embedded) contribute their inline token via
 *   {@link referenceToMarkdown}: file → `[label](file://uri)`, session/commit/
 *   agent → their link/`@` form, skill → `/id`·`$id`. This is unchanged from the
 *   old Markdown path — the wire format the backend's `user_blocks_from_prompt`
 *   and reload adapter expect — so nothing downstream (or already persisted)
 *   changes. Kept **inline, in place** so a cold reload reparses each badge at
 *   its original position.
 * - **embedded** references (a `codeg://embedded/…` display uri for path-less
 *   pasted bytes) are dropped: their real bytes-bearing block is appended
 *   separately by the host's `buildDraft`, so emitting their synthetic display
 *   link here would leak a uri the agent shouldn't see.
 * - **hard breaks** and paragraph boundaries become `\n`.
 *
 * The whole document serializes to a single text block, with every reference
 * sitting inline exactly where the sender placed it.
 */
export function docToPromptBlocks(editor: Editor): PromptInputBlock[] {
  const text = serializeDocToText(editor.state.doc).trim()
  return text ? [{ type: "text", text }] : []
}

/**
 * Map a leaf/atom node to its plain-text form. Shared by full-document send
 * serialization ({@link serializeDocToText}) and the host's selection-copy path,
 * so a copied selection reads back exactly like what is sent.
 *
 * - `reference` → {@link referenceToMarkdown}, except an embedded-attachment
 *   reference (its synthetic `codeg://embedded/…` uri must never surface on the
 *   SEND path), which contributes nothing — unless `keepEmbedded` is set, the
 *   DISPLAY path, which keeps it inline so the sender sees the attached badge.
 * - `hardBreak` → a newline.
 * - anything else (defensive) → nothing.
 *
 * Called by `textBetween` with a single argument (opts undefined → embedded
 * dropped), so the default send/copy behavior is unchanged.
 */
export function composerLeafText(
  leaf: ProseMirrorNode,
  opts?: { keepEmbedded?: boolean }
): string {
  if (leaf.type.name === "reference") {
    const attrs = leaf.attrs as ReferenceAttrs
    if (
      !opts?.keepEmbedded &&
      typeof attrs.uri === "string" &&
      isEmbeddedReferenceUri(attrs.uri)
    ) {
      return ""
    }
    return referenceToMarkdown(attrs)
  }
  if (leaf.type.name === "hardBreak") return "\n"
  if (leaf.type.name === "agentRuleSelection") {
    return isAgentRuleSelectionAttrs(leaf.attrs)
      ? serializeAgentRuleSelection(leaf.attrs)
      : ""
  }
  return ""
}

/**
 * Serialize a whole ProseMirror document to the plain-text prompt form (the SEND
 * wire text): text verbatim, reference badges to their inline token, hard breaks
 * and paragraph boundaries to `\n`, embedded-attachment badges dropped. Callers
 * `.trim()` the result.
 */
export function serializeDocToText(doc: ProseMirrorNode): string {
  return doc.textBetween(0, doc.content.size, "\n", composerLeafText)
}

/**
 * Like {@link serializeDocToText} but KEEPS embedded-attachment references inline
 * (as their `[label](codeg://embedded/…)` link). This is the DISPLAY form for
 * the queue chip / optimistic bubble: the sender must see the file they attached
 * (the transcript renders that link back into an inert file badge), even though
 * the SEND path drops it because its bytes travel as a separate block and the
 * agent must never receive the synthetic uri. The two forms differ ONLY on
 * embedded refs; for all other content they are identical.
 */
export function serializeDocToDisplayText(doc: ProseMirrorNode): string {
  return doc.textBetween(0, doc.content.size, "\n", (leaf) =>
    composerLeafText(leaf, { keepEmbedded: true })
  )
}
