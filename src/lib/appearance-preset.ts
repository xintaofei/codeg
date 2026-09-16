// src/lib/appearance-preset.ts
//
// The shareable appearance preset: one JSON document that captures a whole
// look (base palette, token overrides for both modes, fonts, density, bubble
// and composer shape, code colours) and nothing else.
//
// Three rules shape the format:
//
// 1. Declarative tokens only. A preset carries values for an allowlist of
//    tokens and names fonts / code themes by id from allowlists the app ships.
//    There is no CSS and no script in the format, so importing a stranger's
//    preset can change colours, fonts and spacing and nothing else. The
//    free-form Custom CSS stays a local, per-device escape hatch: it is never
//    written into a preset and a preset can never carry it.
// 2. Strict. Unknown keys anywhere are errors rather than silently dropped
//    (a typo in `colors.dark.primry` should not import as "worked"), every
//    value is syntax- and range-checked, and the document has a hard size
//    cap. Every problem comes back with a JSON path so the UI can point at
//    the exact field.
// 3. Versioned. `schemaVersion` is required. Older documents step forward
//    through `MIGRATIONS`; a document from a newer app is refused with its own
//    code rather than half-applied.
//
// The same document is what a preset gallery would serve later: an index of
// these files plus a name and a thumbnail is a marketplace, and validation on
// that path is this exact function.

import {
  CUSTOM_FONT_ID,
  FONT_BY_ID,
  MONO_FONTS,
  type FontDef,
} from "./font-presets"
import { THEME_COLORS, type ThemeColor } from "./theme-presets"
import {
  LAYOUT_THEME_TOKENS,
  SHADCN_THEME_TOKENS,
  SURFACE_COLOR_TOKENS,
  byteLengthOf,
  isValidTokenValue,
  type CustomTheme,
  type CustomThemeToken,
  type TokenOverrides,
} from "./custom-style"
import {
  DEFAULT_CODE_THEME,
  isCodeThemeId,
  type CodeThemePair,
} from "./code-themes"

// ─── Format ───

export const PRESET_SCHEMA_VERSION = 1 as const

/**
 * A preset is a few kilobytes of tokens; anything larger is not a preset.
 * The cap runs before JSON.parse so an oversized file is refused at the byte
 * count, never parsed.
 */
export const MAX_PRESET_BYTES = 32 * 1024

/** Exported files end in this, so a folder of presets is grep-able. */
export const PRESET_FILE_SUFFIX = ".codeg-preset.json"

export const PRESET_MODES = ["light", "dark", "system"] as const
export type PresetMode = (typeof PRESET_MODES)[number]

export type PresetColorToken =
  | (typeof SHADCN_THEME_TOKENS)[number]
  | (typeof SURFACE_COLOR_TOKENS)[number]

export type PresetColors = Partial<Record<PresetColorToken, string>>

export type PresetShape = {
  /** Drives the whole radius scale (`--radius`). */
  radius?: string
  userBubbleRadius?: string
  assistantBubbleRadius?: string
  assistantBubblePadding?: string
  composerRadius?: string
}

export type PresetDensity = {
  /** Multiplier on the base spacing unit; 1 is the stock layout. */
  spacing?: number
}

export type PresetTypography = {
  /** Interface font id from the bundled / system allowlist. */
  ui?: string
  /** Monospace font id for code in messages. */
  mono?: string
  /** Message text size in rem (0.875 is the stock `text-sm`). */
  chatFontSize?: number
  /** Unitless line height for message text. */
  chatLineHeight?: number
}

export type AppearancePreset = {
  schemaVersion: typeof PRESET_SCHEMA_VERSION
  /** Stable slug, also the export file name. */
  id: string
  name: string
  description?: string
  author?: string
  /** Free-form, for the author's own bookkeeping. */
  version?: string
  /** The built-in palette the token overrides sit on. */
  base: ThemeColor
  /** Light / dark / follow system, applied when the preset is. */
  mode?: PresetMode
  colors: { light?: PresetColors; dark?: PresetColors }
  shape?: PresetShape
  density?: PresetDensity
  typography?: PresetTypography
  code?: CodeThemePair
}

export const PRESET_LIMITS = {
  id: 64,
  name: 80,
  description: 280,
  author: 80,
  version: 32,
  spacing: { min: 0.75, max: 1.25 },
  chatFontSize: { min: 0.7, max: 1.25 },
  chatLineHeight: { min: 1.1, max: 2 },
  lengthRem: 4,
  lengthPx: 64,
} as const

