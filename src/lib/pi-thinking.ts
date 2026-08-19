/**
 * pi's per-model reasoning declaration, as it lives in `~/.pi/agent/models.json`.
 *
 * A model pi loads from `providers.<id>.models[]` gets `reasoning: modelDef.reasoning ?? false`
 * (pi-coding-agent `dist/core/model-registry.js`). With `reasoning` falsy, pi's
 * `getSupportedThinkingLevels()` returns `["off"]` and `AgentSession.setThinkingLevel()`
 * clamps EVERY request down to `off` — which pi-acp then echoes back as the session's
 * `thought_level` config option, so the composer's picker snaps back the instant it is
 * touched. `reasoning` is also the gate on pi sending `reasoning.effort` to the provider
 * at all (pi-ai `dist/api/openai-responses.js`), so an undeclared model has nothing to
 * pick from in the first place.
 *
 * The vocabulary is pi's fixed six (`EXTENDED_THINKING_LEVELS` in pi-ai `dist/models.js`) —
 * unlike Kimi's free-form `support_efforts`, a level name outside this list is rejected by
 * pi-acp's `isThinkingLevel` with `invalidParams`.
 */

export const PI_THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
] as const

export type PiThinkingLevel = (typeof PI_THINKING_LEVELS)[number]

/**
 * pi's `thinkingLevelMap`. Does double duty:
 *  - availability — `null` removes the level from the picker; `xhigh` is inverted and
 *    needs an explicit non-null entry to appear at all.
 *  - wire value — what pi sends as the provider's reasoning effort
 *    (`effort = thinkingLevelMap?.[level] ?? level`). Google-style backends need
 *    `low → "LOW"`; `off` sends `"none"` when unmapped.
 */
export type PiThinkingLevelMap = Partial<Record<PiThinkingLevel, string | null>>

/** The panel's editable view of one model's reasoning declaration. */
export interface PiModelReasoning {
  /** Writes `reasoning` — the gate on the picker existing at all. */
  enabled: boolean
  /** Levels offered in the composer picker, in `PI_THINKING_LEVELS` order. */
  levels: PiThinkingLevel[]
  /** Per-level wire value, only where it differs from the implicit default. */
  wireValues: Partial<Record<PiThinkingLevel, string>>
}

export function isPiThinkingLevel(value: string): value is PiThinkingLevel {
  return (PI_THINKING_LEVELS as readonly string[]).includes(value)
}

/**
 * What pi sends for `level` when the map has no entry for it: `off` becomes `"none"`
 * (pi's own `?? "none"` fallback), every other level is sent verbatim.
 */
export function implicitWireValue(level: PiThinkingLevel): string {
  return level === "off" ? "none" : level
}

/**
 * Which levels pi would offer for this map. Mirrors `getSupportedThinkingLevels`
 * exactly: a `null` entry drops the level, and `xhigh` requires an explicit entry.
 */
export function levelsFromMap(
  map: PiThinkingLevelMap | null | undefined
): PiThinkingLevel[] {
  return PI_THINKING_LEVELS.filter((level) => {
    const mapped = map?.[level]
    if (mapped === null) return false
    if (level === "xhigh") return mapped !== undefined
    return true
  })
}

/**
 * Project a stored model definition into the panel's editable state.
 *
 * Wire values that merely restate the implicit default are folded back to "unset" so the
 * advanced editor shows empty placeholders rather than the `"none"` / `"xhigh"` markers
 * {@link reasoningToMap} has to write — that keeps a load → save round-trip byte-stable.
 */
export function reasoningFromModel(
  reasoning: boolean | null | undefined,
  map: PiThinkingLevelMap | null | undefined
): PiModelReasoning {
  const levels = levelsFromMap(map)
  const wireValues: Partial<Record<PiThinkingLevel, string>> = {}
  for (const level of levels) {
    const mapped = map?.[level]
    if (typeof mapped === "string" && mapped !== implicitWireValue(level)) {
      wireValues[level] = mapped
    }
  }
  return { enabled: reasoning === true, levels, wireValues }
}

/**
 * Derive the `thinkingLevelMap` to persist.
 *
 * Unchecked levels are pinned to `null`. A checked level gets an explicit entry only when
 * it has to: a custom wire value, `off` (so the "reasoning is off" effort is spelled out
 * like pi's own built-in `gpt-5.5`), or `xhigh` (which is invisible without one).
 * Everything else is omitted — same meaning, smaller file.
 */
export function reasoningToMap(
  reasoning: PiModelReasoning
): PiThinkingLevelMap {
  const map: PiThinkingLevelMap = {}
  for (const level of PI_THINKING_LEVELS) {
    if (!reasoning.levels.includes(level)) {
      map[level] = null
      continue
    }
    const override = reasoning.wireValues[level]?.trim()
    if (override) {
      map[level] = override
    } else if (level === "off" || level === "xhigh") {
      map[level] = implicitWireValue(level)
    }
  }
  return map
}

/** Toggle one level, keeping `PI_THINKING_LEVELS` order so the chips never reshuffle. */
export function toggleLevel(
  levels: PiThinkingLevel[],
  level: PiThinkingLevel
): PiThinkingLevel[] {
  const next = levels.includes(level)
    ? levels.filter((item) => item !== level)
    : [...levels, level]
  return PI_THINKING_LEVELS.filter((item) => next.includes(item))
}
