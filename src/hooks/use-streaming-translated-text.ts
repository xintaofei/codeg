"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import {
  MAX_TRANSLATION_CHARS,
  STREAM_FAILURE_PAUSE_LIMIT,
  STREAM_FAILURE_RETRY_MS,
  STREAM_MAX_UNITS_PER_DISPATCH,
  STREAM_MIN_INTERVAL_MS,
  STREAM_MIN_NEW_CHARS,
  STREAM_PAUSE_COOLDOWN_MS,
  STREAM_TAIL_CHUNK_MAX_CHARS,
  STREAM_UNIT_RETRY_BASE_MS,
  STREAM_UNIT_RETRY_LIMIT,
  mergeUnit,
  splitStableUnits,
  tailChunksFor,
  type ContextReference,
} from "@/lib/translation"

import {
  requestNumberedGroup,
  requestTranslationDetailed,
  translationCacheKey,
  useTranslationSettingsSnapshot,
  DEFAULT_BATCH_CHARS,
  type TranslatedTextState,
} from "./use-translated-text"

/**
 * Incremental translation for a thinking block that may still be streaming.
 *
 * Only regions whose bytes can never change are ever requested: sealed units
 * (see `splitStableUnits`) plus fixed-width chunks of the growing tail (see
 * `tailChunksFor`) — a stream is append-only, so a prefix of the tail is as
 * final as a sealed paragraph, and without that a long single-paragraph
 * thought would translate nothing until the turn settled. Requests are paced
 * by STREAM_MIN_INTERVAL_MS / STREAM_MIN_NEW_CHARS so a fast-updating stream
 * turns into at most one batch per couple of seconds. When the turn settles,
 * whatever the translated chain has not covered is flushed once, and every
 * result lands in the shared content-addressed cache.
 */
export interface StreamingTranslatedTextParams {
  text: string
  isStreaming: boolean
  shouldLoad: boolean
  uiLocale: string
  blockKey: string
  enabled: boolean
  /**
   * Queue on the backend's fast lane. Reply prose streams through here too and
   * must never wait behind thinking-block backlog; settled thinking blocks
   * leave the default false.
   */
  priority?: boolean
}

/** A source region `[start, end)` whose bytes are final. */
interface Segment {
  start: number
  end: number
  text: string
}

/** The translation of one segment, stored under the segment's start offset. */
interface Piece {
  start: number
  end: number
  text: string
  /** The exact source slice the translation covers. A stored piece is only
   * restored when the current text still contains this slice verbatim —
   * `blockKey` is positional, so two mounted blocks can share one key, and
   * the source check is what keeps another block's pieces from ever
   * rendering here. */
  source: string
}

interface StreamingProgress {
  /**
   * Start offset → translated piece. Pieces need not form a contiguous chain:
   * the display renders every piece in order and fills the gaps with the raw
   * source, so a failed early chunk cannot hide later successes (a rate-
   * limited endpoint fails *some* of a burst, and under a strict quota the
   * early chunks are exactly the ones most likely to fail first).
   */
  pieces: ReadonlyMap<number, Piece>
}

/**
 * Pieces survive the component. The message list virtualizes: a block that
 * scrolls out of view unmounts, and its in-state pieces used to die with it —
 * scrolling back restarted the block from zero and, when the re-request hit a
 * rate limit, the translation (and its toggle) never came back. The settled
 * turn makes it worse: its parts are re-split into a progress renderer and an
 * answer renderer that EACH number from zero, so a fully-translated reply
 * lands on a different (and already-occupied) positional key the moment the
 * turn ends.
 *
 * The store is therefore keyed only for LRU bookkeeping and looked up BY
 * CONTENT: `findStoredPieces` scans every entry and keeps the pieces whose
 * stored source slice still matches the current text verbatim. A reused
 * positional key can never leak someone else's pieces (their sources don't
 * match), and a renumbered part finds its own translation wherever it moved.
 */
