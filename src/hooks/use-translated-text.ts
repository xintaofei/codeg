"use client"

import { useCallback, useEffect, useMemo, useState } from "react"

import {
  maskForTranslation,
  type MaskedSource,
} from "@/components/ai-elements/markdown-mask"
import { getTranslationSettings, translateTexts } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  buildContextPrefix,
  buildNumberedRequest,
  buildTranslateBody,
  hasSameTranslationPlaceholders,
  mergeUnit,
  mergeUnitGroups,
  missingSourceNumbers,
  missingTargetScript,
  parseNumberedTranslation,
  realignTranslationPlaceholders,
  retryConstraintLine,
  shouldTranslate,
  splitForTranslation,
  stripTranslateEnvelope,
  type ContextReference,
} from "@/lib/translation"
import type { TranslationSettings } from "@/lib/types"

const DISABLED_SETTINGS: TranslationSettings = {
  enabled: false,
  providers: [],
  baseUrl: "",
  apiKey: "",
  model: "",
  targetLang: null,
  translateThinking: false,
  apiFormat: "auto",
  selectionTranslate: true,
  selectionTargetLang: null,
  toggleAlwaysVisible: false,
  batchMaxChars: null,
  carryContext: false,
}

/** The grouped-request width when the user left the setting empty. */
export const DEFAULT_BATCH_CHARS = 3000

/**
 * Where the literal-span mask comes from: Markdown message source uses the
 * full pattern set, while a DOM text selection (plain, markup-free) passes
 * through untouched — see [`maskPlainText`].
 */
type MaskedSourceFactory = (text: string) => MaskedSource

/**
 * Frontend cap on remembered translations. The backend LRU (2000) governs what
 * it will serve; this only keeps the renderer's Map from growing with the
 * session. FIFO is enough — an evicted entry costs one backend lookup, which
 * usually hits its cache anyway.
 */
const MAX_TRANSLATED_ENTRIES = 500

let cachedSettings: TranslationSettings | null = null
let settingsInflight: Promise<TranslationSettings> | null = null
let settingsGeneration = 0
const settingsListeners = new Set<(settings: TranslationSettings) => void>()
const translatedCache = new Map<string, string>()
const translationInflight = new Map<string, Promise<TranslationAttempt>>()

/** Insert, dropping the oldest entry once over the cap. */
function rememberTranslation(key: string, text: string): void {
  translatedCache.delete(key)
  translatedCache.set(key, text)
  if (translatedCache.size > MAX_TRANSLATED_ENTRIES) {
    const oldest = translatedCache.keys().next().value
    if (oldest !== undefined) translatedCache.delete(oldest)
  }
}

function notifySettings(settings: TranslationSettings): void {
  for (const listener of settingsListeners) listener(settings)
}

/**
 * Called by the settings page after a successful save. It makes the saving
 * window reactive immediately and prevents an older initial read from
 * overwriting the newly-saved value.
 */
export function primeTranslationSettings(settings: TranslationSettings): void {
  settingsGeneration += 1
  const providerChanged =
    cachedSettings !== null &&
    (cachedSettings.baseUrl !== settings.baseUrl ||
      cachedSettings.model !== settings.model ||
      cachedSettings.targetLang !== settings.targetLang)
  cachedSettings = settings
  if (providerChanged || !settings.enabled) {
    translatedCache.clear()
    translationInflight.clear()
  }
  notifySettings(settings)
}

function ensureSettingsLoaded(): Promise<TranslationSettings> {
  if (cachedSettings) return Promise.resolve(cachedSettings)
  if (settingsInflight) return settingsInflight

  const startGeneration = settingsGeneration
  settingsInflight = getTranslationSettings()
    .catch(() => DISABLED_SETTINGS)
    .then((settings) => {
      if (settingsGeneration === startGeneration) {
        cachedSettings = settings
        notifySettings(settings)
      }
      return cachedSettings ?? settings
    })
    .finally(() => {
      settingsInflight = null
    })
  return settingsInflight
}

export function useTranslationSettingsSnapshot(): TranslationSettings {
  const [settings, setSettings] = useState<TranslationSettings>(
    () => cachedSettings ?? DISABLED_SETTINGS
  )

  useEffect(() => {
    settingsListeners.add(setSettings)
    if (!cachedSettings) void ensureSettingsLoaded()
    return () => {
      settingsListeners.delete(setSettings)
    }
  }, [])

  return settings
}

