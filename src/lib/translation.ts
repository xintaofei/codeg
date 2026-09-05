const encoder = new TextEncoder()

export const MAX_TRANSLATION_CHARS = 3000
export const MAX_PARSE_BYTES = 256 * 1024

/**
 * Streaming (incremental thinking) translation pacing. A slow endpoint needs
 * several seconds per request, so the floor is an interval rather than a
 * debounce: whichever of "enough time passed" / "enough new text arrived"
 * comes first wins. 1.5 s is the reader's patience threshold — slower than
 * this and the live translation visibly lags the stream — while still leaving
 * most of a shared per-minute quota to generation.
 */
export const STREAM_MIN_INTERVAL_MS = 1500
export const STREAM_MIN_NEW_CHARS = 150
/** Consecutive all-failed dispatches after which incremental work pauses. */
export const STREAM_FAILURE_PAUSE_LIMIT = 3
/**
 * How long a paused block waits before trying again. A rate-limited endpoint
 * refills its quota over tens of seconds, so a full stop until the turn
 * settles strands the live translation for minutes; after this cool-down one
 * batch is let through and the pause re-arms if it fails again.
 */
export const STREAM_PAUSE_COOLDOWN_MS = 30_000
/**
 * At most this many sealed units go out in one incremental dispatch. Five
 * keeps a burst inside the endpoint's concurrency gate while letting a
 * fast-streaming reply translate several paragraphs per window; larger bursts
 * only manufacture 429s — the failures cost quota too.
 */
export const STREAM_MAX_UNITS_PER_DISPATCH = 5
/** Wait before re-dispatching after a wholly failed batch. */
export const STREAM_FAILURE_RETRY_MS = 4000
/** Per-unit retries inside one dispatch: 429 blips must not strand a line. */
export const STREAM_UNIT_RETRY_LIMIT = 2
/** Base of the per-unit retry backoff (attempt N waits N × this). */
export const STREAM_UNIT_RETRY_BASE_MS = 3000

export function utf8ByteLength(text: string): number {
  return encoder.encode(text).byteLength
}

/**
 * Split a masked message into request-sized pieces without changing a byte.
 * Paragraph boundaries win; a single over-long paragraph falls back to a
 * Unicode code-point boundary so an emoji cannot be split into invalid UTF-16.
 */
export function splitForTranslation(text: string): string[] | null {
  if (utf8ByteLength(text) > MAX_PARSE_BYTES) return null
  if (text.length <= MAX_TRANSLATION_CHARS) return [text]

  const chunks: string[] = []
  let rest = text
  while (rest.length > MAX_TRANSLATION_CHARS) {
    let end = MAX_TRANSLATION_CHARS
    const paragraphEnd = rest.lastIndexOf("\n\n", end - 1)
    if (paragraphEnd >= 0) end = paragraphEnd + 2
    if (paragraphEnd < 0) {
      const sentenceEnd = sentenceChunkEnd(rest, 0, 400, end)
      if (sentenceEnd !== null && sentenceEnd > 0) end = sentenceEnd
    }

    // A boundary through the middle of a fenced block sends half a fence to
    // the model unmasked (the fence regex cannot match its broken half), and
    // the translation comes back with the code translated — the exact
    // byte-fidelity failure the mask exists to prevent.
    end = adjustBoundaryOutOfFence(rest, 0, end)

    // A UTF-16 slice between a surrogate pair would turn one code point into
    // two replacement characters in the outbound JSON request.
    if (
      end < rest.length &&
      end > 0 &&
      /[\uD800-\uDBFF]/.test(rest[end - 1]) &&
      /[\uDC00-\uDFFF]/.test(rest[end])
    ) {
      end += 1
    }

    chunks.push(rest.slice(0, end))
    rest = rest.slice(end)
  }
  if (rest) chunks.push(rest)
  return chunks
}