const PIECE_STORE_LIMIT = 200
const pieceStore = new Map<string, ReadonlyMap<number, Piece>>()

function findStoredPieces(text: string): Map<number, Piece> {
  let best: Map<number, Piece> = new Map()
  let bestCovered = 0
  for (const stored of pieceStore.values()) {
    let covered = 0
    const restored = new Map<number, Piece>()
    for (const [offset, piece] of stored) {
      if (
        piece.end <= text.length &&
        text.slice(piece.start, piece.end) === piece.source
      ) {
        restored.set(offset, piece)
        covered += piece.end - piece.start
      }
    }
    if (covered > bestCovered) {
      bestCovered = covered
      best = restored
    }
    // Deliberately NO delete-on-no-match: a block remounting at settle time
    // can mount a frame before the reparse fills its parts, so its text is
    // momentarily EMPTY — every piece fails `piece.end <= text.length`, and
    // deleting the entry there would destroy the block's own translation
    // (the exact "settled and the toggle vanished" flash). Dead entries cost
    // one bounded scan; they leave by LRU eviction, nothing else.
  }
  return best
}

function savePieces(
  blockKey: string,
  pieces: ReadonlyMap<number, Piece>
): void {
  pieceStore.delete(blockKey)
  pieceStore.set(blockKey, pieces)
  if (pieceStore.size > PIECE_STORE_LIMIT) {
    const oldest = pieceStore.keys().next().value
    if (oldest !== undefined) pieceStore.delete(oldest)
  }
}

/** Contiguous covered length from 0, ignoring pieces that outrun the text. */
function chainEnd(
  pieces: ReadonlyMap<number, Piece>,
  textLength: number
): number {
  let pos = 0
  for (;;) {
    const piece = pieces.get(pos)
    if (!piece || piece.end > textLength) return pos
    pos = piece.end
  }
}

/**
 * A clipped straddle shorter than this waits for the settle flush instead of
 * spending a request on a handful of characters (a sealed unit can overlap
 * tail chunks already dispatched into its region).
 */
const MIN_CLIP_CHARS = 80

/**
 * Every final region of `text`, in source order and contiguous from 0: the
 * sealed units, then the fixed-width chunks of the remainder. Chunking stops
 * where a fence is still open — half a code block must not reach the model
 * unmasked, so that region waits for the settle flush.
 *
 * `mergeUpTo` coalesces adjacent segments into spans no wider than it. While a
 * block streams, segments stay per-unit: granularity is what makes a
 * rate-limited endpoint's partial failures cheap. Once it settles the reader
 * wants the whole thing fast, and per-paragraph requests are the bottleneck —
 * a 13k-char reply is ~30 round trips, most of them under the endpoint's
 * concurrency gate. Merging back up to [`MAX_TRANSLATION_CHARS`] per request
 * (the same width the settled hook uses) turns that into a handful.
 */
function segmentsFor(text: string, mergeUpTo = 0): Segment[] {
  const { units, unitEndOffsets, tailStart, openFenceAt } =
    splitStableUnits(text)
  const segments: Segment[] = []
  let start = 0
  for (let index = 0; index < units.length; index += 1) {
    segments.push({ start, end: unitEndOffsets[index], text: units[index] })
    start = unitEndOffsets[index]
  }
  for (const chunk of tailChunksFor(
    text,
    tailStart,
    openFenceAt ?? text.length,
    STREAM_TAIL_CHUNK_MAX_CHARS
  )) {
    segments.push(chunk)
  }
  // The sub-chunk remainder below the last tail chunk is not final while the
  // text grows, so streaming never sends it — the settle flush does. Once
  // settled (the only time merging runs) it is as final as everything else
  // and must join the segments, or the tail rides the one-shot settle flush
  // and the merge saves no round trips.
  if (mergeUpTo > 0 && openFenceAt === null) {
    const chunkedTo =
      segments.length > 0 ? segments[segments.length - 1].end : tailStart
    const remainder = text.slice(chunkedTo)
    if (remainder.trim()) {
      segments.push({ start: chunkedTo, end: text.length, text: remainder })
    }
  }
  if (mergeUpTo <= 0) return segments

  const merged: Segment[] = []
  for (const segment of segments) {
    const last = merged[merged.length - 1]
    if (last && segment.end - last.start <= mergeUpTo) {
      last.end = segment.end
      last.text += segment.text
    } else {
      merged.push({ ...segment })
    }
  }
  return merged
}

