/**
 * Parsing helpers shared by the live `AskQuestionCard` (interactive) and the
 * historical `AskQuestionResultCard` (read-only, in the message stream).
 *
 * The codeg-mcp `ask_user_question` tool serializes into a session transcript
 * as a generic tool call. The input is the raw `{ questions: [...] }` JSON the
 * agent sent. The output is the tool result the agent CLI persisted: in
 * practice that is the companion's structured `{ answers, declined }` envelope
 * (each answer's `selected` is already a string array — see `render_ask_result`
 * in `src-tauri/src/acp/delegation/companion.rs`), which is what we parse first.
 * We also fall back to the companion's human-readable result text for any CLI
 * that persists `content` instead of `structuredContent`.
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
  /** Codex `request_user_input` marks secret answers (API keys etc.) with
   *  `isSecret`; the read-only card masks them. */
  isSecret: boolean
  /** Codex `request_user_input` gives each question a stable `id`; its chosen
   *  answer envelope is keyed by that id rather than the question text (unlike
   *  the codeg-mcp ask, which the card matches by header+question). */
  id?: string
}

export interface AskQuestionAnswer {
  header: string
  question: string
  /** The user's raw picks: each entry is one offered option label or a free-text
   *  "Other" answer (empty when nothing was chosen). Partition against the
   *  question's options with `matchSelections`. */
  selected: string[]
  /** Present for codex `request_user_input`, whose answers are keyed by the
   *  question `id` rather than its header/question text; the card matches on it
   *  when set, otherwise on the header+question signature. */
  id?: string
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

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object"
    ? (value as Record<string, unknown>)
    : null
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
  const root = parsed as Record<string, unknown>
  // codex-acp 1.0.0 wraps every MCP call's input as
  // `{ server, tool, arguments: { questions } }` (see createMcpToolCallUpdate in
  // codex-acp/src/CodexToolCallMapper.ts). Fall back to the nested `arguments`
  // when the questions aren't at the top level (the bare shape Claude / the
  // history parser persist).
  let questions = root.questions
  if (!Array.isArray(questions)) {
    questions = asRecord(root.arguments)?.questions
  }
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
      isSecret: obj.isSecret === true || obj.is_secret === true,
      id: typeof obj.id === "string" && obj.id ? obj.id : undefined,
    })
  }
  return out
}

/** The companion's marker for an answered-but-empty selection (English, not localized). */
const NO_SELECTION = "(no selection)"
const HEADER_LINE_RE = /^\s*\d+\.\s*\[([^\]]*)\]\s*(.*)$/
const SELECTED_LINE_RE = /^\s*→\s*(.*)$/

/**
 * codex persists an MCP tool result in its rollout transcript's
 * `function_call_output.output` as `Wall time: <n> seconds\nOutput:\n<body>`,
 * where `<body>` is the actual result (here the `{ answers, declined }` JSON).
 * Strip that wrapper so the body underneath is reachable. Anything without the
 * marker (the live ACP path, Claude, the bare companion shapes) is returned
 * unchanged.
 */
function stripCodexOutputWrapper(text: string): string {
  const match = text.match(/^Wall time:[^\n]*\r?\nOutput:\r?\n([\s\S]*)$/)
  return match ? match[1] : text
}

function parseAnswers(raw: unknown): AskQuestionAnswer[] {
  if (!Array.isArray(raw)) return []
  const out: AskQuestionAnswer[] = []
  for (const item of raw) {
    const obj = asRecord(item)
    if (!obj) continue
    const selected = Array.isArray(obj.selected)
      ? obj.selected.filter((x): x is string => typeof x === "string")
      : []
    out.push({
      header: asString(obj.header),
      question: asString(obj.question),
      selected,
    })
  }
  return out
}

/**
 * codex `request_user_input` records its answers keyed by question id rather
 * than as the codeg-mcp array envelope:
 *   { "answers": { "<questionId>": { "answers": ["label", ...] } } }
 * Return one answer per id (carrying that id so the card can match it against
 * the question's own id), or `null` when `answers` isn't the object map — so a
 * codeg-mcp / Claude result (whose `answers` is an ARRAY) never lands here.
 */
function parseCodexAnswers(
  source: Record<string, unknown> | null | undefined
): AskQuestionAnswer[] | null {
  if (!source) return null
  const map = source.answers
  if (!map || typeof map !== "object" || Array.isArray(map)) return null
  const out: AskQuestionAnswer[] = []
  for (const [id, entry] of Object.entries(map as Record<string, unknown>)) {
    const raw = asRecord(entry)?.answers
    const selected = Array.isArray(raw)
      ? raw.filter((x): x is string => typeof x === "string")
      : []
    out.push({ id, header: "", question: "", selected })
  }
  return out.length > 0 ? out : null
}