// ─── Issues ───

export type PresetIssueCode =
  | "invalid-json"
  | "too-large"
  | "not-object"
  | "unknown-key"
  | "missing"
  | "wrong-type"
  | "invalid-value"
  | "unsupported-version"
  | "unknown-token"
  | "invalid-color"
  | "invalid-length"
  | "out-of-range"
  | "unknown-font"
  | "unknown-code-theme"
  | "unknown-base"

/** One problem, addressed by a dotted JSON path (`colors.dark.primary`). */
export type PresetIssue = {
  path: string
  code: PresetIssueCode
  detail?: string
}

export type PresetParseResult =
  | { ok: true; preset: AppearancePreset; warnings: PresetIssue[] }
  | { ok: false; issues: PresetIssue[] }

// ─── Validation ───

const TOP_LEVEL_KEYS = [
  "$schema",
  "schemaVersion",
  "id",
  "name",
  "description",
  "author",
  "version",
  "base",
  "mode",
  "colors",
  "shape",
  "density",
  "typography",
  "code",
] as const

const SHAPE_KEYS = [
  "radius",
  "userBubbleRadius",
  "assistantBubbleRadius",
  "assistantBubblePadding",
  "composerRadius",
] as const

const DENSITY_KEYS = ["spacing"] as const
const TYPOGRAPHY_KEYS = [
  "ui",
  "mono",
  "chatFontSize",
  "chatLineHeight",
] as const
const CODE_KEYS = ["light", "dark"] as const
const MODE_KEYS = ["light", "dark"] as const

const ID_RE = /^[a-z0-9][a-z0-9-]*$/
const VERSION_RE = /^[0-9A-Za-z.+-]+$/

/**
 * Colour syntaxes a preset may use for a colour token. Named colours other
 * than `transparent` are refused: a preset is read by people as much as by
 * the app, and `oklch(0.7 0.1 250)` says what it is where `plum` does not.
 */