/**
 * Nudge a chunk boundary out of any fenced code block it cuts through.
 *
 * Splitting splitters (both [`splitForTranslation`] and `tailChunksFor`) pick
 * byte boundaries; a boundary that lands between a fence's opening and
 * closing lines leaves each chunk holding half a fence, which the mask's
 * fence regex cannot pair — the raw code rides to the model as prose and the
 * "translation" comes back with the block's content translated.
 *
 * Returns the boundary unchanged when it is fence-free. Otherwise, when the
 * fence closes later in the text, the boundary extends past the closing line
 * (a slightly wider chunk beats a broken one); when the fence never closes
 * (malformed markdown, or the text simply ends inside it), the boundary
 * retreats to the fence's opening line — unless that line opens at or before
 * the chunk start, where retreating would loop forever and the caller keeps
 * the original boundary.
 */
export function adjustBoundaryOutOfFence(
  text: string,
  start: number,
  boundary: number
): number {
  // Pass 1: walk the lines before the boundary with the same fence rules
  // `splitStableUnits` applies, and learn whether the boundary sits inside a
  // fence (and where that fence opened).
  let fence: { ch: string; len: number; openedAt: number } | null = null
  let index = start
  while (index < boundary && index < text.length) {
    const newline = text.indexOf("\n", index)
    const lineEnd = newline === -1 ? text.length : newline
    const raw = text.slice(index, lineEnd)
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw
    const match = FENCE_LINE.exec(line)
    if (match) {
      const marker = match[1]
      const ch = marker[0]
      const rest = match[2]
      if (!fence) {
        if (ch === "~" || !rest.includes("`")) {
          fence = { ch, len: marker.length, openedAt: index }
        }
      } else if (
        ch === fence.ch &&
        marker.length >= fence.len &&
        rest.trim() === ""
      ) {
        fence = null
      }
    }
    index = newline === -1 ? text.length : newline + 1
  }
  if (!fence) return boundary

  // Pass 2: find where this fence closes and extend the boundary past it.
  let closeEnd = -1
  index = fence.openedAt
  while (index < text.length) {
    const newline = text.indexOf("\n", index)
    const lineEnd = newline === -1 ? text.length : newline
    const raw = text.slice(index, lineEnd)
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw
    const match = FENCE_LINE.exec(line)
    if (
      index > fence.openedAt &&
      match &&
      match[1][0] === fence.ch &&
      match[1].length >= fence.len &&
      match[2].trim() === ""
    ) {
      closeEnd = newline === -1 ? text.length : newline + 1
      break
    }
    index = newline === -1 ? text.length : newline + 1
  }
  if (closeEnd !== -1) return closeEnd
  if (fence.openedAt > start) return fence.openedAt
  return boundary
}

export function joinTranslated(parts: readonly string[]): string {
  return parts.join("")
}

/**
 * Greedy coalescing of adjacent units into numbered-request groups. Each group
 * becomes ONE outbound request carrying `[1] …, [2] …` segments, so a reply of
 * thirty short paragraphs converges in a handful of round trips instead of
 * thirty — the difference between converging under a strict RPM quota and
 * fighting it. A unit wider than `maxChars` forms its own group (equivalent to
 * today's one-request-per-chunk path); groups never straddle the ceiling.
 */
export function mergeUnitGroups(
  units: readonly string[],
  maxChars: number
): number[][] {
  const groups: number[][] = []
  let current: number[] = []
  let currentChars = 0
  for (let index = 0; index < units.length; index += 1) {
    const chars = units[index].length
    if (current.length > 0 && currentChars + chars > maxChars) {
      groups.push(current)
      current = []
      currentChars = 0
    }
    current.push(index)
    currentChars += chars
  }
  if (current.length > 0) groups.push(current)
  return groups
}

/**
 * The wire shape a numbered request sends: each segment under an `[n]`
 * heading, blank line between them. The blank lines give the model a clear
 * frame to translate *inside* each segment without crossing boundaries.
 */
export function buildNumberedRequest(segments: readonly string[]): string {
  return segments
    .map((segment, index) => `[${index + 1}] ${segment.trim()}`)
    .join("\n\n")
}

/**
 * Read a numbered reply back into its per-segment translations.
 *
 * Strict by design: every line group must open with the exact `[n]` header,
 * the numbers must be 1..count in order, and there must be exactly `count` of
 * them. Anything else — a merged pair, a dropped tail, a chatty preamble —
 * returns `null` and the caller falls back to per-segment requests, where the
 * established per-chunk gates judge each piece alone.
 */