/**
 * The contiguous run of work starting at `from`: segments beyond it, clipped
 * where the chain already stands mid-segment. A gap (coverage ending before
 * the next segment starts) stops the run — the settle flush converges it.
 * The batch fills up to `maxChars` of source text (an empty batch excepted —
 * a lone over-wide segment must still go out) and at most `maxUnits`
 * segments, keeping failure isolation at segment granularity.
 */
function batchFrom(
  segments: readonly Segment[],
  from: number,
  pieces: ReadonlyMap<number, Piece>,
  maxUnits: number,
  maxChars: number
): Segment[] {
  const batch: Segment[] = []
  let pos = from
  let batchChars = 0
  for (const segment of segments) {
    if (segment.end <= pos) continue
    // A retry pass after a rollback re-walks the same segments; skipping the
    // ones a piece already covers keeps the retry from spending the
    // endpoint's rate-limit budget re-requesting finished work while the
    // one failed chunk it needs waits behind it.
    const covered = pieces.get(segment.start)
    if (covered && covered.end >= segment.end) {
      pos = Math.max(pos, segment.end)
      continue
    }
    const start = Math.max(segment.start, pos)
    if (start > pos) break
    if (start > segment.start && segment.end - start < MIN_CLIP_CHARS) break
    const segChars = segment.end - start
    // 宽度封顶；空批次例外——单独的超宽段落也必须走得出去。
    if (batch.length > 0 && batchChars + segChars > maxChars) break
    batch.push({
      start,
      end: segment.end,
      text: segment.text.slice(start - segment.start),
    })
    batchChars += segChars
    pos = segment.end
    if (batch.length >= maxUnits) break
  }
  return batch
}

/**
 * Incremental states: IDLE until a region is final, THROTTLED between
 * dispatches, REQUESTING while a batch is out, PAUSED after
 * STREAM_FAILURE_PAUSE_LIMIT total failures, SETTLING once the turn ends,
 * DONE when the flush landed.
 */
