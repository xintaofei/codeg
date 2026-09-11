// src/lib/appearance-preset-gallery.ts
//
// The preset gallery: an index of shareable appearance presets published as
// one static JSON file (a public repository by default), the preset files it
// points at, and the local library of installed ones.
//
// Trust, in order:
//
// 1. The backend fetches (https only, size-capped, time-limited) and refuses a
//    preset file whose SHA-256 differs from the digest the index listed.
// 2. This module validates the index strictly (unknown keys at the top level
//    are errors; a listing that fails validation is skipped and counted),
//    resolves each file URL against the index URL and keeps only same-origin
//    entries, compares the digest again, and runs every downloaded file
//    through `parseAppearancePreset`, the validator an imported file goes
//    through, before anything is applied or stored. A file whose `id` differs
//    from its listing is refused too.
// 3. Installed presets are plain preset documents in localStorage together
//    with the URL and digest they came from. "Update available" is a digest
//    comparison against the current index; removing one deletes the local copy
//    and nothing else.
//
// Nothing here fetches on a timer. The index is loaded when the gallery is
// opened without a usable saved copy or with one a day old, and on request.

import { getTransport } from "@/lib/transport"
import {
  STORAGE_KEY_PRESET_GALLERY_CACHE,
  STORAGE_KEY_PRESET_GALLERY_INDEX_URL,
  STORAGE_KEY_PRESET_GALLERY_INSTALLED,
} from "./appearance-script"
import {
  MAX_PRESET_BYTES,
  PRESET_LIMITS,
  PRESET_MODES,
  isPresetColorValue,
  parseAppearancePreset,
  validateAppearancePreset,
  type AppearancePreset,
  type PresetIssue,
  type PresetIssueCode,
  type PresetMode,
} from "./appearance-preset"
import { byteLengthOf } from "./custom-style"
import {
  THEME_COLORS,
  THEME_COLOR_PREVIEW,
  THEME_COLOR_TITLE,
  type ThemeColor,
} from "./theme-presets"

// ─── Format ───

export const GALLERY_INDEX_SCHEMA_VERSION = 1 as const

/** Index cap, checked before parse. Mirrors `MAX_INDEX_BYTES` in the backend. */
export const MAX_GALLERY_INDEX_BYTES = 256 * 1024
export const MAX_GALLERY_ENTRIES = 500
/** Colours per mode a listing may show on its card. */
export const MAX_GALLERY_SWATCHES = 6
/** A saved index older than this is refreshed when the gallery opens. */
export const GALLERY_CACHE_STALE_MS = 24 * 60 * 60 * 1000

/**
 * The index the gallery reads when no other source is set. Presets in it are
 * the seven bundled looks; the repository documents how to submit one.
 */
export const DEFAULT_PRESET_GALLERY_INDEX_URL =
  "https://raw.githubusercontent.com/Adam-Dalloul/codeg-presets/main/index.json"

/** One listing: what the card shows, where the file is, what it must hash to. */
export type GalleryEntry = {
  id: string
  name: string
  description?: string
  author?: string
  version?: string
  base?: ThemeColor
  mode?: PresetMode
  /** Absolute https URL of the preset file, on the index's origin. */
  url: string
  /** Lowercase hex SHA-256 of the file's bytes. */
  sha256: string
  bytes?: number
  /** Card colours per mode, from the preset's own palette. */
  swatches?: { light?: string[]; dark?: string[] }
}

export type GalleryIndex = {
  schemaVersion: typeof GALLERY_INDEX_SCHEMA_VERSION
  name?: string
  updatedAt?: string
  presets: GalleryEntry[]
}

export type GalleryIssueCode =
  | PresetIssueCode
  | "invalid-url"
  | "cross-origin"
  | "invalid-digest"
  | "duplicate-id"
  | "too-many"

export type GalleryIssue = {
  path: string
  code: GalleryIssueCode
  detail?: string
}

export type GalleryIndexParseResult =
  | { ok: true; index: GalleryIndex; skipped: GalleryIssue[] }
  | { ok: false; issues: GalleryIssue[] }

// ─── Validation ───

const INDEX_KEYS = [
  "$schema",
  "schemaVersion",
  "name",
  "updatedAt",
  "presets",
] as const

const ENTRY_KEYS = [
  "id",
  "name",
  "description",
  "author",
  "version",
  "base",
  "mode",
  "url",
  "sha256",
  "bytes",
  "swatches",
] as const