export function parseNumberedTranslation(
  reply: string,
  count: number
): string[] | null {
  const parts = reply.split(/^\[(\d+)\][ \t]/m)
  // split yields: [preamble, "1", body1, "2", body2, ...]
  if (parts[0].trim() !== "") return null
  const found: string[] = []
  for (let index = 1; index < parts.length; index += 2) {
    const number = Number(parts[index])
    if (number !== found.length + 1) return null
    found.push(parts[index + 1] ?? "")
  }
  if (found.length !== count) return null
  return found.map((part) => part.trim())
}

/** The blank-line run a unit or chunk ends with — its separator in the source. */
export const UNIT_SEPARATOR = /(?:\r?\n)+$/

/**
 * Re-attach the source separator instead of trusting the model to have kept
 * the trailing blank line: a dropped one would glue two paragraphs together.
 * Endpoints trim every reply, so the separator a splitter cut at has to be
 * put back from the source side.
 */
export function mergeUnit(unit: string, translated: string): string {
  return translated.trimEnd() + (UNIT_SEPARATOR.exec(unit)?.[0] ?? "")
}

/**
 * Whether `translated` looks like an echo or a refusal rather than a
 * translation: the target language is CJK, the source carries real prose, and
 * the reply contains **zero** target-script characters. Both shapes were
 * served by a real relay — an English source "translated" into English
 * unchanged, and a bare "I am not able to comply with this request." — and
 * the length gate cannot see either (an echo is 1:1, a refusal is shorter).
 *
 * The prose bar (≥30 Latin letters after masked placeholders are stripped)
 * keeps short fragments exempt: a legit translation of a two-word chunk can
 * be longer than the source in *characters* while a code-only chunk masks
 * down to nothing and never had prose to refuse. Latin-script targets have no
 * equivalent test and are never gated.
 */
export function missingTargetScript(
  chunk: string,
  translated: string,
  targetLang: string
): boolean {
  const lang = targetLang.trim().toLowerCase()
  if (
    !(lang === "zh" || lang.startsWith("zh-") || lang === "ja" || lang === "ko")
  ) {
    return false
  }
  const prose = chunk.replace(/\[\s*\[?_?CBLK\d+\s*\]\s*\]?/g, "")
  if ((prose.match(/[A-Za-z]/g) ?? []).length < 30) return false
  return !/[\u3400-\u4dbf\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af]/.test(
    translated
  )
}

/**
 * Digit runs of two or more digits that the source prose carries and the
 * translation dropped. A model that answers the text instead of translating
 * it routinely sheds the concrete numbers ("Git 2.34" → "Git 较新版本");
 * a faithful translation keeps them verbatim in any language codeg ships.
 * Only runs of ≥2 digits count — a lone "v5"-style digit is too noisy — and
 * masked regions (code, URLs, math) are excluded up front, so their numbers
 * never reach this gate. A false positive costs one discarded attempt and a
 * retry; a missed invention poisons the cache for every later render.
 */
export function missingSourceNumbers(
  chunk: string,
  translated: string
): boolean {
  const prose = chunk.replace(/\[\s*\[?_?CBLK\d+\s*\]\s*\]?/g, "")
  const runs = prose.match(/\d{2,}/g) ?? []
  return runs.some((run) => !translated.includes(run))
}

export interface TailChunk {
  /** Inclusive start offset of the chunk in the source text. */
  start: number
  /** Exclusive end offset of the chunk in the source text. */
  end: number
  text: string
}

/**
 * Streaming tail-chunk width bounds. The floor keeps a payload from shattering
 * into single sentences; the ceiling keeps live translation fresh. The old
 * fixed 600-char width cut long paragraphs mid-sentence, and a model handed
 * half a sentence can only translate it broken — the main source of fragment
 * quality complaints.
 */
export const STREAM_TAIL_CHUNK_MIN_CHARS = 600
export const STREAM_TAIL_CHUNK_MAX_CHARS = 1500