export function translationCacheKey({
  blockKey,
  text,
  uiLocale,
  settings,
}: {
  blockKey: string
  text: string
  uiLocale: string
  settings: TranslationSettings
}): string {
  // Length-prefixed like the backend cache key: joining on a separator that
  // can appear inside `text` lets two different field sets render the same
  // string and serve each other's translations.
  //
  // The key deliberately EXCLUDES the carry-context reference: requests are
  // addressed by their segment text, so the same paragraph translates once
  // and is reused everywhere. The context block only shapes quality — a
  // reference-less retry of the same segment must still hit the cached
  // translation instead of paying for it twice.
  return [
    blockKey,
    uiLocale,
    settings.targetLang ?? "",
    settings.baseUrl,
    settings.model,
    text,
  ]
    .map((field) => `${field.length}:${field}`)
    .join(":")
}

export async function requestTranslation(
  text: string,
  uiLocale: string,
  key: string,
  priority: boolean = false,
  targetLang?: string | null,
  mask?: MaskedSourceFactory
): Promise<string | null> {
  return requestTranslationDetailed(
    text,
    uiLocale,
    key,
    priority,
    targetLang,
    mask
  ).then((result) => result.text)
}

export interface TranslationAttempt {
  /** The translation, or `null` when the attempt failed. */
  text: string | null
  /** Why the attempt failed, in the endpoint's own words when available. */
  error?: string
}

/**
 * The shared per-chunk gates, judging one source chunk against its reply.
 * The grouped (numbered) path and the per-chunk fallback both run every
 * candidate through here, so a grouped success can never smuggle past a gate
 * the single-chunk path would have enforced.
 */
function judgeChunkTranslation(
  chunk: string,
  translated: string,
  effectiveTarget: string | null,
  label: string
): { aligned?: string; error?: string } {
  // The numbered-protocol example in the system prompt makes some endpoints
  // prefix even un-numbered single-chunk replies with "[1] " — strip one
  // leading marker so it never rides into the rendered text.
  const cleaned = translated.replace(/^\[\d+\][ \t]/, "")
  translated = cleaned
  // An empty reply must count as a failure, never as a translation: a
  // chunk already written in the target language is exactly the one a
  // model likes to "translate" into nothing, and storing that as a piece
  // ERASES the source paragraph from the display. Retrying is right —
  // the endpoint may answer properly on a second ask, and while it
  // doesn't, the raw text stays visible.
  if (!translated.trim()) {
    console.warn(`[translation] ${label} came back empty`)
    return { error: "EMPTY_REPLY" }
  }
  // A translation is never an order of magnitude longer than its
  // source. A distill asked to translate a short already-target-language
  // line has been observed answering with a self-written essay — serving
  // it pours invented content into the message (the backend refuses and
  // refuses to cache the same reply; this is the display-side backstop
  // that also covers entries cached before that gate existed).
  if (translated.length > chunk.length * 2.5 + 200) {
    console.warn(
      `[translation] discarded ${label}: the reply is far longer than its source (${translated.length} vs ${chunk.length} characters) — the endpoint answered with invented content`
    )
    return { error: "INVENTED_CONTENT" }
  }
  // An echo (English in, English out) or a bare refusal carries no
  // target-script character at all; serving either shows the reader a
  // "translation" that never happened.
  if (
    effectiveTarget &&
    missingTargetScript(chunk, translated, effectiveTarget)
  ) {
    console.warn(
      `[translation] discarded ${label}: the reply has no target-script characters — the endpoint echoed or refused the chunk`
    )
    return { error: "ECHO_OR_REFUSAL" }
  }
  // A translation that shed the source's concrete numbers ("Git 2.34"
  // → "Git 较新版本") is answering the text, not translating it. The
  // backend refuses the same reply before its cache write; this display-
  // side backstop also covers entries cached before the gate existed.
  if (missingSourceNumbers(chunk, translated)) {
    console.warn(
      `[translation] discarded ${label}: the reply dropped numbers present in the source — likely invented content`
    )
    return { error: "DROPPED_NUMBERS" }
  }
  if (hasSameTranslationPlaceholders(chunk, translated)) {
    return { aligned: translated }
  }
  const realigned = realignTranslationPlaceholders(chunk, translated)
  if (realigned === null) {
    console.warn(
      `[translation] discarded ${label}: the endpoint changed the CBLK placeholders (this is what "no visible effect" with live API traffic usually means)`
    )
    return { error: "PLACEHOLDERS_LOST" }
  }
  return { aligned: realigned }
}