/**
 * Parse the structured `{ answers, declined }` envelope the agent CLI persists
 * for the tool result (the companion's `structuredContent`). It may sit at the
 * top level or nested under `structuredContent`. Also recognizes codex's
 * object-keyed `request_user_input` envelope (see `parseCodexAnswers`). Returns
 * `null` when `output` is neither, so the text fallback can take over.
 */
function parseOutcomeJson(output: string): AskQuestionOutcome | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(output)
  } catch {
    return null
  }
  const top = asRecord(parsed)
  if (!top) return null
  // Resolve an answer envelope from a record: the record itself when it carries
  // `answers`/`declined`, otherwise its `structuredContent`.
  const envelopeOf = (
    r: Record<string, unknown> | null
  ): Record<string, unknown> | null => {
    if (!r) return null
    if (Array.isArray(r.answers) || typeof r.declined === "boolean") return r
    return asRecord(r.structuredContent)
  }
  // Prefer the bare top-level shape (Claude / the history parser) first, so a
  // valid top-level envelope is never shadowed by an unrelated `result` key.
  // codex's live ACP path wraps the MCP result as `{ result, error }`, where
  // `result` is the CallToolResult (`{ content, structuredContent }`) — sometimes
  // tagged again under an `Ok` serde variant — so fall through those layers when
  // the top level yields nothing.
  const result = asRecord(top.result)
  const resultOk = result ? (asRecord(result.Ok) ?? asRecord(result.ok)) : null
  const env = envelopeOf(top) ?? envelopeOf(result) ?? envelopeOf(resultOk)
  if (
    env &&
    (Array.isArray(env.answers) || typeof env.declined === "boolean")
  ) {
    if (env.declined === true) return { declined: true, answers: [] }
    return { declined: false, answers: parseAnswers(env.answers) }
  }

  // codex `request_user_input`: object-keyed answers. Checked after the
  // array-shaped envelope above so a codeg-mcp result never falls through here.
  const codex =
    parseCodexAnswers(top) ??
    parseCodexAnswers(asRecord(top.structuredContent)) ??
    parseCodexAnswers(result) ??
    parseCodexAnswers(resultOk)
  if (codex) return { declined: false, answers: codex }

  return null
}

/**
 * Reconstruct the answered/declined outcome from the persisted tool result.
 *
 * Primary shape — the structured envelope the CLI stores verbatim:
 *   {"answers":[{"header":"…","question":"…","selected":["A","B"]}],"declined":false}
 *
 * Fallback shape — the companion's human-readable text, for any CLI that keeps
 * `content` instead of `structuredContent` (see `render_ask_result`):
 *   "The user dismissed the question(s) …"  (declined)
 *   "The user answered your question(s):\n1. [Header] Question\n   → a, b\n…"
 *
 * Returns `null` when there is no output yet (the call is still in flight). In
 * the fallback, selections are split on ", " (lossy for a label containing a
 * comma — the structured envelope keeps such a label intact as one array entry).
 */
export function parseAskQuestionOutcome(
  output: string | null | undefined
): AskQuestionOutcome | null {
  if (!output || !output.trim()) return null

  // codex wraps the result as `Wall time: <n>s\nOutput:\n<body>` in its rollout;
  // strip it so both the JSON and text paths below see the real payload.
  const body = stripCodexOutputWrapper(output)

  const fromJson = parseOutcomeJson(body)
  if (fromJson) return fromJson

  if (/\bdismissed the question/i.test(body)) {
    return { declined: true, answers: [] }
  }

  const answers: AskQuestionAnswer[] = []
  let current: AskQuestionAnswer | null = null
  for (const line of body.split(/\r?\n/)) {
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
    const selectedLine = line.match(SELECTED_LINE_RE)
    if (selectedLine && current) {
      const joined = selectedLine[1].trim()
      current.selected =
        joined && joined !== NO_SELECTION ? joined.split(", ") : []
      current = null
    }
  }
  return { declined: false, answers }
}

/**
 * Partition the user's raw picks into the offered option labels they chose
 * (`selected`) and any free-text "Other" answers (`other`), order-preserving.
 * Each pick is already a whole value (one array entry), so a label that itself
 * contains a comma matches cleanly — no fragile text splitting required.
 */
export function matchSelections(
  values: string[],
  optionLabels: string[]
): { selected: string[]; other: string[] } {
  const labels = new Set(optionLabels.filter(Boolean))
  const selected: string[] = []
  const other: string[] = []
  for (const raw of values) {
    const value = raw.trim()
    if (!value || value === NO_SELECTION) continue
    if (labels.has(value)) selected.push(value)
    else other.push(value)
  }
  return { selected, other }
}