const STRONG_SENTENCE_END = new Set("。！？!?…".split(""))
const WEAK_SENTENCE_END = new Set(";；:：,，、".split(""))
/** 句末标点后跟着的收尾符号（引号、括号），一并吃进切点。 */
const CLOSING_MARKS = new Set("」』）)】》〉\"'’”".split(""))
/** 会跨句存活的括号对（引号不参与：中英文引号开闭同形，计数不可靠，
 * 且引号极少真的横跨一个 600+ 字符窗口的两个句界）。 */
const OPEN_BRACKETS = new Set("（(【〔［「《〈".split(""))
const CLOSE_BRACKETS = new Set("）)】〕］」》〉".split(""))

/** 段 [start, end) 内悬空的开括号数：>0 表示切点落在某个未闭合
 * 括号内部，切开会把半个引用送进请求。 */
function unclosedBrackets(text: string, start: number, end: number): number {
  let depth = 0
  for (let i = start; i < end; i += 1) {
    if (OPEN_BRACKETS.has(text[i])) depth += 1
    else if (CLOSE_BRACKETS.has(text[i]) && depth > 0) depth -= 1
  }
  return depth
}

/**
 * 在 [start + minChars, start + maxChars] 窗口内找最后一个安全的
 * 切点，按 强句末 → 弱标点 → 空白 退级；同级取最后一个。只读窗口内
 * 已收到的字节，所以流式增长时同一文本产生的切点稳定不变。返回互斥
 * end 偏移，null 表示窗口内没有任何可用边界（调用方硬切）。
 */
export function sentenceChunkEnd(
  text: string,
  start: number,
  minChars: number,
  maxChars: number
): number | null {
  const hardEnd = Math.min(start + maxChars, text.length)
  const minEnd = Math.min(start + minChars, hardEnd)

  for (const ends of [STRONG_SENTENCE_END, WEAK_SENTENCE_END]) {
    let best: number | null = null
    for (let i = minEnd; i < hardEnd; i += 1) {
      if (!ends.has(text[i])) continue
      let end = i + 1
      while (end < hardEnd && CLOSING_MARKS.has(text[end])) end += 1
      while (end < hardEnd && (text[end] === " " || text[end] === "\t"))
        end += 1
      if (unclosedBrackets(text, start, end) > 0) continue
      best = end
    }
    if (best !== null) return best
  }

  let best: number | null = null
  for (let i = minEnd; i < hardEnd; i += 1) {
    if (" \n\t".includes(text[i])) best = i + 1
  }
  return best
}

/** Settled 路径窗口更宽，句界下限可以更小。 */
const TAIL_MIN_SENTENCE_CHARS = 400

/**
 * Fixed-width pieces of a streaming tail whose bytes can never change.
 *
 * `splitStableUnits` only seals at blank lines, so a thinking block that
 * streams as one long paragraph seals nothing and its live translation would
 * wait for the turn to settle. The tail is append-only, so any fixed prefix
 * of it is just as final as a sealed unit: this cuts it into request-sized
 * chunks so the streaming machine can translate it without waiting for a
 * paragraph break that may never come.
 *
 * Boundaries prefer a sentence end, found by [`sentenceChunkEnd`] inside the
 * [TAIL_MIN_SENTENCE_CHARS, chunkSize] window (whole sentences translate far
 * better than mid-sentence fragments), falling back to any whitespace there,
 * and only hitting the hard width when the window holds no boundary at all.
 * They never split a surrogate pair. Both steps look only at bytes already
 * received, so the chunks a given text produces stay identical as the tail
 * grows. `limit` (default: the end of the text) is where chunking must stop —
 * the start of a still-open fence, whose half-block would otherwise reach the
 * model unmasked. `chunkSize` (default: [`MAX_TRANSLATION_CHARS`]) is the
 * hard width; the streaming machine passes [`STREAM_TAIL_CHUNK_MAX_CHARS`] to
 * keep live translation flowing before a full chunk has accumulated.
 */