/**
 * One numbered request carrying `segments` as `[1] … [2] …`, parsed back
 * apart and gated per segment. Returns the per-segment translations, or
 * `null` when the group as a whole failed — the transport errored, the reply
 * would not parse, or ANY segment failed a gate. A null here costs nothing:
 * callers fall back to per-chunk requests, and the small extra latency of
 * the wasted numbered attempt buys far larger group successes everywhere
 * else.
 */
export async function requestNumberedGroup(
  segments: readonly string[],
  uiLocale: string,
  priority: boolean = false,
  targetLang?: string | null,
  context?: ContextReference,
  variant: number = 0
): Promise<string[] | null> {
  if (segments.length === 0) return []
  // A lone segment rides as itself: the numbering protocol exists to make
  // several paragraphs one round trip, and wrapping the common single-chunk
  // case in it would spend an extra attempt wherever grouping does nothing.
  // Observed on a live relay: the protocol example in the system prompt makes
  // the model prefix even un-numbered input with "[1] " — strip it, or it
  // rides into the rendered text.
  const single = segments.length === 1
  const numbered = single ? segments[0] : buildNumberedRequest(segments)
  const effectiveTarget =
    targetLang ?? cachedSettings?.targetLang ?? (uiLocale as string | null)
  // The XML envelope separates source (DATA) from instructions — the main
  // echo-mode defense; the constraint line escalates on retries, because at
  // temperature 0 an identical retry returns an identical wrong answer. The
  // reference block stays OUTSIDE the envelope: it is read-only framing, not
  // content to translate.
  const envelope = buildTranslateBody(numbered, effectiveTarget ?? uiLocale)
  const outbound =
    (context ? buildContextPrefix(context) : "") +
    retryConstraintLine(variant) +
    envelope
  let result
  try {
    const results = await translateTexts(
      [outbound],
      uiLocale,
      priority,
      targetLang ?? null
    )
    result = results[0]
  } catch (error) {
    console.warn(`[translation] numbered group request failed:`, error)
    return null
  }
  if (!result || result.error) {
    console.warn(
      `[translation] numbered group of ${segments.length} failed: ${result?.error ?? "no result"}`
    )
    return null
  }
  // A model imitating the envelope gets its edge tags removed before the
  // numbered parser and the gates judge the bare translation.
  const reply = stripTranslateEnvelope(result.text)
  const parsed = single
    ? [reply.replace(/^\[\d+\][ \t]/, "")]
    : parseNumberedTranslation(reply, segments.length)
  if (!parsed) {
    console.warn(
      `[translation] numbered group of ${segments.length} came back unparseable — falling back to per-chunk requests`
    )
    return null
  }
  const out: string[] = []
  for (let offset = 0; offset < segments.length; offset += 1) {
    const judged = judgeChunkTranslation(
      segments[offset],
      parsed[offset],
      effectiveTarget,
      `numbered segment ${offset + 1}/${segments.length}`
    )
    if (judged.error) return null
    out.push(judged.aligned ?? "")
  }
  return out
}

/**
 * Like {@link requestTranslation}, but reports WHY a failure happened. The
 * selection card surfaces the reason inline; the message-list hooks only need
 * the text. All the gates below (rate limit, empty, invented, echo) attach
 * the endpoint's message or a precise description to the failure.
 */
