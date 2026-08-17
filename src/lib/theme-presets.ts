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

export const FOLDER_THEME_COLOR_INHERIT = "inherit" as const

export type FolderThemeColor = ThemeColor | typeof FOLDER_THEME_COLOR_INHERIT

const THEME_COLOR_SET = new Set<string>(THEME_COLORS)

/**
 * 早期版本的文件夹颜色存储的是十六进制值；迁移映射到最接近的主题预设。
 */
const LEGACY_FOLDER_COLOR_MAP: Record<string, FolderThemeColor> = {
  foreground: FOLDER_THEME_COLOR_INHERIT,
  "#ef4444": "red",
  "#f97316": "orange",
  "#eab308": "yellow",
  "#84cc16": "green",
  "#22c55e": "green",
  "#06b6d4": "blue",
  "#8b5cf6": "violet",
  "#d946ef": "rose",
  "#ec4899": "rose",
}

/**
 * 把 FolderDetail.color 的原始存储值（预设名或遗留十六进制）规约成
 * FolderThemeColor。未知值回退 inherit。
 */
export function normalizeFolderThemeColor(
  color: string | null | undefined
): FolderThemeColor {
  if (!color) return FOLDER_THEME_COLOR_INHERIT
  const normalized = color.toLowerCase()
  if (normalized === FOLDER_THEME_COLOR_INHERIT) {
    return FOLDER_THEME_COLOR_INHERIT
  }
  if (THEME_COLOR_SET.has(normalized)) return normalized as ThemeColor
  return LEGACY_FOLDER_COLOR_MAP[normalized] ?? FOLDER_THEME_COLOR_INHERIT
}

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
export const ZOOM_LEVELS = [
  80, 90, 100, 110, 125, 150, 175, 200, 250, 300,
] as const

export type ZoomLevel = (typeof ZOOM_LEVELS)[number]

export const DEFAULT_ZOOM_LEVEL: ZoomLevel = 100

/** Next discrete Settings zoom step. Stops at the first / last rung. */
export function stepZoom(current: ZoomLevel, direction: 1 | -1): ZoomLevel {
  const index = ZOOM_LEVELS.indexOf(current)
  const from = index >= 0 ? index : ZOOM_LEVELS.indexOf(DEFAULT_ZOOM_LEVEL)
  const next = Math.min(ZOOM_LEVELS.length - 1, Math.max(0, from + direction))
  return ZOOM_LEVELS[next]
}
