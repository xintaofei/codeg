/**
 * Parsing helpers shared by the live `AskQuestionCard` (interactive) and the
 * historical `AskQuestionResultCard` (read-only, in the message stream).
 *
 * The codeg-mcp `ask_user_question` tool serializes into a session transcript
 * as a generic tool call: the input is the raw `{ questions: [...] }` JSON the
 * agent sent, and the output is the human-readable text the companion renders
 * back (`render_ask_result` in `src-tauri/src/acp/delegation/companion.rs`).
 * Neither carries the structured `{ answers, declined }` envelope once
 * persisted, so the read-only card reconstructs the Q&A from these two strings.
 */

export interface AskQuestionOption {
  label: string
  description: string
}

export interface AskQuestion {
  question: string
  header: string
  /** The wire field is `multiSelect` (camelCase); we also accept `multi_select`. */
  multiSelect: boolean
  options: AskQuestionOption[]
}

export interface AskQuestionAnswer {
  header: string
  question: string
  /** Selected labels and any free-text "Other", merged (matches the backend). */
  selected: string[]
}

export interface AskQuestionOutcome {
  declined: boolean
  answers: AskQuestionAnswer[]
}

/**
 * Strip a trailing " (Recommended)" so it can render as a badge while the
 * underlying value keeps the agent's original label verbatim. Shared so the
 * live and historical cards present recommendations identically.
 */
export function splitRecommended(label: string): {
  text: string
  recommended: boolean
} {
  const m = label.match(/^(.*?)\s*\(recommended\)\s*$/i)
  const text = m?.[1].trim()
  // Only treat "(Recommended)" as a suffix when real text precedes it — a bare
  // "(Recommended)" label keeps its literal text rather than rendering empty.
  return text
    ? { text, recommended: true }
    : { text: label, recommended: false }
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : ""
}

function parseOptions(raw: unknown): AskQuestionOption[] {
  if (!Array.isArray(raw)) return []
  const out: AskQuestionOption[] = []
  for (const item of raw) {
    if (!item || typeof item !== "object") continue
    const obj = item as Record<string, unknown>
    const label = asString(obj.label)
    // An option with no label carries no meaning to display; drop it.
    if (!label) continue
    out.push({ label, description: asString(obj.description) })
  }
  return out
}

/**
 * Parse the `ask_user_question` tool input (the raw `{ questions: [...] }` JSON
 * the agent sent). Tolerant of partial/streaming input and missing fields —
 * returns `[]` rather than throwing so callers can fall back gracefully.
 */
export function parseAskQuestionInput(
  input: string | null | undefined
): AskQuestion[] {
  if (!input) return []
  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    return []
  }
  if (!parsed || typeof parsed !== "object") return []
  const questions = (parsed as Record<string, unknown>).questions
  if (!Array.isArray(questions)) return []

  const out: AskQuestion[] = []
  for (const item of questions) {
    if (!item || typeof item !== "object") continue
    const obj = item as Record<string, unknown>
    const options = parseOptions(obj.options)
    const question = asString(obj.question)
    // An entry with neither prompt text nor options is empty noise; skip it.
    if (!question && options.length === 0) continue
    out.push({
      question,
      header: asString(obj.header),
      multiSelect: obj.multiSelect === true || obj.multi_select === true,
      options,
    })
  }
  return out
}

/** The companion's marker for an answered-but-empty selection (English, not localized). */
const NO_SELECTION = "(no selection)"
const HEADER_LINE_RE = /^\s*\d+\.\s*\[([^\]]*)\]\s*(.*)$/
const SELECTED_LINE_RE = /^\s*→\s*(.*)$/

/**
 * Parse the companion's human-readable result text back into a structured
 * outcome. Two shapes (see `render_ask_result`):
 *
 *   - declined: "The user dismissed the question(s) …"
 *   - answered: "The user answered your question(s):\n1. [Header] Question\n   → a, b\n…"
 *
 * Returns `null` when there is no output yet (the call is still in flight).
 * Selections are split on ", "; a label that itself contains ", " may
 * over-split, but the card matches tokens against the known option labels so
 * such a token simply surfaces as an "Other" chip rather than a wrong highlight.
 */
export function parseAskQuestionOutcome(
  output: string | null | undefined
): AskQuestionOutcome | null {
  if (!output || !output.trim()) return null
  if (/\bdismissed the question/i.test(output)) {
    return { declined: true, answers: [] }
  }

  const answers: AskQuestionAnswer[] = []
  let current: AskQuestionAnswer | null = null
  for (const line of output.split(/\r?\n/)) {
    const header = line.match(HEADER_LINE_RE)
    if (header) {
      current = {
        header: header[1].trim(),
        question: header[2].trim(),
        selected: [],
      }
      answers.push(current)
      continue
    }
    const selected = line.match(SELECTED_LINE_RE)
    if (selected && current) {
      const joined = selected[1].trim()
      current.selected =
        joined && joined !== NO_SELECTION
          ? joined
              .split(", ")
              .map((s) => s.trim())
              .filter(Boolean)
          : []
      current = null
    }
  }
  return { declined: false, answers }
}