export async function requestTranslationDetailed(
  text: string,
  uiLocale: string,
  key: string,
  priority: boolean = false,
  targetLang?: string | null,
  mask: MaskedSourceFactory = maskForTranslation,
  context?: ContextReference,
  variant: number = 0
): Promise<TranslationAttempt> {
  const cached = translatedCache.get(key)
  if (cached !== undefined) return { text: cached }

  const existing = translationInflight.get(key)
  if (existing) return existing

  const pending = (async (): Promise<TranslationAttempt> => {
    const masked = mask(text)
    const chunks = splitForTranslation(masked.masked)
    if (!chunks) return { text: null, error: "SELECTION_TOO_LONG" }
    const effectiveTarget =
      targetLang ?? cachedSettings?.targetLang ?? (uiLocale as string | null)
    // Every outbound rides the XML envelope (source as DATA), and a retry
    // variant escalates the constraint line — an identical request at
    // temperature 0 returns an identical wrong answer, so retries must
    // change the request, not just repeat it.
    const outbound = (chunk: string) =>
      (context ? buildContextPrefix(context) : "") +
      retryConstraintLine(variant) +
      buildTranslateBody(chunk, effectiveTarget ?? uiLocale)

    const judgeChunk = (
      index: number,
      translated: string
    ): { aligned?: string; error?: string } => {
      return judgeChunkTranslation(
        chunks[index],
        translated,
        effectiveTarget,
        `chunk ${index} of ${key}`
      )
    }

    try {
      // Small adjacent chunks travel together: one numbered request per
      // group, `batchMaxChars` wide. A strict-RPM endpoint converges in a
      // handful of round trips instead of one per paragraph — the difference
      // between finishing and stalling.
      const batchChars = cachedSettings?.batchMaxChars ?? DEFAULT_BATCH_CHARS
      const aligned: (string | null)[] = chunks.map(() => null)
      let groupFailed = false

      for (const group of mergeUnitGroups(chunks, batchChars)) {
        const segments = group.map((index) => chunks[index])
        if (segments.length === 1) {
          // The lone-chunk contract is the old one, deliberately: one
          // request, judged, done. Routing it through the numbered group
          // would double the attempts whenever a gate fails — and gates fail
          // on exactly the endpoints that can least afford it.
          let result
          try {
            const results = await translateTexts(
              [outbound(segments[0])],
              uiLocale,
              priority,
              targetLang ?? null
            )
            result = results[0]
          } catch (error) {
            console.warn(`[translation] request failed for ${key}:`, error)
            return { text: null, error: toErrorMessage(error) }
          }
          if (!result || result.error) {
            console.warn(
              `[translation] chunk ${group[0]} of ${key} failed: ${result?.error ?? "no result"}`
            )
            return { text: null, error: result?.error ?? "BAD_BATCH" }
          }
          const judged = judgeChunk(
            group[0],
            stripTranslateEnvelope(result.text)
          )
          if (judged.error) {
            return { text: null, error: judged.error }
          }
          aligned[group[0]] = judged.aligned ?? null
          continue
        }
        const translations = await requestNumberedGroup(
          segments,
          uiLocale,
          priority,
          targetLang,
          context,
          variant
        )
        if (!translations) {
          groupFailed = true
          continue
        }
        for (let offset = 0; offset < group.length; offset += 1) {
          aligned[group[offset]] = translations[offset]
        }
      }

      // The fallback path: every chunk a numbered group could not serve goes
      // out on its own, under the per-chunk gates the grouped path skipped.
      // Chunks that already have an aligned translation here are NOT
      // re-requested — the grouped path's successes stand.
      if (groupFailed) {
        const failed = aligned
          .map((value, index) => (value === null ? index : -1))
          .filter((index) => index >= 0)
        const results = await translateTexts(
          failed.map((index) => outbound(chunks[index])),
          uiLocale,
          priority,
          targetLang ?? null
        )
        if (results.length !== failed.length) {
          console.warn(
            `[translation] discarded ${key}: expected ${failed.length} results, got ${results.length}`
          )
          return { text: null, error: "BAD_BATCH" }
        }
        for (let offset = 0; offset < failed.length; offset += 1) {
          const index = failed[offset]
          const result = results[offset]
          // A chunk the endpoint failed on (rate limits fail *some* of a
          // large burst) has no text. Returning null here is safe: the
          // backend cached the successful siblings, so the bounded retry
          // re-requests only the failed chunks and the batch converges.
          if (result.error) {
            console.warn(
              `[translation] chunk ${index} of ${key} failed: ${result.error}`
            )
            return { text: null, error: result.error }
          }
          const judged = judgeChunk(index, stripTranslateEnvelope(result.text))
          if (judged.error) {
            return { text: null, error: judged.error }
          }
          aligned[index] = judged.aligned ?? null
        }
      }

      // `restore` consumes every well-formed placeholder, and the strict
      // sequence gate above guarantees their count — nothing placeholder-
      // shaped can survive here.
      const restored = masked.restore(
        aligned
          .map((translated, index) =>
            mergeUnit(chunks[index], translated ?? "")
          )
          .join("")
      )
      rememberTranslation(key, restored)
      return { text: restored }
    } catch (error) {
      console.warn(`[translation] request failed for ${key}:`, error)
      return { text: null, error: toErrorMessage(error) }
    }
  })().finally(() => {
    translationInflight.delete(key)
  })

  translationInflight.set(key, pending)
  return pending
}

