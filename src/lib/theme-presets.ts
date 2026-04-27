// src/lib/theme-presets.ts

/**
 * 12 个 shadcn 官方主题预设的标识符。
 * 实际 CSS 变量值定义在 src/app/globals.css 的 [data-theme="..."] 选择器中。
 */
export const THEME_COLORS = [
  "neutral",
  "zinc",
  "slate",
  "stone",
  "gray",
  "red",
  "rose",
  "orange",
  "green",
  "blue",
  "yellow",
  "violet",
] as const

export type ThemeColor = (typeof THEME_COLORS)[number]

/**
 * 默认主题色。选用 "neutral" 是因为它对应当前 globals.css 的现存 :root 值
 * （所有 chroma=0 的纯灰阶），可保证升级后视觉零差异。
 */
export const DEFAULT_THEME_COLOR: ThemeColor = "neutral"

/**
 * UI 预览用的代表色（OKLch 字符串，对应各预设的 primary 色 light 版本）。
 * 仅用于 Appearance 页面的"色盘圆点"按钮渲染，不会被写入真实样式。
 *
 * 选择 light primary 而非其他变量，是因为 primary 是各预设视觉差异最大的部分。
 * 这些值必须硬编码（不能通过 var(--primary) 读取），因为每个圆点要永远显示
 * 自己对应预设的代表色，不能跟随当前激活的主题色。
 */
export const THEME_COLOR_PREVIEW: Record<ThemeColor, string> = {
  neutral: "oklch(0.205 0 0)",
  zinc: "oklch(0.21 0.006 285.885)",
  slate: "oklch(0.208 0.042 265.755)",
  stone: "oklch(0.216 0.006 56.043)",
  gray: "oklch(0.21 0.034 264.665)",
  red: "oklch(0.637 0.237 25.331)",
  rose: "oklch(0.645 0.246 16.439)",
  orange: "oklch(0.705 0.213 47.604)",
  green: "oklch(0.723 0.219 149.579)",
  blue: "oklch(0.546 0.245 262.881)",
  yellow: "oklch(0.795 0.184 86.047)",
  violet: "oklch(0.606 0.25 292.717)",
}

/**
 * 缩放档位（百分比）。100 是默认。
 * 选用离散档位而非连续滑块，是为了与现有 ThemeMode 选择器保持视觉一致。
 */
export const ZOOM_LEVELS = [80, 90, 100, 110, 125, 150] as const

export type ZoomLevel = (typeof ZOOM_LEVELS)[number]

export const DEFAULT_ZOOM_LEVEL: ZoomLevel = 100

export type FontFamilyPreference = string | null

export const DEFAULT_UI_FONT_FAMILY: FontFamilyPreference = null
export const DEFAULT_CODE_FONT_FAMILY: FontFamilyPreference = null

export const UI_FONT_FALLBACK_STACK =
  "Inter, Avenir, Helvetica, Arial, sans-serif"
export const CODE_FONT_FALLBACK_STACK =
  'Menlo, Monaco, "Courier New", monospace'

export const MAX_FONT_FAMILY_NAME_LENGTH = 128

export const BUILT_IN_UI_FONT_FAMILIES = [
  "system-ui",
  "ui-sans-serif",
  "Arial",
  "Helvetica",
  "sans-serif",
] as const

export const BUILT_IN_CODE_FONT_FAMILIES = [
  "ui-monospace",
  "Menlo",
  "Monaco",
  "Courier New",
  "monospace",
] as const

export const BUILT_IN_FONT_FAMILY_OPTIONS = [
  ...BUILT_IN_UI_FONT_FAMILIES,
  ...BUILT_IN_CODE_FONT_FAMILIES,
] as const

const CSS_GENERIC_FONT_FAMILIES = new Set<string>([
  "sans-serif",
  "monospace",
  "system-ui",
  "ui-sans-serif",
  "ui-monospace",
])

const BUILT_IN_FONT_FAMILY_KEYS = new Set<string>(
  BUILT_IN_FONT_FAMILY_OPTIONS.map((family) => family.toLowerCase())
)

export function normalizeFontFamilyPreference(
  value: unknown
): FontFamilyPreference {
  if (typeof value !== "string") return null

  const trimmed = value.trim()
  if (
    !trimmed ||
    trimmed.startsWith(".") ||
    [...trimmed].length > MAX_FONT_FAMILY_NAME_LENGTH ||
    [...trimmed].some((char) => {
      const code = char.charCodeAt(0)
      return code < 32 || code === 127
    })
  ) {
    return null
  }

  return trimmed
}

export function isBuiltInFontFamilyOption(family: string): boolean {
  return BUILT_IN_FONT_FAMILY_KEYS.has(family.trim().toLowerCase())
}

function formatFontFamilyForCss(family: string): string {
  const normalized = normalizeFontFamilyPreference(family)
  if (!normalized) return ""
  if (CSS_GENERIC_FONT_FAMILIES.has(normalized.toLowerCase())) {
    return normalized
  }
  return JSON.stringify(normalized)
}

export function buildFontFamilyStack(
  selectedFont: FontFamilyPreference,
  fallbackStack: string
): string {
  const normalized = normalizeFontFamilyPreference(selectedFont)
  if (!normalized) return fallbackStack
  return `${formatFontFamilyForCss(normalized)}, ${fallbackStack}`
}

export function buildUiFontFamilyStack(
  selectedFont: FontFamilyPreference
): string {
  return buildFontFamilyStack(selectedFont, UI_FONT_FALLBACK_STACK)
}

export function buildCodeFontFamilyStack(
  selectedFont: FontFamilyPreference
): string {
  return buildFontFamilyStack(selectedFont, CODE_FONT_FALLBACK_STACK)
}
