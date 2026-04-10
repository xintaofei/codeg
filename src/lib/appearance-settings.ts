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
