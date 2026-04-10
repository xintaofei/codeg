import { THEME_COLOR_PRESETS } from "./theme-color-presets"

export type ThemeColor =
  | "zinc"
  | "slate"
  | "blue"
  | "green"
  | "violet"
  | "orange"
  | "rose"

export interface AppearanceSettings {
  themeColor: ThemeColor
  uiFontSize: number
  codeFontSize: number
}

export const THEME_COLORS: ThemeColor[] = [
  "zinc",
  "slate",
  "blue",
  "green",
  "violet",
  "orange",
  "rose",
]

export const DEFAULT_APPEARANCE: AppearanceSettings = {
  themeColor: "zinc",
  uiFontSize: 14,
  codeFontSize: 13,
}

export const UI_FONT_SIZE_MIN = 12
export const UI_FONT_SIZE_MAX = 20
export const CODE_FONT_SIZE_MIN = 10
export const CODE_FONT_SIZE_MAX = 24

export const APPEARANCE_STORAGE_KEY = "settings:appearance:v1"
export const APPEARANCE_UPDATED_EVENT = "codeg:appearance-updated"

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max)
}

function normalizeSettings(input: unknown): AppearanceSettings {
  const next = { ...DEFAULT_APPEARANCE }
  if (!input || typeof input !== "object") return next
  const record = input as Record<string, unknown>

  if (
    typeof record.themeColor === "string" &&
    THEME_COLORS.includes(record.themeColor as ThemeColor)
  ) {
    next.themeColor = record.themeColor as ThemeColor
  }

  if (typeof record.uiFontSize === "number") {
    next.uiFontSize = clamp(
      Math.round(record.uiFontSize),
      UI_FONT_SIZE_MIN,
      UI_FONT_SIZE_MAX
    )
  }

  if (typeof record.codeFontSize === "number") {
    next.codeFontSize = clamp(
      Math.round(record.codeFontSize),
      CODE_FONT_SIZE_MIN,
      CODE_FONT_SIZE_MAX
    )
  }

  return next
}

export function readAppearanceSettings(): AppearanceSettings {
  if (typeof window === "undefined") return { ...DEFAULT_APPEARANCE }
  try {
    const raw = window.localStorage.getItem(APPEARANCE_STORAGE_KEY)
    if (!raw) return { ...DEFAULT_APPEARANCE }
    return normalizeSettings(JSON.parse(raw))
  } catch {
    return { ...DEFAULT_APPEARANCE }
  }
}

export function writeAppearanceSettings(settings: AppearanceSettings): void {
  if (typeof window === "undefined") return
  const normalized = normalizeSettings(settings)
  try {
    window.localStorage.setItem(
      APPEARANCE_STORAGE_KEY,
      JSON.stringify(normalized)
    )
    window.dispatchEvent(new Event(APPEARANCE_UPDATED_EVENT))
  } catch {
    // Ignore storage failures
  }
}

/** Apply appearance settings to the document root as CSS custom properties. */
export function applyAppearanceSettings(settings: AppearanceSettings): void {
  if (typeof document === "undefined") return

  const root = document.documentElement
  const isDark =
    root.classList.contains("dark") ||
    (!root.classList.contains("light") &&
      window.matchMedia("(prefers-color-scheme: dark)").matches)

  // UI font size: scale via root font-size AND zoom for complete coverage.
  // - root font-size affects all rem-based Tailwind utilities (text-sm, etc.)
  // - zoom affects absolute px values (text-[13px], etc.)
  const zoom = settings.uiFontSize / DEFAULT_APPEARANCE.uiFontSize
  root.style.setProperty("--ui-font-size", `${settings.uiFontSize}px`)
  root.style.fontSize = `${settings.uiFontSize}px`
  // Try zoom (works in Chromium/Safari); harmless no-op if unsupported
  try {
    root.style.setProperty("zoom", String(zoom))
  } catch {
    // Fallback: font-size alone handles rem-based values
  }

  // Code font size: store as CSS variable; getCodeFontSize() compensates
  // for zoom so Monaco/terminal render at the intended pixel size
  root.style.setProperty("--code-font-size", `${settings.codeFontSize}px`)
  root.style.setProperty("--ui-zoom", String(zoom))

  // Clear previous theme color overrides
  const allVars = new Set<string>()
  for (const preset of THEME_COLOR_PRESETS) {
    for (const key of Object.keys(preset.light)) allVars.add(key)
    for (const key of Object.keys(preset.dark)) allVars.add(key)
  }
  for (const v of allVars) {
    root.style.removeProperty(v)
  }

  // Apply current theme color
  const preset = THEME_COLOR_PRESETS.find((p) => p.name === settings.themeColor)
  if (preset) {
    const overrides = isDark ? preset.dark : preset.light
    for (const [key, value] of Object.entries(overrides)) {
      root.style.setProperty(key, value)
    }
  }
}

/**
 * Read the current code font size, compensated for UI zoom so that
 * Monaco/terminal render at the user's intended pixel size.
 */
export function getCodeFontSize(): number {
  if (typeof document === "undefined") return DEFAULT_APPEARANCE.codeFontSize
  const root = document.documentElement
  const raw = root.style.getPropertyValue("--code-font-size")
  const zoom = parseFloat(root.style.getPropertyValue("--ui-zoom")) || 1
  const size = raw ? parseInt(raw, 10) : DEFAULT_APPEARANCE.codeFontSize
  if (Number.isNaN(size)) return DEFAULT_APPEARANCE.codeFontSize
  return Math.round(size / zoom)
}
