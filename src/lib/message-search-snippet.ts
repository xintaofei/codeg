/** One run of a message-search snippet, highlighted or not. */
export interface SnippetPart {
  text: string
  marked: boolean
}

const MARKER = /(\[\[mark\]\]|\[\[\/mark\]\])/

/**
 * Split a message-search snippet into text runs. The backend wraps every
 * matched term in `[[mark]]…[[/mark]]` — plain markers rather than HTML — so
 * the highlight is rendered as elements and the message text is never parsed
 * as markup.
 */
export function splitSnippet(snippet: string): SnippetPart[] {
  const parts: SnippetPart[] = []
  let marked = false
  for (const piece of snippet.split(MARKER)) {
    if (piece === "[[mark]]") marked = true
    else if (piece === "[[/mark]]") marked = false
    else if (piece) parts.push({ text: piece, marked })
  }
  return parts
}