export function tailChunksFor(
  text: string,
  tailStart: number,
  limit: number = text.length,
  chunkSize: number = MAX_TRANSLATION_CHARS
): TailChunk[] {
  const chunks: TailChunk[] = []
  let start = tailStart
  while (limit - start >= chunkSize) {
    let end = start + chunkSize
    const sentenceEnd = sentenceChunkEnd(
      text,
      start,
      TAIL_MIN_SENTENCE_CHARS,
      chunkSize
    )
    if (sentenceEnd !== null && sentenceEnd > start) end = sentenceEnd
    // The boundary must not cut a (closed) fence in half: half a fence masks
    // to nothing and the model translates the code. Extension past the close
    // is safe — fences inside [tailStart, limit) are closed before limit, so
    // the adjusted end never passes it.
    end = Math.min(limit, adjustBoundaryOutOfFence(text, start, end))
    if (end <= start) break
    if (
      end < text.length &&
      /[\uD800-\uDBFF]/.test(text[end - 1]) &&
      /[\uDC00-\uDFFF]/.test(text[end])
    ) {
      end += 1
    }
    chunks.push({ start, end, text: text.slice(start, end) })
    start = end
  }
  return chunks
}

/**
 * A prefix of `text` that a stream can no longer rewrite, cut into units.
 *
 * Incremental translation of a growing text may only send regions whose bytes
 * are final: masking is positional, so re-masking a block whose fence later
 * closes renumbers every placeholder and invalidates the whole cache. A blank
 * line outside a fence is that guarantee — nothing after it can change what
 * came before.
 */
export interface StableUnits {
  /**
   * Sealed slices in source order. Each unit *includes* the blank-line
   * separator that closed it, so `units.join("") + text.slice(tailStart)`
   * reproduces `text` byte for byte.
   */
  units: string[]
  /** Exclusive end offset of each unit in `text`. */
  unitEndOffsets: number[]
  /** Start of the still-growing remainder (`text.slice(tailStart)`). */
  tailStart: number
  /**
   * Start of the line that opened a fence still unclosed at the end of the
   * text, or `null` when no fence is open. A tail chunk cut past this point
   * would carry half a code block whose placeholder never closes, so fixed-
   * width chunking must stop there.
   */
  openFenceAt: number | null
}

