/**
 * Placeholder masking for the pre-parse Markdown rewrites this directory runs
 * before handing text to Streamdown (`normalizeMathDelimiters` in message.tsx,
 * `escapeWindowsPathSeparators` in windows-path-escape.ts).
 *
 * Both rewrites edit the *source string*, so both must first take the regions
 * whose bytes are already literal — fenced blocks and inline code — out of
 * harm's way. Sharing the pattern and the placeholder here keeps the two from
 * drifting apart: a region one rewrite protects and the other does not would
 * corrupt exactly the content users are most likely to paste verbatim.
 */

/**
 * Fenced code blocks and inline code spans. Fences are matched before inline
 * spans so a ``` block containing single backticks is captured whole. The
 * inline alternative excludes LF, so a stray backtick cannot swallow the rest
 * of a message.
 */
export const CODE_SPANS = /`{3,}[\s\S]*?`{3,}|~{3,}[\s\S]*?~{3,}|`[^`\n]+`/g

/**
 * Literal regions a translation model must never see. Code remains first so a
 * URL or formula inside a fenced block is captured as part of that block,
 * rather than creating nested placeholders the one-pass restore cannot decode.
 */
export const TRANSLATABLE_MASK = new RegExp(
  [
    CODE_SPANS.source,
    String.raw`\]\((?:\\.|[^)\n])+\)`,
    String.raw`\$\$[\s\S]*?\$\$`,
    String.raw`\$(?!\$)(?:\\.|[^$\n])+\$`,
    String.raw`<[^>\n]+>`,
  ].join("|"),
  "g"
)

/**
 * The byte shape a placeholder takes. Two shapes serve two very different
 * callers:
 *
 * - NUL-delimited for the in-process Markdown rewrites: the token never
 *   leaves the app, and a control character cannot collide with anything a
 *   rewrite or the prose itself might produce.
 * - `[[CBLK<n>]]` for text sent to the translation endpoint. The token rides
 *   inside the model's context, and relays routinely sanitize control
 *   characters out of requests — an ASCII token survives every relay, and the
 *   model can copy it verbatim because the system prompt can show its exact
 *   shape. (The NUL shape once leaked as literal "\0CBLK0\0" text for exactly
 *   this reason.)
 */
export interface MaskSentinel {
  /** Wrap `prefix` + `index` into a placeholder token. */
  wrap: (prefix: string, index: number) => string
  /** Regex matching that sentinel's tokens, capturing the index. */
  matcher: (prefix: string) => RegExp
  /** Whether `text` already carries a token with this prefix. */
  collides: (text: string, prefix: string) => boolean
}

export const NUL_SENTINEL: MaskSentinel = {
  wrap: (prefix, index) => `\0${prefix}${index}\0`,
  matcher: (prefix) => new RegExp(`\\0${prefix}(\\d+)\\0`, "g"),
  collides: (text, prefix) => text.includes(`\0${prefix}`),
}

export const BRACKET_SENTINEL: MaskSentinel = {
  wrap: (prefix, index) => `[[${prefix}${index}]]`,
  matcher: (prefix) => new RegExp(`\\[\\[${prefix}(\\d+)\\]\\]`, "g"),
  collides: (text, prefix) => text.includes(`[[${prefix}`),
}

export interface MaskedSource {
  /** `text` with every `pattern` match replaced by an opaque placeholder. */
  masked: string
  /** Put the masked regions back, verbatim, once the rewrite is done. */
  restore: (rewritten: string) => string
}

/**
 * Replace every match of `pattern` (which MUST be a global regex) with an
 * opaque placeholder, returning the masked text plus the inverse operation.
 */
export function maskLiteralSpans(
  text: string,
  pattern: RegExp = CODE_SPANS,
  sentinel: MaskSentinel = NUL_SENTINEL
): MaskedSource {
  const saved: string[] = []
  let prefix = "CBLK"
  while (sentinel.collides(text, prefix)) prefix = `_${prefix}`
  const placeholder = sentinel.matcher(prefix)
  const masked = text.replace(pattern, (match) => {
    saved.push(match)
    return sentinel.wrap(prefix, saved.length - 1)
  })
  return {
    masked,
    restore: (rewritten: string) =>
      rewritten.replace(
        placeholder,
        (_m, index: string) => saved[Number(index)] ?? _m
      ),
  }
}

/** Translation-bound text: the ASCII sentinel the endpoint can copy back. */
export function maskForTranslation(text: string): MaskedSource {
  return maskLiteralSpans(text, TRANSLATABLE_MASK, BRACKET_SENTINEL)
}

/**
 * Identity mask for text that is NOT Markdown source. A text selection read
 * back from the DOM (`selection.toString()`) has no fences or backticks left,
 * so every pattern in [`TRANSLATABLE_MASK`] can only mangle real prose there:
 * the `<[^>\n]+>` rule swallows a Git conflict hunk (`<<<<<<< HEAD … then
 * >>>>>>> branch-name`) as a fake "tag", and the `$...$` rule eats money
 * amounts ("$100 and $200"). Pass-through keeps the whole selection
 * translatable; the endpoint's own sanity gates still apply to the reply.
 */
export function maskPlainText(text: string): MaskedSource {
  return { masked: text, restore: (rewritten) => rewritten }
}
