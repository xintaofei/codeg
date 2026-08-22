/**
 * Label for an in-position image card.
 *
 * Codex image generation hardcodes the English title "Image generation"
 * (codex-acp PR #271). Codeg also routes ANY image-bearing tool (Read of a
 * PNG, a page screenshot, a fetched resource) through that same card, and
 * the card used to print "Image generation" even when the tool already had
 * a real name. Keep the dedicated copy only for actual generation; otherwise
 * use the tool title, URL slug, or filename the agent already knew.
 */

const IMAGE_GENERATION_TITLE = "image generation"

export function isImageGenerationTitle(
  title: string | null | undefined
): boolean {
  return (title ?? "").trim().toLowerCase() === IMAGE_GENERATION_TITLE
}

function lastPathSegment(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed) return ""
  try {
    if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) {
      const url = new URL(trimmed)
      const path = url.pathname.replace(/\/+$/, "")
      const leaf = path.split("/").filter(Boolean).pop()
      return decodeURIComponent(leaf || url.hostname)
    }
  } catch {
    /* not a URL */
  }
  const leaf = trimmed.split(/[\\/]/).filter(Boolean).pop() ?? trimmed
  return leaf
}

function humanizeSegment(segment: string): string {
  const withoutExt = segment.replace(/\.[a-z0-9]{1,8}$/i, "")
  const words = withoutExt.replace(/[-_]+/g, " ").trim()
  if (!words) return segment
  return words.replace(/\b\w/g, (c) => c.toUpperCase())
}

function stringField(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null
}

/** Filename or URL from a tool's JSON input preview. */
export function pathFromToolInput(
  input: string | null | undefined
): string | null {
  if (!input) return null
  try {
    const parsed: unknown = JSON.parse(input)
    if (!parsed || typeof parsed !== "object") return null
    const obj = parsed as Record<string, unknown>
    return (
      stringField(obj.file_path) ||
      stringField(obj.path) ||
      stringField(obj.filename) ||
      stringField(obj.url) ||
      stringField(obj.uri)
    )
  } catch {
    return null
  }
}

export function imageCardLabel(opts: {
  title?: string | null
  toolName?: string | null
  input?: string | null
}): string | null {
  const title = opts.title?.trim() || null
  if (title && !isImageGenerationTitle(title)) return title

  const fromInput = pathFromToolInput(opts.input ?? null)
  if (fromInput) {
    const segment = lastPathSegment(fromInput)
    if (segment) return humanizeSegment(segment)
  }

  const toolName = opts.toolName?.trim() || null
  if (toolName && !isImageGenerationTitle(toolName)) return toolName

  return null
}