export function useStreamingTranslatedText({
  text,
  isStreaming,
  shouldLoad,
  uiLocale,
  blockKey,
  enabled,
  priority = false,
}: StreamingTranslatedTextParams): TranslatedTextState {
  const settings = useTranslationSettingsSnapshot()
  // Per-unit segments while the reply streams (a rate-limited endpoint's
  // partial failures stay cheap); merged into request-sized spans once it
  // settles, where request count is the bottleneck.
  const segments = useMemo(
    () => segmentsFor(text, isStreaming ? 0 : MAX_TRANSLATION_CHARS),
    [text, isStreaming]
  )

  // Restored from the piece store: a remount (the virtualized list scrolling
  // this block out of view and back) picks up every piece already translated
  // instead of starting from zero. The lookup is content-based, so it also
  // survives the settled turn renumbering the parts.
  const [progress, setProgress] = useState<StreamingProgress>(() => ({
    pieces: findStoredPieces(text),
  }))
  const [originalKey, setOriginalKey] = useState<string | null>(null)
  /** The last failure reason any dispatch attempt reported. */
  const [lastError, setLastError] = useState<string | null>(null)

  const segmentsRef = useRef(segments)
  const progressRef = useRef(progress)
  const blockKeyRef = useRef(blockKey)
  /** The source offset dispatched batches have covered; never moves back. */
  const dispatchedEndRef = useRef(0)
  const lastDispatchAtRef = useRef(0)
  const lastDispatchCoveredRef = useRef(0)
  const timerRef = useRef<number | null>(null)
  const consecutiveFailuresRef = useRef(0)
  /** When the pause was last armed; the cool-down lets a paused block retry. */
  const lastFailureAtRef = useRef(0)
  /** The chain end the settle flush already covered; null until it runs. */
  const settledBoundaryRef = useRef<number | null>(null)
  /** Settle flushes failed since the last success; bounded, then give up. */
  const settledRetriesRef = useRef(0)
  const aliveRef = useRef(true)

  const active = enabled && shouldLoad

  const clearTimer = useCallback(() => {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }, [])

  // A NEW block identity restarts the machine — but "new" here can be a mere
  // key shift: the settled turn re-splits its parts into a progress renderer
  // and an answer renderer that EACH number from zero, so a fully-translated
  // reply lands on a different (and already-occupied) positional key the
  // moment the turn ends. Wiping state there threw the finished translation
  // away (and the toggle with it). The restore is content-based (see
  // findStoredPieces): pieces whose stored source matches the current text
  // come back wherever the key moved; genuinely different content matches
  // nothing and starts clean.
  const prevBlockKeyRef = useRef(blockKey)
  useEffect(() => {
    segmentsRef.current = segments
    progressRef.current = progress
    blockKeyRef.current = blockKey
  }, [segments, progress, blockKey])
  useEffect(() => {
    if (prevBlockKeyRef.current === blockKey) return
    prevBlockKeyRef.current = blockKey
    dispatchedEndRef.current = 0
    lastDispatchAtRef.current = 0
    lastDispatchCoveredRef.current = 0
    consecutiveFailuresRef.current = 0
    settledBoundaryRef.current = null
    settledRetriesRef.current = 0
    // The key changed: restore by content instead of dropping the user's
    // translation, and release the show-original choice — it belonged to the
    // previous key.
    setProgress({ pieces: findStoredPieces(text) })
    setOriginalKey(null)
    setLastError(null)
  }, [blockKey, text])

  // A block can mount a frame BEFORE the settle-time reparse fills its parts:
  // the initializer above then restored nothing (the text was empty or
  // partial), and without a retry the block would flash to raw, lose its
  // toggle, and wait for a whole fresh translation. Re-running the content
  // lookup as the text grows picks the stored pieces up incrementally — a
  // prefix of the final text validates exactly the pieces that cover it.
  useEffect(() => {
    if (progress.pieces.size > 0) return
    const restored = findStoredPieces(text)
    if (restored.size === 0) return
    setProgress({ pieces: restored })
  }, [text, progress.pieces])

  // Settle is the moment the text is final AND the parts may have been
  // re-split: re-validate what is on screen against the final bytes and keep
  // whichever covers more. Without this, a re-split that shifted the piece
  // layout strands stale pieces that break the display chain at offset 0 —
  // the whole block shows raw even though every piece is still in the store.
  useEffect(() => {
    if (isStreaming) return
    setProgress((prev) => {
      const restored = findStoredPieces(text)
      if (
        chainEnd(restored, text.length) <= chainEnd(prev.pieces, text.length)
      ) {
        return prev
      }
      return { pieces: restored }
    })
  }, [isStreaming, text])

  useEffect(() => {
    aliveRef.current = true
    return () => {
      aliveRef.current = false
      clearTimer()
    }
  }, [blockKey, clearTimer])

  useEffect(() => {
    const isCurrent = () => aliveRef.current && blockKeyRef.current === blockKey

    // Grouped-request width from the settings page; the same ceiling the
    // settled hook uses for its mergeUnitGroups batches.
    const batchWidth = settings.batchMaxChars ?? DEFAULT_BATCH_CHARS

    const scheduleRetry = (retry: () => void, delay: number) => {
      clearTimer()
      timerRef.current = window.setTimeout(() => {
        timerRef.current = null
        if (!isCurrent()) return
        retry()
      }, delay)
    }

    /** One segment, retried with backoff so a 429 blip cannot strand a line. */
    const requestSegmentWithRetry = async (
      segment: Segment,
      key: string,
      context?: ContextReference
    ): Promise<string | null> => {
      for (let attempt = 0; ; attempt += 1) {
        // The detailed variant so the failure reason survives for the
        // toggle's warning indicator.
        const attempt_ = await requestTranslationDetailed(
          segment.text,
          uiLocale,
          key,
          priority,
          undefined,
          undefined,
          context
        )
        if (
          attempt_.text !== null ||
          attempt >= STREAM_UNIT_RETRY_LIMIT ||
          !isCurrent()
        ) {
          if (attempt_.text === null && attempt_.error && isCurrent()) {
            setLastError(attempt_.error)
          }
          return attempt_.text
        }
        await new Promise((resolve) =>
          window.setTimeout(resolve, STREAM_UNIT_RETRY_BASE_MS * (attempt + 1))
        )
      }
    }

    const dispatchBatch = (from: number) => {
      const batch = batchFrom(
        segmentsRef.current,
        from,
        progressRef.current.pieces,
        STREAM_MAX_UNITS_PER_DISPATCH,
        batchWidth
      )
      if (batch.length === 0) return false
      const pos = batch[batch.length - 1].end
      dispatchedEndRef.current = Math.max(dispatchedEndRef.current, pos)
      lastDispatchAtRef.current = Date.now()
      lastDispatchCoveredRef.current = pos

      // Terminology consistency: the piece immediately before this batch
      // rides along (source + translation, tail-truncated) as a read-only
      // reference inside the same request body — no extra round trip. One
      // segment back is enough; history never accumulates.
      let context: ContextReference | undefined
      if (settings.carryContext) {
        const batchStart = batch[0].start
        let prev: Piece | undefined
        for (const piece of progressRef.current.pieces.values()) {
          if (piece.end <= batchStart && (!prev || piece.end > prev.end))
            prev = piece
        }
        if (prev) context = { source: prev.source, translation: prev.text }
      }

      const sent = batch.map((segment) => ({
        segment,
        key: translationCacheKey({
          blockKey,
          text: segment.text,
          uiLocale,
          settings,
        }),
      }))

      /** Store per-segment results, rolling the cursor back over failures. */
      const land = (results: (string | null)[]) => {
        if (!isCurrent()) return
        // Any landing clears the amber flag: the failure it reported is no
        // longer the newest fact about this block.
        if (results.some((value) => value !== null)) {
          setLastError(null)
        }
        setProgress((prev) => {
          const next = new Map(prev.pieces)
          sent.forEach(({ segment }, offset) => {
            const value = results[offset]
            if (value === null) return
            // A longer piece at the same start must not be overwritten by a
            // shorter one arriving later (an overlapping unit landing after
            // its tail chunk): that would drag the chain back and re-request
            // bytes that are already translated.
            const existing = next.get(segment.start)
            if (!existing || existing.end < segment.end) {
              next.set(segment.start, {
                start: segment.start,
                end: segment.end,
                text: value,
                source: segment.text,
              })
            }
          })
          savePieces(blockKey, next)
          return { pieces: next }
        })
        // Roll the dispatch cursor back to the first chunk that still has no
        // translation (all failed, or a burst where only some chunks made it
        // past the endpoint's rate limit). Without this the cursor marks the
        // failed region "covered" forever and the gap is never retried while
        // the stream runs — the live display strands everything from the
        // failed chunk onward until the settle flush.
        const firstFailed = sent.find(
          (_sent, offset) => results[offset] === null
        )
        if (firstFailed) {
          dispatchedEndRef.current = Math.min(
            dispatchedEndRef.current,
            firstFailed.segment.start
          )
        }
        if (results.every((value) => value === null)) {
          consecutiveFailuresRef.current += 1
          lastFailureAtRef.current = Date.now()
          // All-failed batches used to strand the block until the next text
          // flush; a bounded retry keeps it converging on a flaky endpoint.
          if (consecutiveFailuresRef.current < STREAM_FAILURE_PAUSE_LIMIT) {
            scheduleRetry(() => {
              if (consecutiveFailuresRef.current < STREAM_FAILURE_PAUSE_LIMIT) {
                dispatchBatch(from)
              }
            }, STREAM_FAILURE_RETRY_MS)
          }
          return
        }
        consecutiveFailuresRef.current = 0
      }

      // Multi-segment batches ride ONE numbered request: `[1] … [2] …` in,
      // per-segment translations back out, each judged by the same gates the
      // single-chunk path runs. A live reply converging through N paragraphs
      // costs one round trip per dispatch instead of N — under a strict RPM
      // quota that is the difference between keeping up and falling behind.
      // Any group failure (transport, unparseable reply, one bad segment)
      // falls back to the per-segment path below, where each piece stands
      // alone and the partial-failure economics are already proven.
      if (batch.length > 1) {
        void requestNumberedGroup(
          batch.map((segment) => segment.text),
          uiLocale,
          priority,
          undefined,
          context
        ).then((translations) => {
          if (translations) {
            land(translations)
            return
          }
          void Promise.all(
            sent.map(({ segment, key }) =>
              requestSegmentWithRetry(segment, key, context)
            )
          ).then(land)
        })
        return true
      }

      void Promise.all(
        sent.map(({ segment, key }) =>
          requestSegmentWithRetry(segment, key, context)
        )
      ).then(land)
      return true
    }

    // Settled convergence: request whatever the translated chain has not
    // covered as a single remainder. Cold-mounted old messages degrade to one
    // whole-block request, the same shape the settled hook would have made.
    //
    // One flush per boundary, not once per block: when a segment result that
    // was in flight at settle time lands afterwards, the chain moves past the
    // remainder's start and the re-request covers the new tail — the
    // overlapped region it refetches is content-addressed cache material.
    const flushSettled = () => {
      clearTimer()

      const covered = chainEnd(progress.pieces, text.length)
      if (settledBoundaryRef.current === covered) return
      settledBoundaryRef.current = covered

      const pending = text.slice(covered)
      if (!pending.trim()) return

      const key = translationCacheKey({
        blockKey,
        text: pending,
        uiLocale,
        settings,
      })
      void requestTranslationDetailed(pending, uiLocale, key, priority).then(
        (attempt) => {
          if (!isCurrent()) return
          if (attempt.text === null) {
            // A failed settle flush used to pin the boundary and leave the
            // tail raw forever; release it so the bounded retry can converge.
            // The backoff widens each attempt (4s → 12s → 36s): a rate-
            // limited endpoint needs a minute of slack to serve the
            // remainder, and the flat 4s spent all three attempts inside one
            // saturated window.
            if (attempt.error) setLastError(attempt.error)
            settledBoundaryRef.current = null
            if (settledRetriesRef.current < STREAM_FAILURE_PAUSE_LIMIT) {
              settledRetriesRef.current += 1
              scheduleRetry(
                flushSettled,
                STREAM_FAILURE_RETRY_MS *
                  Math.pow(3, settledRetriesRef.current - 1)
              )
            }
            return
          }
          settledRetriesRef.current = 0
          setLastError(null)
          setProgress((prev) => {
            const next = new Map(prev.pieces)
            next.set(covered, {
              start: covered,
              end: text.length,
              text: attempt.text ?? "",
              source: text.slice(covered),
            })
            savePieces(blockKey, next)
            return { pieces: next }
          })
        }
      )
    }

    if (!active) {
      // Owned by the settled hook (or not yet near the viewport): stand down.
      return clearTimer
    }

    if (!isStreaming) {
      // Keep the normal batch machinery running after settle instead of
      // flushing the whole remainder as one request. The single flush was
      // fragile twice over: under the relay's rate limit it failed wholesale
      // (three backoff attempts, then the tail stayed raw forever), and one
      // model omission inside the big remainder erased every uncovered
      // paragraph from the display at once. Batched segments isolate each
      // failure, and requestTranslation splits whatever is left internally.
      if (consecutiveFailuresRef.current < STREAM_FAILURE_PAUSE_LIMIT) {
        const settledFrom = Math.max(
          chainEnd(progress.pieces, text.length),
          dispatchedEndRef.current
        )
        if (dispatchBatch(settledFrom)) return clearTimer
      }
      flushSettled()
      return clearTimer
    }

    const from = Math.max(
      chainEnd(progress.pieces, text.length),
      dispatchedEndRef.current
    )
    // Dispatch pacing lives on the backend's adaptive limiter now; the
    // frontend keeps only its own batching floors (don't ask more often than
    // the reader can read, don't ship a batch thinner than this).
    const minIntervalMs = STREAM_MIN_INTERVAL_MS
    const minNewChars = STREAM_MIN_NEW_CHARS
    const frontierEnd =
      segments.length > 0 ? segments[segments.length - 1].end : 0
    const newChars = frontierEnd - lastDispatchCoveredRef.current
    const elapsed = Date.now() - lastDispatchAtRef.current
    const due = elapsed >= minIntervalMs || newChars >= minNewChars

    if (!due) {
      // THROTTLED: arm (or keep) the interval timer. Returning without
      // clearing is deliberate — the timeout just scheduled is the machine's
      // only path forward until the next text flush re-runs this effect.
      if (timerRef.current === null) {
        const wait = Math.max(
          lastDispatchAtRef.current + minIntervalMs - Date.now(),
          0
        )
        timerRef.current = window.setTimeout(() => {
          timerRef.current = null
          if (!isCurrent()) return
          if (consecutiveFailuresRef.current < STREAM_FAILURE_PAUSE_LIMIT) {
            dispatchBatch(
              Math.max(
                chainEnd(progressRef.current.pieces, text.length),
                dispatchedEndRef.current
              )
            )
          }
        }, wait)
      }
      return
    }

    if (consecutiveFailuresRef.current >= STREAM_FAILURE_PAUSE_LIMIT) {
      // PAUSED — but not for the whole stream. A rate-limited endpoint
      // refills its quota over tens of seconds, so after the cool-down one
      // batch is let through; if it fails again the pause re-arms with a
      // fresh cool-down. The settle flush still converges the block when
      // streaming ends.
      if (Date.now() - lastFailureAtRef.current < STREAM_PAUSE_COOLDOWN_MS) {
        return clearTimer
      }
      consecutiveFailuresRef.current = STREAM_FAILURE_PAUSE_LIMIT - 1
    }

    dispatchBatch(from)
    return clearTimer
  }, [
    active,
    blockKey,
    clearTimer,
    isStreaming,
    priority,
    progress,
    segments,
    settings,
    text,
    uiLocale,
  ])

  // Contiguous-chain assembly: the translation replaces the source from the
  // top and only the still-untranslated tail shows raw. Rendering pieces out
  // of order (gap-filling) was tried and rejected: with a rate-limited
  // endpoint the early chunks fail first, and the reader got Chinese and
  // English interleaved mid-document. The chain never skips — a failed chunk
  // is retried in place (see the dispatch-cursor rollback) until the
  // translation is whole again.
  const display = useMemo(() => {
    let out = ""
    let pos = 0
    for (;;) {
      const piece = progress.pieces.get(pos)
      if (!piece || piece.end > text.length) break
      out += mergeUnit(text.slice(pos, piece.end), piece.text)
      pos = piece.end
    }
    return out + text.slice(pos)
  }, [progress.pieces, text])

  const hasTranslation = progress.pieces.size > 0
  const showingOriginal = originalKey === blockKey
  const showOriginal = useCallback(() => setOriginalKey(blockKey), [blockKey])
  const showTranslation = useCallback(() => setOriginalKey(null), [])

  return {
    display: showingOriginal ? text : display,
    hasTranslation,
    isTranslated: hasTranslation && !showingOriginal,
    hasErrors: lastError !== null,
    errorHint: lastError,
    showOriginal,
    showTranslation,
  }
}
