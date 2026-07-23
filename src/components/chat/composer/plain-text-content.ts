import type { JSONContent } from "@tiptap/core"

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
 * Returns the inline content to insert (from `text/plain`, `\n` → hardBreak) when
 * the clipboard carries an *external* `text/html` fragment, or `null` to let
 * ProseMirror handle the paste with its default behavior.
 *
 * Why this exists: the composer schema has no Link mark (see
 * {@link "./editor-config".buildComposerExtensions}). Browsers put a rich
 * `<a href="URL">Page Title</a>` fragment on the clipboard when a URL is copied
 * from the address bar; ProseMirror's default paste prefers `text/html`, drops
 * the href (the mark isn't in the schema), and keeps the anchor **text** — so a
 * copied URL pastes as the page's `<title>` instead of the URL. Forcing
 * `text/plain` for that *external* fragment fixes it, but must leave these to
 * ProseMirror (return `null`):
 * - No `text/html` at all — a pure `text/plain` paste is already correct.
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
export function decidePastedPlainText(
  snapshot: ClipboardTextSnapshot
): JSONContent[] | null {
  // Only an HTML payload can mislead the plain-text schema; a text/plain-only
  // clipboard already pastes correctly, so defer to ProseMirror.
  if (!snapshot.html) return null
  // Copied from within a ProseMirror editor: defer so its native HTML round-trip
  // restores structure/hard breaks/badges exactly (see the doc comment).
  if (snapshot.html.includes("data-pm-slice")) return null
  // Defensive: reference badge HTML lacking the slice wrapper still defers so the
  // badge round-trips instead of collapsing to its token.
  if (snapshot.html.includes("data-reference")) return null
  // External rich fragment: insert its plain-text flavor verbatim (never the
  // HTML). Nothing sensible to insert when there is no text/plain, so defer.
  if (!snapshot.text) return null
  return textToInlineContent(snapshot.text)
}
