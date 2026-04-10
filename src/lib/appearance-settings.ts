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

  // Apply font sizes
  root.style.setProperty("--ui-font-size", `${settings.uiFontSize}px`)
  root.style.setProperty("--code-font-size", `${settings.codeFontSize}px`)

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

/** Read the current code font size from the CSS variable on :root. */
export function getCodeFontSize(): number {
  if (typeof document === "undefined") return DEFAULT_APPEARANCE.codeFontSize
  const value =
    document.documentElement.style.getPropertyValue("--code-font-size")
  if (!value) return DEFAULT_APPEARANCE.codeFontSize
  const parsed = parseInt(value, 10)
  return Number.isNaN(parsed) ? DEFAULT_APPEARANCE.codeFontSize : parsed
}
