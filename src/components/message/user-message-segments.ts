import { parseCodegReferenceUri } from "@/components/chat/composer/reference-uri"
import type { ReferenceAttrs } from "@/components/chat/composer/types"
import { INVOCATION_TOKEN_RE } from "@/lib/invocation-token"
import {
  tokenizeReferenceLinks,
  unescapeReferenceLabel,
} from "@/lib/reference-link"

/**
 * One render unit of a user message: a run of literal prose, or a resolved
 * reference to show as an inline badge.
 */
export type UserMessageSegment =
  | { kind: "text"; text: string }
  | { kind: "reference"; attrs: ReferenceAttrs }

/**
 * Only these schemes become badges. A `[label](https://…)` a user typed is NOT a
 * reference — it stays literal text (the composer is plain-text; genuine badges
 * are always inserted via the `@`·`/`·`$` menus and serialize to `file:`/`codeg:`).
 */
const REFERENCE_SCHEME = /^(?:file:|codeg:)/i

/** Strip a CommonMark angle-bracket destination (`<uri>`) to the bare uri, so the
 *  scheme test and `parseCodegReferenceUri` see a clean value (mirrors the reload
 *  adapter's unwrap in `ai-elements-adapter.handleMarkdownLink`). */
function unwrapDestination(destination: string): string {
  const trimmed = destination.trim()
  return trimmed.startsWith("<") && trimmed.endsWith(">")
    ? trimmed.slice(1, -1).trim()
    : trimmed
}

/**
 * Split a plain-prose run into literal text and bare `/slug`·`$slug` skill
 * badges (same {@link INVOCATION_TOKEN_RE} the composer's `/`·`$` triggers use).
 * The badge label drops the literal `/`·`$` prefix (the parser strips it) so a
 * sent invocation token renders identically to the composer's inline badge,
 * which shows the bare command/skill name.
 */
function pushProseSegments(value: string, out: UserMessageSegment[]): void {
  INVOCATION_TOKEN_RE.lastIndex = 0
  let lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = INVOCATION_TOKEN_RE.exec(value)) !== null) {
    const token = match[2]
    const tokenStart = match.index + match[1].length
    if (tokenStart > lastIndex) {
      out.push({ kind: "text", text: value.slice(lastIndex, tokenStart) })
    }
    const slug = token.slice(1)
    // Resolve through the shared reference parser, which strips the leading
    // `/`·`$` so the badge label is the bare slug (`build`, `deploy`) — matching
    // the composer's inline command/skill badge.
    const attrs = parseCodegReferenceUri(
      `codeg://skill/${encodeURIComponent(slug)}`,
      token
    )
    out.push(
      attrs ? { kind: "reference", attrs } : { kind: "text", text: token }
    )
    lastIndex = INVOCATION_TOKEN_RE.lastIndex
  }
  if (lastIndex < value.length) {
    out.push({ kind: "text", text: value.slice(lastIndex) })
  }
}

/**
 * Parse a sent user-message text string into ordered render segments: literal
 * prose (line breaks preserved by the renderer) interleaved with the five
 * built-in reference badges. Pure — no React, so it round-trips against
 * {@link "@/components/chat/composer/reference-text".referenceToMarkdown} in tests.
 *
 * Two passes over the shared wire format (unchanged by this feature):
 *  1. {@link tokenizeReferenceLinks} splits `[label](dest)` links from prose. A
 *     link whose (angle-unwrapped) destination is a `file:`/`codeg:` reference
 *     becomes a badge via {@link parseCodegReferenceUri}; any other link stays
 *     literal (rendered as its raw `[label](dest)` source).
 *  2. The prose between links is scanned for bare `/slug`·`$slug` skill tokens.
 *
 * Deliberately NOT Markdown: headings/bold/lists/code/tables in the text stay
 * literal, matching the plain-text composer.
 */
export function parseUserMessageSegments(text: string): UserMessageSegment[] {
  const out: UserMessageSegment[] = []
  for (const token of tokenizeReferenceLinks(text)) {
    if (token.type === "link") {
      const destination = unwrapDestination(token.destination)
      if (REFERENCE_SCHEME.test(destination)) {
        const attrs = parseCodegReferenceUri(
          destination,
          unescapeReferenceLabel(token.label)
        )
        if (attrs) {
          out.push({ kind: "reference", attrs })
          continue
        }
      }
      // Not a recognized reference link: keep its raw source verbatim.
      out.push({ kind: "text", text: token.raw })
      continue
    }
    pushProseSegments(token.value, out)
  }
  return out
}