/** An opening fence keeps its info string; a closing one may not have any. */
const FENCE_LINE = /^ {0,3}(`{3,}|~{3,})(.*)$/

/**
 * An ATX heading line (up to three leading spaces, 1-6 `#`, then a space or
 * the line end — CommonMark's shape).
 */
const HEADING_LINE = /^ {0,3}#{1,6}(?:[ \t].*)?$/

/**
 * Scan `text` once, sealing a unit at every blank-line run that is not inside a
 * fenced code block — and directly before an ATX heading line. A heading that
 * follows its previous paragraph without a blank line would otherwise ride in
 * that paragraph's unit, and a model asked to translate a mixed unit likes to
 * silently DROP the part already written in the target language (a Chinese
 * preamble ahead of an English heading, say) — the paragraph vanishes from the
 * translation while its piece still counts as covered. Sealing before the
 * heading gives the preamble its own request, where an omission is at worst an
 * empty reply, and an empty reply is refused (it must never erase source).
 *
 * A fence that spans blank lines keeps its block whole, and an unclosed fence
 * makes everything from its opening line unstable — a closing fence arriving
 * later would otherwise re-shuffle the units already sent.
 *
 * Blank-only regions are never sealed on their own; they merge into the next
 * unit so no request is ever spent on whitespace. A line holding only spaces is
 * deliberately not a separator: it does not match `(?:\r?\n){2,}`, the same
 * rule `splitForTranslation` and Markdown itself apply.
 */
export function splitStableUnits(text: string): StableUnits {
  const units: string[] = []
  const unitEndOffsets: number[] = []
  let fence: { ch: string; len: number; at: number } | null = null
  let sealedAt = 0
  let index = 0

  while (index < text.length) {
    const newline = text.indexOf("\n", index)
    const lineEnd = newline === -1 ? text.length : newline
    const nextIndex = newline === -1 ? text.length : newline + 1
    const raw = text.slice(index, lineEnd)
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw

    if (
      !fence &&
      index > sealedAt &&
      HEADING_LINE.test(line) &&
      text.slice(sealedAt, index).trim() !== ""
    ) {
      // The heading line itself stays unsealed (it may still be streaming);
      // everything before it is final and becomes a unit of its own.
      units.push(text.slice(sealedAt, index))
      unitEndOffsets.push(index)
      sealedAt = index
    }

    if (line.length === 0 && newline !== -1 && !fence) {
      // Consume the whole run so `\n\n\n\n` seals once, exactly where the
      // `(?:\r?\n){2,}` match would end.
      let runEnd = nextIndex
      while (runEnd < text.length) {
        const runNewline = text.indexOf("\n", runEnd)
        if (runNewline === -1) break
        const runRaw = text.slice(runEnd, runNewline)
        if (runRaw !== "" && runRaw !== "\r") break
        runEnd = runNewline + 1
      }
      if (text.slice(sealedAt, runEnd).trim() !== "") {
        units.push(text.slice(sealedAt, runEnd))
        unitEndOffsets.push(runEnd)
        sealedAt = runEnd
      }
      index = runEnd
      continue
    }

    const fenceMatch = FENCE_LINE.exec(line)
    if (fenceMatch) {
      const marker = fenceMatch[1]
      const ch = marker[0]
      const rest = fenceMatch[2]
      if (!fence) {
        // A backtick fence's info string may not contain a backtick, so
        // ```` ```a`b ```` opens nothing and stays prose.
        if (ch === "~" || !rest.includes("`")) {
          fence = { ch, len: marker.length, at: index }
        }
      } else if (
        ch === fence.ch &&
        marker.length >= fence.len &&
        rest.trim() === ""
      ) {
        fence = null
      }
    }

    index = nextIndex
  }

  return {
    units,
    unitEndOffsets,
    tailStart: sealedAt,
    openFenceAt: fence ? fence.at : null,
  }
}

export function shouldTranslate({
  isStreaming,
  text,
  isUser,
  enabled,
}: {
  /** Must mean this individual message has not settled (`!completed` today). */
  isStreaming: boolean
  text: string
  isUser: boolean
  enabled: boolean
}): boolean {
  if (!enabled || isUser || isStreaming || !text.trim()) return false
  return utf8ByteLength(text) <= MAX_PARSE_BYTES
}

/**
 * The canonical placeholder: `[[CBLK<n>]]`, optionally carrying the `_`
 * collision prefix the mask adds when the prose already contained `[[CBLK`.
 * Pure ASCII, so no relay can strip it and the model can copy it verbatim —
 * the prompt shows this exact shape.
 */
const TRANSLATION_PLACEHOLDER = /\[\[_?CBLK\d+\]\]/g

/**
 * A model that loses, reorders, or renumbers an opaque placeholder would make
 * restore either leak a token or put protected bytes in the wrong place. Such
 * output is discarded and the renderer keeps the original.
 */
export function hasSameTranslationPlaceholders(
  source: string,
  translated: string
): boolean {
  return (
    JSON.stringify(source.match(TRANSLATION_PLACEHOLDER) ?? []) ===
    JSON.stringify(translated.match(TRANSLATION_PLACEHOLDER) ?? [])
  )
}

/**
 * Loose token shapes a model may produce while imitating the sentinel: stray
 * whitespace inside the brackets ("[ [CBLK0] ]") or a dropped outer bracket
 * pair ("[CBLK0]"). Each is rewritten to the canonical token so the strict
 * sequence comparison below can judge it; anything genuinely mangled — a
 * renamed body, a wrong digit, a dropped token — still fails that comparison
 * and the chunk is discarded. The lookarounds keep an already-canonical
 * `[[CBLK0]]` from matching the single-bracket rule (its inner bracket pair).
 */
export function canonicalizeTranslationPlaceholders(
  translated: string
): string {
  return translated
    .replace(/\[\s*\[(_?)CBLK(\d+)\s*\]\s*\]/g, "[[$1CBLK$2]]")
    .replace(/(?<!\[)\[(_?)CBLK(\d+)\](?!\])/g, "[[$1CBLK$2]]")
}

/**
 * Recover a translation whose placeholders came back slightly deformed. The
 * token sequence still has to match the source exactly — same tokens, same
 * order — for the canonicalized text to be accepted; otherwise the chunk is
 * discarded and the renderer keeps the original.
 */
export function realignTranslationPlaceholders(
  source: string,
  translated: string
): string | null {
  const canonical = canonicalizeTranslationPlaceholders(translated)
  if (!hasSameTranslationPlaceholders(source, canonical)) return null
  return canonical
}
