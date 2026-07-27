import type { JSONContent } from "@tiptap/core"

import { parseUserMessageSegments } from "@/components/message/user-message-segments"

import { referenceToMarkdown } from "./reference-text"
import { isEmbeddedReferenceUri } from "./reference-uri"
import type { ReferenceAttrs } from "./types"

/**
 * Convert a plain-text string into Tiptap inline content: literal text with each
 * `\n` turned into a `hardBreak` node. The plain-text composer schema has no code
 * block to hold a literal newline, so line breaks are hard breaks — which
 * {@link "./to-prompt-blocks".serializeDocToText} maps back to `\n`, so the text
 * round-trips. An empty string yields an empty array.
 *
 * Used wherever the host seeds the composer from plain text (drafts, quick
 * messages, expert/office prompt templates, injected content) now that no
 * Markdown parser is loaded.
 */
export function textToInlineContent(text: string): JSONContent[] {
  if (!text) return []
  const out: JSONContent[] = []
  const lines = text.split("\n")
  lines.forEach((line, index) => {
    if (index > 0) out.push({ type: "hardBreak" })
    // A ProseMirror text node may not be empty, so a blank line contributes only
    // its hardBreak (two adjacent breaks = one blank line).
    if (line.length > 0) out.push({ type: "text", text: line })
  })
  return out
}

/**
 * A whole document (one paragraph) holding {@link textToInlineContent}. Used to
 * replace the composer content from a plain-text string.
 */
export function textToDoc(text: string): JSONContent {
  return {
    type: "doc",
    content: [{ type: "paragraph", content: textToInlineContent(text) }],
  }
}

/**
 * A badge that would be silently DROPPED on send: an embedded-attachment
 * display uri whose bytes only ever live in the sender's out-of-band payload
 * map (`composerLeafText` omits it). Pasted text can carry such a link (copied
 * from a queue chip's display text), but hydrating it would turn visible text
 * into nothing on send — so it stays literal.
 */
function isSendDroppedReference(attrs: ReferenceAttrs): boolean {
  return typeof attrs.uri === "string" && isEmbeddedReferenceUri(attrs.uri)
}

/**
 * Parse pasted plain text back into badge-hydrated inline content: the same
 * wire format the transcript renders ({@link parseUserMessageSegments} —
 * `[label](file:·codeg:…)` links and bare `/cmd`·`$skill` tokens) becomes
 * reference nodes, the prose between them literal text with `\n` → hardBreak.
 * The hydrated badges re-serialize (via `referenceToMarkdown`) to exactly the
 * text that was pasted, so send output is unchanged — only the composer now
 * shows the same badges the sent message would.
 *
 * Returns null when nothing hydrates (no reference in the text), so callers
 * can leave a plain paste to ProseMirror's default handling.
 */
export function textToHydratedInlineContent(
  text: string
): JSONContent[] | null {
  if (!text) return null
  const segments = parseUserMessageSegments(text)
  const hydratable = segments.some(
    (segment) =>
      segment.kind === "reference" && !isSendDroppedReference(segment.attrs)
  )
  if (!hydratable) return null
  const out: JSONContent[] = []
  for (const segment of segments) {
    if (segment.kind === "text") {
      out.push(...textToInlineContent(segment.text))
    } else if (isSendDroppedReference(segment.attrs)) {
      out.push({ type: "text", text: referenceToMarkdown(segment.attrs) })
    } else {
      out.push({ type: "reference", attrs: segment.attrs })
    }
  }
  return out
}

/** The two clipboard flavors the paste decision looks at. */
export interface ClipboardTextSnapshot {
  /** `text/html` payload (empty string when the clipboard has none). */
  html: string
  /** `text/plain` payload (empty string when the clipboard has none). */
  text: string
}

/**
 * Decide how the plain-text composer should paste a clipboard's text.
 *
 * Returns the inline content to insert — the `text/plain` flavor with `\n` →
 * hardBreak and any serialized references hydrated back into badges
 * ({@link textToHydratedInlineContent}) — or `null` to let ProseMirror handle
 * the paste with its default behavior.
 *
 * Two reasons to take over:
 * - The clipboard carries an *external* `text/html` fragment. The composer
 *   schema has no Link mark (see
 *   {@link "./editor-config".buildComposerExtensions}); browsers put a rich
 *   `<a href="URL">Page Title</a>` fragment on the clipboard when a URL is
 *   copied from the address bar, and ProseMirror's default paste prefers
 *   `text/html`, drops the href, and keeps the anchor **text** — so a copied
 *   URL would paste as the page's `<title>`. Force the `text/plain` flavor.
 * - A pure `text/plain` paste whose text contains serialized references
 *   (`[label](file:·codeg:…)` links, `/cmd`·`$skill` tokens). Left to
 *   ProseMirror they insert as literal text that only turns into badges after
 *   sending; hydrating on paste shows the same badges immediately. Plain text
 *   with no references stays with ProseMirror (`null`) — its default multi-line
 *   handling is already correct.
 *
 * These must always defer to ProseMirror (return `null`):
 * - HTML copied from within a ProseMirror editor (this composer), which
 *   `serializeForClipboard` tags with a `data-pm-slice` marker. Its native
 *   round-trip must win: it restores paragraphs, hard breaks, and reference
 *   badges exactly. Forcing `text/plain` here would corrupt content — the
 *   clipboard text drops hard breaks (they serialize to `""`) and widens
 *   paragraph gaps into blank lines (block separator `"\n\n"` vs the composer's
 *   `"\n"`), so a copied two-line message would paste as one line or gain a
 *   blank line.
 * - HTML carrying our reference badges (`<span data-reference>`) even without a
 *   slice wrapper — a badge must never downgrade to its plain-text token.
 */
export function decidePastedContent(
  snapshot: ClipboardTextSnapshot
): JSONContent[] | null {
  // Copied from within a ProseMirror editor: defer so its native HTML round-trip
  // restores structure/hard breaks/badges exactly (see the doc comment).
  if (snapshot.html.includes("data-pm-slice")) return null
  // Defensive: reference badge HTML lacking the slice wrapper still defers so the
  // badge round-trips instead of collapsing to its token.
  if (snapshot.html.includes("data-reference")) return null
  // Nothing sensible to insert without a text/plain flavor, so defer.
  if (!snapshot.text) return null
  const hydrated = textToHydratedInlineContent(snapshot.text)
  // An external rich fragment must insert its plain-text flavor even when
  // nothing hydrates (never the HTML); a plain-only clipboard without
  // references keeps ProseMirror's default paste.
  if (snapshot.html) return hydrated ?? textToInlineContent(snapshot.text)
  return hydrated
}