const SWATCH_KEYS = ["light", "dark"] as const

/**
 * Mirror `ID_RE` / `VERSION_RE` in appearance-preset.ts. The listing is only
 * a promise about the file; the file itself is re-checked by that module.
 */
const ID_RE = /^[a-z0-9][a-z0-9-]*$/
const VERSION_RE = /^[0-9A-Za-z.+-]+$/
const SHA256_RE = /^[0-9a-f]{64}$/i
const MAX_URL_CHARS = 2048
const MAX_INDEX_NAME_CHARS = 80
const MAX_UPDATED_AT_CHARS = 40

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function join(path: string, key: string): string {
  return path ? `${path}.${key}` : key
}

class Checker {
  readonly issues: GalleryIssue[] = []

  add(path: string, code: GalleryIssueCode, detail?: string): void {
    this.issues.push(detail ? { path, code, detail } : { path, code })
  }

  keys(
    record: Record<string, unknown>,
    allowed: readonly string[],
    path: string
  ): void {
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
}

/** A hostname the backend would refuse too: an IP literal or a loopback name. */
function isIpOrLocalHost(hostname: string): boolean {
  const lower = hostname.toLowerCase()
  if (lower === "localhost" || lower.endsWith(".localhost")) return true
  if (lower.startsWith("[")) return true // bracketed IPv6
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(lower)
}

/**
 * The URL a preset file listing points at, absolute and https, or null. A
 * relative `url` resolves against the index, so a repository can list
 * `presets/<id>.codeg-preset.json` and move as a whole.
 */
export function resolveGalleryFileUrl(
  raw: string,
  indexUrl: string
): string | null {
  try {
    const resolved = new URL(raw, indexUrl)
    if (resolved.protocol !== "https:") return null
    if (resolved.username || resolved.password) return null
    if (isIpOrLocalHost(resolved.hostname)) return null
    resolved.hash = ""
    return resolved.href
  } catch {
    return null
  }
}

/**
 * A gallery source as the user typed it, normalized, or null when it is not
 * an https URL to a public host without credentials.
 */
export function normalizeGalleryIndexUrl(raw: string): string | null {
  const trimmed = raw.trim()
  if (!trimmed || trimmed.length > MAX_URL_CHARS) return null
  try {
    const url = new URL(trimmed)
    if (url.protocol !== "https:") return null
    if (url.username || url.password) return null
    if (isIpOrLocalHost(url.hostname)) return null
    url.hash = ""
    return url.href
  } catch {
    return null
  }
}

type EntryResult =
  | { ok: true; entry: GalleryEntry }
  | { ok: false; issues: GalleryIssue[] }

function validateGalleryEntry(
  raw: unknown,
  path: string,
  indexUrl: string,
  origin: string
): EntryResult {
  const c = new Checker()
  const record = c.object(raw, path, true)
  if (!record) return { ok: false, issues: c.issues }
  c.keys(record, ENTRY_KEYS, path)

  const id = c.string(record.id, join(path, "id"), {
    required: true,
    max: PRESET_LIMITS.id,
    pattern: ID_RE,
  })
  const name = c.string(record.name, join(path, "name"), {
    required: true,
    max: PRESET_LIMITS.name,
  })
  const description = c.string(record.description, join(path, "description"), {
    required: false,
    max: PRESET_LIMITS.description,
  })
  const author = c.string(record.author, join(path, "author"), {
    required: false,
    max: PRESET_LIMITS.author,
  })
  const version = c.string(record.version, join(path, "version"), {
    required: false,
    max: PRESET_LIMITS.version,
    pattern: VERSION_RE,
  })

  let base: ThemeColor | undefined
  if (record.base !== undefined) {
    if (typeof record.base !== "string") {
      c.add(join(path, "base"), "wrong-type")
    } else if (!(THEME_COLORS as readonly string[]).includes(record.base)) {
      c.add(join(path, "base"), "unknown-base")
    } else {
      base = record.base as ThemeColor
    }
  }

  let mode: PresetMode | undefined
  if (record.mode !== undefined) {
    if (typeof record.mode !== "string") {
      c.add(join(path, "mode"), "wrong-type")
    } else if (!(PRESET_MODES as readonly string[]).includes(record.mode)) {
      c.add(join(path, "mode"), "invalid-value")
    } else {
      mode = record.mode as PresetMode
    }
  }

  let url: string | undefined
  const urlPath = join(path, "url")
  const rawUrl = c.string(record.url, urlPath, {
    required: true,
    max: MAX_URL_CHARS,
  })
  if (rawUrl) {
    const resolved = resolveGalleryFileUrl(rawUrl, indexUrl)
    if (!resolved) {
      c.add(urlPath, "invalid-url")
    } else if (new URL(resolved).origin !== origin) {
      // The user chose one site to trust; a listing cannot widen that.
      c.add(urlPath, "cross-origin")
    } else {
      url = resolved
    }
  }

  let sha256: string | undefined
  const digestPath = join(path, "sha256")
  const rawDigest = c.string(record.sha256, digestPath, {
    required: true,
    max: 64,
  })
  if (rawDigest !== undefined) {
    if (SHA256_RE.test(rawDigest)) sha256 = rawDigest.toLowerCase()
    else c.add(digestPath, "invalid-digest")
  }

  let bytes: number | undefined
  if (record.bytes !== undefined) {
    const bytesPath = join(path, "bytes")
    if (typeof record.bytes !== "number" || !Number.isInteger(record.bytes)) {
      c.add(bytesPath, "wrong-type")
    } else if (record.bytes < 1 || record.bytes > MAX_PRESET_BYTES) {
      c.add(bytesPath, "out-of-range", `1 to ${MAX_PRESET_BYTES}`)
    } else {
      bytes = record.bytes
    }
  }

  let swatches: GalleryEntry["swatches"]
  const swatchPath = join(path, "swatches")
  const swatchRecord = c.object(record.swatches, swatchPath, false)
  if (swatchRecord) {
    c.keys(swatchRecord, SWATCH_KEYS, swatchPath)
    swatches = {}
    for (const key of SWATCH_KEYS) {
      const list = swatchRecord[key]
      if (list === undefined) continue
      const listPath = join(swatchPath, key)
      if (!Array.isArray(list)) {
        c.add(listPath, "wrong-type")
        continue
      }
      if (list.length < 1 || list.length > MAX_GALLERY_SWATCHES) {
        c.add(listPath, "out-of-range", `1 to ${MAX_GALLERY_SWATCHES} colors`)
        continue
      }
      const colors: string[] = []
      list.forEach((color, index) => {
        const colorPath = `${listPath}[${index}]`
        if (typeof color !== "string") {
          c.add(colorPath, "wrong-type")
          return
        }
        const trimmed = color.trim()
        if (!isPresetColorValue(trimmed)) {
          c.add(colorPath, "invalid-color")
          return
        }
        colors.push(trimmed)
      })
      if (colors.length === list.length) swatches[key] = colors
    }
  }

  if (c.issues.length > 0 || !id || !name || !url || !sha256) {
    return { ok: false, issues: c.issues }
  }
  const entry: GalleryEntry = { id, name, url, sha256 }
  if (description) entry.description = description
  if (author) entry.author = author
  if (version) entry.version = version
  if (base) entry.base = base
  if (mode) entry.mode = mode
  if (bytes !== undefined) entry.bytes = bytes
  if (swatches && Object.keys(swatches).length > 0) entry.swatches = swatches
  return { ok: true, entry }
}

/**
 * Validate an already-parsed index. The document must be well-formed as a
 * whole (version, keys, a `presets` array); each listing then stands or falls
 * on its own, so one bad entry hides itself rather than the gallery.
 */
export function validatePresetGalleryIndex(
  input: unknown,
  indexUrl: string
): GalleryIndexParseResult {
  const c = new Checker()
  const doc = c.object(input, "", true)
  if (!doc) return { ok: false, issues: c.issues }
  c.keys(doc, INDEX_KEYS, "")

  const version = doc.schemaVersion
  if (version === undefined) {
    c.add("schemaVersion", "missing")
  } else if (typeof version !== "number" || !Number.isInteger(version)) {
    c.add("schemaVersion", "wrong-type")
  } else if (version !== GALLERY_INDEX_SCHEMA_VERSION) {
    c.add(
      "schemaVersion",
      "unsupported-version",
      version > GALLERY_INDEX_SCHEMA_VERSION
        ? `newer than ${GALLERY_INDEX_SCHEMA_VERSION}`
        : undefined
    )
  }
  if (doc.$schema !== undefined && typeof doc.$schema !== "string") {
    c.add("$schema", "wrong-type")
  }
  const name = c.string(doc.name, "name", {
    required: false,
    max: MAX_INDEX_NAME_CHARS,
  })
  const updatedAt = c.string(doc.updatedAt, "updatedAt", {
    required: false,
    max: MAX_UPDATED_AT_CHARS,
  })

  let origin: string | null = null
  try {
    origin = new URL(indexUrl).origin
  } catch {
    c.add("", "invalid-url", "index URL")
  }

  const rawPresets = doc.presets
  if (rawPresets === undefined) {
    c.add("presets", "missing")
  } else if (!Array.isArray(rawPresets)) {
    c.add("presets", "wrong-type")
  } else if (rawPresets.length > MAX_GALLERY_ENTRIES) {
    c.add("presets", "too-many", `max ${MAX_GALLERY_ENTRIES}`)
  }
  if (c.issues.length > 0 || !Array.isArray(rawPresets) || origin === null) {
    return { ok: false, issues: c.issues }
  }

  const presets: GalleryEntry[] = []
  const skipped: GalleryIssue[] = []
  const seen = new Set<string>()
  rawPresets.forEach((raw, index) => {
    const path = `presets[${index}]`
    const result = validateGalleryEntry(raw, path, indexUrl, origin)
    if (!result.ok) {
      skipped.push(...result.issues)
      return
    }
    if (seen.has(result.entry.id)) {
      skipped.push({
        path: join(path, "id"),
        code: "duplicate-id",
        detail: result.entry.id,
      })
      return
    }
    seen.add(result.entry.id)
    presets.push(result.entry)
  })

  const index: GalleryIndex = {
    schemaVersion: GALLERY_INDEX_SCHEMA_VERSION,
    presets,
  }
  if (name) index.name = name
  if (updatedAt) index.updatedAt = updatedAt
  return { ok: true, index, skipped }
}

/** Index text from the network or the saved copy -> a validated index. */
export function parsePresetGalleryIndex(
  text: string,
  indexUrl: string
): GalleryIndexParseResult {
  const bytes = byteLengthOf(text)
  if (bytes > MAX_GALLERY_INDEX_BYTES) {
    return {
      ok: false,
      issues: [
        {
          path: "",
          code: "too-large",
          detail: `${bytes} > ${MAX_GALLERY_INDEX_BYTES}`,
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
  return validatePresetGalleryIndex(value, indexUrl)
}

/** Case-insensitive match on id, name, author and description. */
export function filterGalleryEntries(
  entries: readonly GalleryEntry[],
  query: string
): GalleryEntry[] {
  const needle = query.trim().toLowerCase()
  if (!needle) return [...entries]
  return entries.filter((entry) =>
    [entry.id, entry.name, entry.author, entry.description].some((field) =>
      field?.toLowerCase().includes(needle)
    )
  )
}

// ─── Swatches ───

function neutralSwatches(dark: boolean): [string, string, string] {
  return dark
    ? ["oklch(0.145 0 0)", "oklch(0.205 0 0)", "oklch(0.985 0 0)"]
    : ["oklch(1 0 0)", "oklch(0.985 0 0)", "oklch(0.145 0 0)"]
}

function basePrimary(base: ThemeColor, dark: boolean): string {
  return dark ? THEME_COLOR_TITLE[base].dark : THEME_COLOR_PREVIEW[base]
}

/**
 * Four swatches that stand for a preset, for the mode the app is in:
 * background, sidebar, primary, text. Explicit preset colours win; the
 * fallbacks are the base palette's primary and the neutral surfaces, close for
 * every built-in base.
 */
export function presetSwatches(
  preset: AppearancePreset,
  dark: boolean
): string[] {
  const colors = (dark ? preset.colors.dark : preset.colors.light) ?? {}
  const [background, sidebar, foreground] = neutralSwatches(dark)
  return [
    colors.background ?? background,
    colors.sidebar ?? sidebar,
    colors.primary ?? basePrimary(preset.base, dark),
    colors.foreground ?? foreground,
  ]
}

/**
 * The card strip for a listing the user has not installed: the swatches the
 * index carries for the mode, else the same fallbacks as a preset with no
 * colour overrides on its base palette.
 */
export function galleryEntrySwatches(
  entry: Pick<GalleryEntry, "base" | "swatches">,
  dark: boolean
): string[] {
  const listed = dark ? entry.swatches?.dark : entry.swatches?.light
  if (listed && listed.length > 0) return listed
  const [background, sidebar, foreground] = neutralSwatches(dark)
  return [
    background,
    sidebar,
    basePrimary(entry.base ?? "neutral", dark),
    foreground,
  ]
}

// ─── Storage helpers ───

function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function persist(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key)
    else localStorage.setItem(key, value)
  } catch {
    // localStorage unavailable; this session still works from memory
  }
}

// ─── Gallery source ───

/** The index URL in use: the stored one when it validates, else the default. */
export function readGalleryIndexUrl(): string {
  const stored = readStored(STORAGE_KEY_PRESET_GALLERY_INDEX_URL)
  return (
    (stored && normalizeGalleryIndexUrl(stored)) ??
    DEFAULT_PRESET_GALLERY_INDEX_URL
  )
}

/** Store a validated index URL; the default is stored as "nothing set". */
export function writeGalleryIndexUrl(url: string): void {
  persist(
    STORAGE_KEY_PRESET_GALLERY_INDEX_URL,
    url === DEFAULT_PRESET_GALLERY_INDEX_URL ? null : url
  )
}

// ─── Installed library ───

/** An installed preset with where it came from. */
export type InstalledGalleryPreset = {
  preset: AppearancePreset
  url: string
  indexUrl: string
  sha256: string
  /** ISO timestamp of the install or last update. */
  installedAt: string
}

/**
 * The stored library. Each preset runs the full validation, so a document
 * written by an older build that no longer validates is forgotten rather
 * than trusted; the first entry wins a duplicate id.
 */
export function parseInstalledGalleryPresets(
  raw: string | null
): InstalledGalleryPreset[] {
  if (!raw) return []
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    return []
  }
  if (!Array.isArray(value)) return []
  const out: InstalledGalleryPreset[] = []
  const seen = new Set<string>()
  for (const item of value) {
    if (!isRecord(item)) continue
    const parsed = validateAppearancePreset(item.preset)
    if (!parsed.ok) continue
    const { url, indexUrl, sha256, installedAt } = item
    if (
      typeof url !== "string" ||
      typeof indexUrl !== "string" ||
      typeof sha256 !== "string" ||
      typeof installedAt !== "string" ||
      !SHA256_RE.test(sha256)
    ) {
      continue
    }
    if (seen.has(parsed.preset.id)) continue
    seen.add(parsed.preset.id)
    out.push({
      preset: parsed.preset,
      url,
      indexUrl,
      sha256: sha256.toLowerCase(),
      installedAt,
    })
  }
  return out
}

export function serializeInstalledGalleryPresets(
  list: readonly InstalledGalleryPreset[]
): string {
  return JSON.stringify(list)
}

export function readInstalledGalleryPresets(): InstalledGalleryPreset[] {
  return parseInstalledGalleryPresets(
    readStored(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
  )
}

export function writeInstalledGalleryPresets(
  list: readonly InstalledGalleryPreset[]
): void {
  persist(
    STORAGE_KEY_PRESET_GALLERY_INSTALLED,
    serializeInstalledGalleryPresets(list)
  )
}

/** Install or update: replace by preset id, else append. */
export function upsertInstalledGalleryPreset(
  list: readonly InstalledGalleryPreset[],
  item: InstalledGalleryPreset
): InstalledGalleryPreset[] {
  const index = list.findIndex((i) => i.preset.id === item.preset.id)
  if (index < 0) return [...list, item]
  const next = [...list]
  next[index] = item
  return next
}

/** Remove deletes the local copy and nothing else. */
export function removeInstalledGalleryPreset(
  list: readonly InstalledGalleryPreset[],
  id: string
): InstalledGalleryPreset[] {
  return list.filter((item) => item.preset.id !== id)
}

export type GalleryEntryStatus = "available" | "installed" | "update"

/** "Update available" is a digest comparison, nothing more. */
export function galleryEntryStatus(
  entry: Pick<GalleryEntry, "sha256">,
  installed: Pick<InstalledGalleryPreset, "sha256"> | undefined
): GalleryEntryStatus {
  if (!installed) return "available"
  return installed.sha256 === entry.sha256 ? "installed" : "update"
}

// ─── Saved index ───

export type GalleryCache = {
  indexUrl: string
  /** ISO timestamp of the fetch. */
  fetchedAt: string
  text: string
}

/** The saved copy, only when it is a copy of `indexUrl` and within the cap. */
export function parseGalleryCache(
  raw: string | null,
  indexUrl: string
): GalleryCache | null {
  if (!raw) return null
  try {
    const value: unknown = JSON.parse(raw)
    if (!isRecord(value)) return null
    if (value.indexUrl !== indexUrl) return null
    if (typeof value.fetchedAt !== "string" || typeof value.text !== "string") {
      return null
    }
    if (byteLengthOf(value.text) > MAX_GALLERY_INDEX_BYTES) return null
    return { indexUrl, fetchedAt: value.fetchedAt, text: value.text }
  } catch {
    return null
  }
}

export function readGalleryCache(indexUrl: string): GalleryCache | null {
  return parseGalleryCache(
    readStored(STORAGE_KEY_PRESET_GALLERY_CACHE),
    indexUrl
  )
}

export function writeGalleryCache(cache: GalleryCache): void {
  persist(STORAGE_KEY_PRESET_GALLERY_CACHE, JSON.stringify(cache))
}

export function isGalleryCacheStale(
  fetchedAt: string,
  now: number = Date.now()
): boolean {
  const at = Date.parse(fetchedAt)
  return !Number.isFinite(at) || now - at > GALLERY_CACHE_STALE_MS
}

// ─── Transport bindings ───

/** camelCase mirror of the Rust `GalleryDocument`. */
export type GalleryDocument = {
  text: string
  sha256: string
  bytes: number
}

export async function fetchGalleryIndexDocument(
  url: string
): Promise<GalleryDocument> {
  return getTransport().call("preset_gallery_fetch_index", { url })
}

export async function fetchGalleryPresetDocument(
  url: string,
  sha256: string
): Promise<GalleryDocument> {
  return getTransport().call("preset_gallery_fetch_preset", { url, sha256 })
}

// ─── Operations ───

export type GalleryRefreshResult =
  | {
      ok: true
      index: GalleryIndex
      skipped: GalleryIssue[]
      fetchedAt: string
    }
  | { ok: false; issues: GalleryIssue[] }

/**
 * Fetch and validate the index at `indexUrl`. A valid one becomes the saved
 * copy; an invalid one leaves the previous saved copy alone. Transport and
 * backend refusals (offline, https policy, size cap) throw.
 */
export async function refreshGalleryIndex(
  indexUrl: string,
  now: () => number = Date.now
): Promise<GalleryRefreshResult> {
  const doc = await fetchGalleryIndexDocument(indexUrl)
  const parsed = parsePresetGalleryIndex(doc.text, indexUrl)
  if (!parsed.ok) return parsed
  const fetchedAt = new Date(now()).toISOString()
  writeGalleryCache({ indexUrl, fetchedAt, text: doc.text })
  return { ok: true, index: parsed.index, skipped: parsed.skipped, fetchedAt }
}

export type GalleryDownloadResult =
  | { ok: true; preset: AppearancePreset; sha256: string }
  | { ok: false; code: "hash-mismatch" | "id-mismatch" }
  | { ok: false; code: "invalid-preset"; issues: PresetIssue[] }

/**
 * Download one listed file. The backend has already refused a digest
 * mismatch; the digest is compared again here, then the text goes through the
 * preset validator, then the file's id must be the listing's. Nothing is
 * applied or stored by this function.
 */
export async function downloadGalleryPreset(
  entry: GalleryEntry
): Promise<GalleryDownloadResult> {
  const doc = await fetchGalleryPresetDocument(entry.url, entry.sha256)
  const sha256 = doc.sha256.toLowerCase()
  if (sha256 !== entry.sha256) return { ok: false, code: "hash-mismatch" }
  const parsed = parseAppearancePreset(doc.text)
  if (!parsed.ok)
    return { ok: false, code: "invalid-preset", issues: parsed.issues }
  if (parsed.preset.id !== entry.id) return { ok: false, code: "id-mismatch" }
  return { ok: true, preset: parsed.preset, sha256 }
}

/** The library record for a verified download. */
export function installedFromDownload(
  entry: GalleryEntry,
  download: { preset: AppearancePreset; sha256: string },
  indexUrl: string,
  now: number = Date.now()
): InstalledGalleryPreset {
  return {
    preset: download.preset,
    url: entry.url,
    indexUrl,
    sha256: download.sha256,
    installedAt: new Date(now).toISOString(),
  }
}