export interface UseTranslatedTextParams {
  text: string
  /** Must mean this individual message is unsettled (`!completed` today). */
  isStreaming: boolean
  isUser: boolean
  shouldLoad: boolean
  uiLocale: string
  blockKey: string
  /** Thinking has its own opt-in setting; ordinary prose leaves this false. */
  isThinking?: boolean
  /**
   * Stand down entirely. Set while the streaming thinking hook owns this block,
   * so the settled path cannot also request the whole text.
   */
  disabled?: boolean
  /**
   * Queue on the backend's fast lane (reply prose, user-initiated requests)
   * instead of behind background thinking-block polish.
   */
  priority?: boolean
}

export interface TranslatedTextState {
  display: string
  hasTranslation: boolean
  isTranslated: boolean
  /**
   * The block's last translation attempt failed and nothing landed. The
   * renderer shows this as an amber toggle indicator — the first place a
   * "why is this still English" reader looks, instead of the console.
   */
  hasErrors: boolean
  /** The failure reason, in the endpoint's own words when available. */
  errorHint: string | null
  showOriginal: () => void
  showTranslation: () => void
}

export function useTranslatedText({
  text,
  isStreaming,
  isUser,
  shouldLoad,
  uiLocale,
  blockKey,
  isThinking = false,
  disabled = false,
  priority = false,
}: UseTranslatedTextParams): TranslatedTextState {
  const settings = useTranslationSettingsSnapshot()
  const [loaded, setLoaded] = useState<{ key: string; text: string } | null>(
    null
  )
  const [originalKey, setOriginalKey] = useState<string | null>(null)
  const [lastError, setLastError] = useState<string | null>(null)

  const enabled =
    settings.enabled && (!isThinking || settings.translateThinking)
  const key = useMemo(
    () => translationCacheKey({ blockKey, text, uiLocale, settings }),
    [blockKey, text, uiLocale, settings]
  )

  useEffect(() => {
    let current = true

    if (
      disabled ||
      !shouldLoad ||
      !shouldTranslate({ text, isStreaming, isUser, enabled })
    ) {
      return () => {
        current = false
      }
    }

    // The detailed variant so the failure reason survives — the plain
    // requestTranslation returns a bare null and the "why" would die here.
    void requestTranslationDetailed(text, uiLocale, key, priority).then(
      (attempt) => {
        if (!current) return
        if (attempt.text !== null) {
          setLoaded({ key, text: attempt.text })
          setOriginalKey(null)
          setLastError(null)
        } else if (attempt.error) {
          setLastError(attempt.error)
        }
      }
    )

    return () => {
      current = false
    }
  }, [
    disabled,
    enabled,
    isStreaming,
    isUser,
    key,
    priority,
    shouldLoad,
    text,
    uiLocale,
  ])

  // Derive the active view from the current key rather than resetting state in
  // an effect: when the text (or settings) changes, `key` moves on and this
  // stale entry — and its "showing original" flag — stops applying on its own.
  const translation = loaded?.key === key ? loaded.text : null
  const showingOriginal = originalKey === key
  const showOriginal = useCallback(() => setOriginalKey(key), [key])
  const showTranslation = useCallback(() => setOriginalKey(null), [])
  const hasTranslation = translation !== null
  const isTranslated = hasTranslation && !showingOriginal
  // A stale error belongs to a previous text/settings shape; it stops
  // applying the moment the current key has a translation of its own.
  const errorHint = hasTranslation ? null : lastError

  return {
    display: isTranslated ? translation : text,
    hasTranslation,
    isTranslated,
    hasErrors: errorHint !== null,
    errorHint,
    showOriginal,
    showTranslation,
  }
}

/**
 * Whether translation is switched on at all, for callers that offer it as an
 * explicit action (selection translation) rather than rendering a block. The
 * `translateThinking` opt-in does not gate this: asking for a translation by
 * hand is not the same as translating thinking automatically.
 */
export function useTranslationEnabled(): boolean {
  return useTranslationSettingsSnapshot().enabled
}