const COLOR_SYNTAX_RE =
  /^(#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})|transparent|(rgb|rgba|hsl|hsla|oklch|oklab|lab|lch|hwb|color|color-mix)\(.+\))$/i

const LENGTH_RE = /^(\d+\.?\d*|\.\d+)(rem|px|em)$/

const PRESET_COLOR_TOKEN_SET = new Set<string>([
  ...SHADCN_THEME_TOKENS,
  ...SURFACE_COLOR_TOKENS,
])

export function isPresetColorToken(name: unknown): name is PresetColorToken {
  return typeof name === "string" && PRESET_COLOR_TOKEN_SET.has(name)
}

/**
 * Whether `value` is a colour a preset may carry: the token charset guard
 * (declaration-escape defence) first, then the syntax check that says "this
 * is a colour". Exported for the gallery index, whose card swatches are
 * colours that belong to no token.
 */
export function isPresetColorValue(value: string): boolean {
  return isValidTokenValue("primary", value) && COLOR_SYNTAX_RE.test(value)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function join(path: string, key: string): string {
  return path ? `${path}.${key}` : key
}

class Collector {
  readonly issues: PresetIssue[] = []
  readonly warnings: PresetIssue[] = []

  add(path: string, code: PresetIssueCode, detail?: string): void {
    this.issues.push(detail ? { path, code, detail } : { path, code })
  }

  /** Fails on any key outside `allowed`, naming each one. */
  keys(
    record: Record<string, unknown>,
    allowed: readonly string[],
    path: string
  ) {
    const allowedSet = new Set<string>(allowed)
    for (const key of Object.keys(record)) {
      if (!allowedSet.has(key)) this.add(join(path, key), "unknown-key")
    }
  }

  object(
    value: unknown,
    path: string,
    required: boolean
  ): Record<string, unknown> | null {
    if (value === undefined) {
      if (required) this.add(path, "missing")
      return null
    }
    if (!isRecord(value)) {
      this.add(path, value === null ? "wrong-type" : "not-object")
      return null
    }
    return value
  }

  string(
    value: unknown,
    path: string,
    {
      required,
      max,
      pattern,
    }: { required: boolean; max: number; pattern?: RegExp }
  ): string | undefined {
    if (value === undefined) {
      if (required) this.add(path, "missing")
      return undefined
    }
    if (typeof value !== "string") {
      this.add(path, "wrong-type")
      return undefined
    }
    const trimmed = value.trim()
    if (required && !trimmed) {
      this.add(path, "invalid-value", "empty")
      return undefined
    }
    if (trimmed.length > max) {
      this.add(path, "out-of-range", `max ${max} characters`)
      return undefined
    }
    if (pattern && trimmed && !pattern.test(trimmed)) {
      this.add(path, "invalid-value")
      return undefined
    }
    return trimmed
  }

  number(
    value: unknown,
    path: string,
    range: { min: number; max: number }
  ): number | undefined {
    if (value === undefined) return undefined
    if (typeof value !== "number" || !Number.isFinite(value)) {
      this.add(path, "wrong-type")
      return undefined
    }
    if (value < range.min || value > range.max) {
      this.add(path, "out-of-range", `${range.min} to ${range.max}`)
      return undefined
    }
    return value
  }

  /** A CSS length in rem / px / em within the layout ranges. */
  length(value: unknown, path: string): string | undefined {
    if (value === undefined) return undefined
    if (typeof value !== "string") {
      this.add(path, "wrong-type")
      return undefined
    }
    const trimmed = value.trim()
    const parsed = parseCssLength(trimmed)
    if (!parsed) {
      this.add(path, "invalid-length")
      return undefined
    }
    const max =
      parsed.unit === "px" ? PRESET_LIMITS.lengthPx : PRESET_LIMITS.lengthRem
    if (parsed.value > max) {
      this.add(path, "out-of-range", `max ${max}${parsed.unit}`)
      return undefined
    }
    return trimmed
  }

  colors(value: unknown, path: string): PresetColors | undefined {
    const record = this.object(value, path, false)
    if (!record) return undefined
    const out: PresetColors = {}
    for (const [key, raw] of Object.entries(record)) {
      const keyPath = join(path, key)
      if (!isPresetColorToken(key)) {
        this.add(keyPath, "unknown-token")
        continue
      }
      if (typeof raw !== "string") {
        this.add(keyPath, "wrong-type")
        continue
      }
      const trimmed = raw.trim()
      if (key === "radius") {
        const radius = this.length(trimmed, keyPath)
        if (radius !== undefined) out.radius = radius
        continue
      }
      // The token charset guard first (declaration-escape defence), then
      // the syntax check that says "this is a colour".
      if (!isValidTokenValue(key, trimmed) || !COLOR_SYNTAX_RE.test(trimmed)) {
        this.add(keyPath, "invalid-color")
        continue
      }
      out[key] = trimmed
    }
    return out
  }
}

export type CssLength = { value: number; unit: "rem" | "px" | "em" }

export function parseCssLength(value: string): CssLength | null {
  const match = LENGTH_RE.exec(value.trim())
  if (!match) return null
  const n = Number(match[1])
  if (!Number.isFinite(n)) return null
  return { value: n, unit: match[2] as CssLength["unit"] }
}

function isMonoFont(id: string): boolean {
  return MONO_FONTS.some((font: FontDef) => font.id === id)
}

/**
 * Forward migrations, keyed by the version they migrate FROM. Empty while
 * version 1 is the only version; adding a version means adding an entry here
 * and bumping `PRESET_SCHEMA_VERSION`, and the migration test walks the chain.
 */
const MIGRATIONS: Record<
  number,
  (document: Record<string, unknown>) => Record<string, unknown>
> = {}

function migrate(
  document: Record<string, unknown>,
  collector: Collector
): Record<string, unknown> | null {
  const version = document.schemaVersion
  if (version === undefined) {
    collector.add("schemaVersion", "missing")
    return null
  }
  if (typeof version !== "number" || !Number.isInteger(version)) {
    collector.add("schemaVersion", "wrong-type")
    return null
  }
  if (version > PRESET_SCHEMA_VERSION) {
    collector.add(
      "schemaVersion",
      "unsupported-version",
      `newer than ${PRESET_SCHEMA_VERSION}`
    )
    return null
  }
  let current = document
  for (let v = version; v < PRESET_SCHEMA_VERSION; v += 1) {
    const step = MIGRATIONS[v]
    if (!step) {
      collector.add("schemaVersion", "unsupported-version", `no path from ${v}`)
      return null
    }
    current = { ...step(current), schemaVersion: v + 1 }
  }
  if (version < 1) {
    collector.add("schemaVersion", "unsupported-version")
    return null
  }
  return current
}

/**
 * Validate an already-parsed value. Prefer {@link parseAppearancePreset} for
 * text, which adds the size cap and JSON errors in front of this.
 */
export function validateAppearancePreset(input: unknown): PresetParseResult {
  const c = new Collector()
  const raw = c.object(input, "", true)
  if (!raw) return { ok: false, issues: c.issues }

  const doc = migrate(raw, c)
  if (!doc) return { ok: false, issues: c.issues }

  c.keys(doc, TOP_LEVEL_KEYS, "")

  if (doc.$schema !== undefined && typeof doc.$schema !== "string") {
    c.add("$schema", "wrong-type")
  }

  const id = c.string(doc.id, "id", {
    required: true,
    max: PRESET_LIMITS.id,
    pattern: ID_RE,
  })
  const name = c.string(doc.name, "name", {
    required: true,
    max: PRESET_LIMITS.name,
  })
  const description = c.string(doc.description, "description", {
    required: false,
    max: PRESET_LIMITS.description,
  })
  const author = c.string(doc.author, "author", {
    required: false,
    max: PRESET_LIMITS.author,
  })
  const version = c.string(doc.version, "version", {
    required: false,
    max: PRESET_LIMITS.version,
    pattern: VERSION_RE,
  })

  let base: ThemeColor | undefined
  if (doc.base === undefined) {
    c.add("base", "missing")
  } else if (typeof doc.base !== "string") {
    c.add("base", "wrong-type")
  } else if (!(THEME_COLORS as readonly string[]).includes(doc.base)) {
    c.add("base", "unknown-base")
  } else {
    base = doc.base as ThemeColor
  }

  let mode: PresetMode | undefined
  if (doc.mode !== undefined) {
    if (typeof doc.mode !== "string") {
      c.add("mode", "wrong-type")
    } else if (!(PRESET_MODES as readonly string[]).includes(doc.mode)) {
      c.add("mode", "invalid-value")
    } else {
      mode = doc.mode as PresetMode
    }
  }

  const colors: AppearancePreset["colors"] = {}
  const colorsRecord = c.object(doc.colors, "colors", true)
  if (colorsRecord) {
    c.keys(colorsRecord, MODE_KEYS, "colors")
    const light = c.colors(colorsRecord.light, "colors.light")
    const dark = c.colors(colorsRecord.dark, "colors.dark")
    if (light) colors.light = light
    if (dark) colors.dark = dark
  }

  let shape: PresetShape | undefined
  const shapeRecord = c.object(doc.shape, "shape", false)
  if (shapeRecord) {
    c.keys(shapeRecord, SHAPE_KEYS, "shape")
    shape = {}
    for (const key of SHAPE_KEYS) {
      const value = c.length(shapeRecord[key], join("shape", key))
      if (value !== undefined) shape[key] = value
    }
  }

  let density: PresetDensity | undefined
  const densityRecord = c.object(doc.density, "density", false)
  if (densityRecord) {
    c.keys(densityRecord, DENSITY_KEYS, "density")
    density = {}
    const spacing = c.number(
      densityRecord.spacing,
      "density.spacing",
      PRESET_LIMITS.spacing
    )
    if (spacing !== undefined) density.spacing = spacing
  }

  let typography: PresetTypography | undefined
  const typographyRecord = c.object(doc.typography, "typography", false)
  if (typographyRecord) {
    c.keys(typographyRecord, TYPOGRAPHY_KEYS, "typography")
    typography = {}
    for (const key of ["ui", "mono"] as const) {
      const value = typographyRecord[key]
      if (value === undefined) continue
      const path = join("typography", key)
      if (typeof value !== "string") {
        c.add(path, "wrong-type")
      } else if (
        value === CUSTOM_FONT_ID ||
        !(value in FONT_BY_ID) ||
        (key === "mono" && !isMonoFont(value))
      ) {
        c.add(path, "unknown-font")
      } else {
        typography[key] = value
      }
    }
    const size = c.number(
      typographyRecord.chatFontSize,
      "typography.chatFontSize",
      PRESET_LIMITS.chatFontSize
    )
    if (size !== undefined) typography.chatFontSize = size
    const lineHeight = c.number(
      typographyRecord.chatLineHeight,
      "typography.chatLineHeight",
      PRESET_LIMITS.chatLineHeight
    )
    if (lineHeight !== undefined) typography.chatLineHeight = lineHeight
  }

  let code: CodeThemePair | undefined
  const codeRecord = c.object(doc.code, "code", false)
  if (codeRecord) {
    c.keys(codeRecord, CODE_KEYS, "code")
    const pair: Partial<CodeThemePair> = {}
    for (const key of CODE_KEYS) {
      const value = codeRecord[key]
      const path = join("code", key)
      if (value === undefined) {
        c.add(path, "missing")
      } else if (typeof value !== "string") {
        c.add(path, "wrong-type")
      } else if (!isCodeThemeId(value)) {
        c.add(path, "unknown-code-theme")
      } else {
        pair[key] = value
      }
    }
    if (pair.light && pair.dark) code = { light: pair.light, dark: pair.dark }
  }

  if (c.issues.length > 0 || !id || !name || !base) {
    return { ok: false, issues: c.issues }
  }

  const preset: AppearancePreset = {
    schemaVersion: PRESET_SCHEMA_VERSION,
    id,
    name,
    base,
    colors,
  }
  if (description) preset.description = description
  if (author) preset.author = author
  if (version) preset.version = version
  if (mode) preset.mode = mode
  if (shape && Object.keys(shape).length > 0) preset.shape = shape
  if (density && Object.keys(density).length > 0) preset.density = density
  if (typography && Object.keys(typography).length > 0) {
    preset.typography = typography
  }
  if (code) preset.code = code
  return { ok: true, preset, warnings: c.warnings }
}

/** Text from a file or the clipboard -> a validated preset, or every problem. */
export function parseAppearancePreset(text: string): PresetParseResult {
  const bytes = byteLengthOf(text)
  if (bytes > MAX_PRESET_BYTES) {
    return {
      ok: false,
      issues: [
        {
          path: "",
          code: "too-large",
          detail: `${bytes} > ${MAX_PRESET_BYTES}`,
        },
      ],
    }
  }
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch (error) {
    return {
      ok: false,
      issues: [
        {
          path: "",
          code: "invalid-json",
          detail: error instanceof Error ? error.message : undefined,
        },
      ],
    }
  }
  return validateAppearancePreset(value)
}

/** Stable, human-diffable serialization: 2-space JSON with a trailing newline. */
export function serializeAppearancePreset(preset: AppearancePreset): string {
  return `${JSON.stringify(preset, null, 2)}\n`
}

/**
 * The last applied preset as kept in localStorage. Runs the full validation,
 * so a document written by an older build that no longer validates is simply
 * forgotten rather than trusted.
 */
export function parseStoredAppearancePreset(
  raw: string | null
): AppearancePreset | null {
  if (!raw) return null
  const result = parseAppearancePreset(raw)
  return result.ok ? result.preset : null
}

export function presetFileName(preset: Pick<AppearancePreset, "id">): string {
  return `${preset.id}${PRESET_FILE_SUFFIX}`
}

// ─── Preset <-> appearance state ───

/** The base spacing unit Tailwind ships (`--spacing: 0.25rem`). */
export const BASE_SPACING_REM = 0.25

/** Stock message text: Tailwind `text-sm` (0.875rem / 1.25rem line box). */
export const DEFAULT_CHAT_FONT_SIZE_REM = 0.875
export const DEFAULT_CHAT_LINE_HEIGHT = 1.25 / 0.875

function formatNumber(n: number): string {
  return String(Number(n.toFixed(4)))
}

export function spacingToken(multiplier: number): string {
  return `${formatNumber(BASE_SPACING_REM * multiplier)}rem`
}

/** Inverse of {@link spacingToken}; null for a hand-edited non-rem value. */
export function spacingMultiplierFromToken(
  value: string | undefined
): number | null {
  if (!value) return null
  const parsed = parseCssLength(value)
  if (!parsed || parsed.unit !== "rem") return null
  return Number((parsed.value / BASE_SPACING_REM).toFixed(4))
}

function remFromToken(value: string | undefined): number | null {
  if (!value) return null
  const parsed = parseCssLength(value)
  return parsed && parsed.unit === "rem" ? parsed.value : null
}

function ratioFromToken(value: string | undefined): number | null {
  if (!value) return null
  const n = Number(value.trim())
  return Number.isFinite(n) ? n : null
}

const SHAPE_TOKENS: Record<keyof PresetShape, CustomThemeToken> = {
  radius: "radius",
  userBubbleRadius: "bubble-user-radius",
  assistantBubbleRadius: "bubble-assistant-radius",
  assistantBubblePadding: "bubble-assistant-padding",
  composerRadius: "composer-radius",
}

/**
 * What applying a preset means in terms of the appearance state the provider
 * already manages: a base palette, a light / dark token map (the preset's
 * mode-independent settings fanned out to both modes), and the id-based
 * settings that have their own storage.
 */
export type PresetApplication = {
  themeColor: ThemeColor
  customTheme: CustomTheme
  uiFont?: string
  monoFont?: string
  codeTheme?: CodeThemePair
  mode?: PresetMode
}

export function presetToApplication(
  preset: AppearancePreset
): PresetApplication {
  const light: TokenOverrides = { ...(preset.colors.light ?? {}) }
  const dark: TokenOverrides = { ...(preset.colors.dark ?? {}) }
  const shared: TokenOverrides = {}

  if (preset.shape) {
    for (const key of SHAPE_KEYS) {
      const value = preset.shape[key]
      if (value !== undefined) shared[SHAPE_TOKENS[key]] = value
    }
  }
  if (preset.density?.spacing !== undefined) {
    shared.spacing = spacingToken(preset.density.spacing)
  }
  if (preset.typography?.chatFontSize !== undefined) {
    shared["chat-font-size"] =
      `${formatNumber(preset.typography.chatFontSize)}rem`
  }
  if (preset.typography?.chatLineHeight !== undefined) {
    shared["chat-line-height"] = formatNumber(preset.typography.chatLineHeight)
  }
  Object.assign(light, shared)
  Object.assign(dark, shared)

  const application: PresetApplication = {
    themeColor: preset.base,
    customTheme: { light, dark },
  }
  if (preset.typography?.ui) application.uiFont = preset.typography.ui
  if (preset.typography?.mono) application.monoFont = preset.typography.mono
  if (preset.code) application.codeTheme = { ...preset.code }
  if (preset.mode) application.mode = preset.mode
  return application
}

/** Everything the current appearance state contributes to an export. */
export type AppearanceSnapshot = {
  themeColor: ThemeColor
  customTheme: CustomTheme
  uiFontId: string
  monoFontId: string
  codeTheme: CodeThemePair
  /** The next-themes setting; null leaves `mode` out of the file. */
  mode: PresetMode | null
}

export type PresetMeta = Pick<
  AppearancePreset,
  "id" | "name" | "description" | "author" | "version"
>

function firstLayoutValue(
  theme: CustomTheme,
  token: CustomThemeToken
): string | undefined {
  return theme.light[token] ?? theme.dark[token]
}

/**
 * Build a preset from the live state. Layout tokens are read from whichever
 * mode has them (they are always written to both). Anything that cannot be
 * shared, such as a custom font family installed on this machine or a
 * hand-edited spacing in px, is left out and reported as a warning so the
 * exporter can say what did not travel.
 */
export function snapshotToPreset(
  snapshot: AppearanceSnapshot,
  meta: PresetMeta
): { preset: AppearancePreset; warnings: PresetIssue[] } {
  const warnings: PresetIssue[] = []
  const layoutSet = new Set<string>(LAYOUT_THEME_TOKENS)

  const pickColors = (overrides: TokenOverrides): PresetColors => {
    const out: PresetColors = {}
    for (const [key, value] of Object.entries(overrides)) {
      if (key === "radius" || layoutSet.has(key) || !value) continue
      if (isPresetColorToken(key)) out[key] = value
    }
    return out
  }

  const preset: AppearancePreset = {
    schemaVersion: PRESET_SCHEMA_VERSION,
    id: meta.id,
    name: meta.name,
    base: snapshot.themeColor,
    colors: {},
  }
  if (meta.description) preset.description = meta.description
  if (meta.author) preset.author = meta.author
  if (meta.version) preset.version = meta.version
  if (snapshot.mode) preset.mode = snapshot.mode

  const light = pickColors(snapshot.customTheme.light)
  const dark = pickColors(snapshot.customTheme.dark)
  if (Object.keys(light).length > 0) preset.colors.light = light
  if (Object.keys(dark).length > 0) preset.colors.dark = dark

  const shape: PresetShape = {}
  for (const key of SHAPE_KEYS) {
    const value = firstLayoutValue(snapshot.customTheme, SHAPE_TOKENS[key])
    if (value === undefined) continue
    const parsed = parseCssLength(value)
    const max =
      parsed?.unit === "px" ? PRESET_LIMITS.lengthPx : PRESET_LIMITS.lengthRem
    if (!parsed || parsed.value > max) {
      warnings.push({ path: join("shape", key), code: "invalid-length" })
      continue
    }
    shape[key] = value
  }
  if (Object.keys(shape).length > 0) preset.shape = shape

  const spacingValue = firstLayoutValue(snapshot.customTheme, "spacing")
  if (spacingValue !== undefined) {
    const multiplier = spacingMultiplierFromToken(spacingValue)
    if (
      multiplier === null ||
      multiplier < PRESET_LIMITS.spacing.min ||
      multiplier > PRESET_LIMITS.spacing.max
    ) {
      warnings.push({ path: "density.spacing", code: "out-of-range" })
    } else {
      preset.density = { spacing: multiplier }
    }
  }

  const typography: PresetTypography = {}
  for (const [key, fontId] of [
    ["ui", snapshot.uiFontId],
    ["mono", snapshot.monoFontId],
  ] as const) {
    if (fontId === CUSTOM_FONT_ID || !(fontId in FONT_BY_ID)) {
      warnings.push({ path: join("typography", key), code: "unknown-font" })
      continue
    }
    if (key === "mono" && !isMonoFont(fontId)) {
      warnings.push({ path: "typography.mono", code: "unknown-font" })
      continue
    }
    typography[key] = fontId
  }
  const chatFontSize = remFromToken(
    firstLayoutValue(snapshot.customTheme, "chat-font-size")
  )
  if (chatFontSize !== null) {
    if (
      chatFontSize < PRESET_LIMITS.chatFontSize.min ||
      chatFontSize > PRESET_LIMITS.chatFontSize.max
    ) {
      warnings.push({ path: "typography.chatFontSize", code: "out-of-range" })
    } else {
      typography.chatFontSize = chatFontSize
    }
  }
  const chatLineHeight = ratioFromToken(
    firstLayoutValue(snapshot.customTheme, "chat-line-height")
  )
  if (chatLineHeight !== null) {
    if (
      chatLineHeight < PRESET_LIMITS.chatLineHeight.min ||
      chatLineHeight > PRESET_LIMITS.chatLineHeight.max
    ) {
      warnings.push({ path: "typography.chatLineHeight", code: "out-of-range" })
    } else {
      typography.chatLineHeight = chatLineHeight
    }
  }
  if (Object.keys(typography).length > 0) preset.typography = typography

  preset.code = { ...snapshot.codeTheme }

  return { preset, warnings }
}

// ─── Matching ───

/** The live state a preset is compared against (mode is a window setting). */
export type AppearanceCurrent = {
  themeColor: ThemeColor
  customTheme: CustomTheme
  uiFontId: string
  monoFontId: string
  codeTheme: CodeThemePair
}

function sameOverrides(a: TokenOverrides, b: TokenOverrides): boolean {
  const keysA = Object.keys(a).filter((k) => a[k as CustomThemeToken])
  const keysB = Object.keys(b).filter((k) => b[k as CustomThemeToken])
  if (keysA.length !== keysB.length) return false
  return keysA.every(
    (k) => a[k as CustomThemeToken] === b[k as CustomThemeToken]
  )
}

/**
 * True when the live state is exactly what applying `preset` produces. Only
 * the settings the preset specifies are compared: a preset that says nothing
 * about fonts still matches whichever font the user picked.
 */
export function presetMatches(
  preset: AppearancePreset,
  current: AppearanceCurrent
): boolean {
  const target = presetToApplication(preset)
  if (target.themeColor !== current.themeColor) return false
  if (!sameOverrides(target.customTheme.light, current.customTheme.light)) {
    return false
  }
  if (!sameOverrides(target.customTheme.dark, current.customTheme.dark)) {
    return false
  }
  if (target.uiFont && target.uiFont !== current.uiFontId) return false
  if (target.monoFont && target.monoFont !== current.monoFontId) return false
  if (
    target.codeTheme &&
    (target.codeTheme.light !== current.codeTheme.light ||
      target.codeTheme.dark !== current.codeTheme.dark)
  ) {
    return false
  }
  return true
}

export function findMatchingPreset(
  presets: readonly AppearancePreset[],
  current: AppearanceCurrent
): AppearancePreset | null {
  return presets.find((preset) => presetMatches(preset, current)) ?? null
}

/** Default code theme, exported here so the UI has one import for the format. */
export { DEFAULT_CODE_THEME }
