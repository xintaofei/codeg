import type { ThemeColor } from "./appearance-settings"

export interface ThemeColorPreset {
  name: ThemeColor
  /** Hex color for the preview circle in settings UI */
  preview: string
  /** CSS variable overrides for light mode (empty = use globals.css defaults) */
  light: Record<string, string>
  /** CSS variable overrides for dark mode */
  dark: Record<string, string>
}

export const THEME_COLOR_PRESETS: ThemeColorPreset[] = [
  {
    name: "zinc",
    preview: "#a1a1aa",
    light: {},
    dark: {},
  },
  {
    name: "slate",
    preview: "#64748b",
    light: {
      "--primary": "oklch(0.279 0.041 260)",
      "--primary-foreground": "oklch(0.985 0.002 247)",
      "--ring": "oklch(0.551 0.027 264)",
      "--sidebar-primary": "oklch(0.279 0.041 260)",
      "--sidebar-primary-foreground": "oklch(0.985 0.002 247)",
    },
    dark: {
      "--primary": "oklch(0.929 0.013 255)",
      "--primary-foreground": "oklch(0.208 0.042 265)",
      "--ring": "oklch(0.551 0.027 264)",
      "--sidebar-primary": "oklch(0.551 0.027 264)",
      "--sidebar-primary-foreground": "oklch(0.985 0.002 247)",
    },
  },
  {
    name: "blue",
    preview: "#3b82f6",
    light: {
      "--primary": "oklch(0.546 0.245 262.881)",
      "--primary-foreground": "oklch(0.985 0.002 247)",
      "--ring": "oklch(0.623 0.214 259.815)",
      "--sidebar-primary": "oklch(0.546 0.245 262.881)",
      "--sidebar-primary-foreground": "oklch(0.985 0.002 247)",
    },
    dark: {
      "--primary": "oklch(0.623 0.214 259.815)",
      "--primary-foreground": "oklch(0.985 0.002 247)",
      "--ring": "oklch(0.546 0.245 262.881)",
      "--sidebar-primary": "oklch(0.623 0.214 259.815)",
      "--sidebar-primary-foreground": "oklch(0.985 0.002 247)",
    },
  },
  {
    name: "green",
    preview: "#22c55e",
    light: {
      "--primary": "oklch(0.586 0.209 145)",
      "--primary-foreground": "oklch(0.985 0.014 140)",
      "--ring": "oklch(0.648 0.2 145)",
      "--sidebar-primary": "oklch(0.586 0.209 145)",
      "--sidebar-primary-foreground": "oklch(0.985 0.014 140)",
    },
    dark: {
      "--primary": "oklch(0.648 0.2 145)",
      "--primary-foreground": "oklch(0.21 0.065 145)",
      "--ring": "oklch(0.586 0.209 145)",
      "--sidebar-primary": "oklch(0.648 0.2 145)",
      "--sidebar-primary-foreground": "oklch(0.985 0.014 140)",
    },
  },
  {
    name: "violet",
    preview: "#8b5cf6",
    light: {
      "--primary": "oklch(0.541 0.281 293)",
      "--primary-foreground": "oklch(0.969 0.016 293)",
      "--ring": "oklch(0.606 0.25 292)",
      "--sidebar-primary": "oklch(0.541 0.281 293)",
      "--sidebar-primary-foreground": "oklch(0.969 0.016 293)",
    },
    dark: {
      "--primary": "oklch(0.606 0.25 292)",
      "--primary-foreground": "oklch(0.969 0.016 293)",
      "--ring": "oklch(0.541 0.281 293)",
      "--sidebar-primary": "oklch(0.606 0.25 292)",
      "--sidebar-primary-foreground": "oklch(0.969 0.016 293)",
    },
  },
  {
    name: "orange",
    preview: "#f97316",
    light: {
      "--primary": "oklch(0.705 0.191 47)",
      "--primary-foreground": "oklch(0.985 0.016 73)",
      "--ring": "oklch(0.752 0.183 55)",
      "--sidebar-primary": "oklch(0.705 0.191 47)",
      "--sidebar-primary-foreground": "oklch(0.985 0.016 73)",
    },
    dark: {
      "--primary": "oklch(0.752 0.183 55)",
      "--primary-foreground": "oklch(0.255 0.072 45)",
      "--ring": "oklch(0.705 0.191 47)",
      "--sidebar-primary": "oklch(0.752 0.183 55)",
      "--sidebar-primary-foreground": "oklch(0.985 0.016 73)",
    },
  },
  {
    name: "rose",
    preview: "#f43f5e",
    light: {
      "--primary": "oklch(0.585 0.22 17)",
      "--primary-foreground": "oklch(0.969 0.016 17)",
      "--ring": "oklch(0.645 0.246 16)",
      "--sidebar-primary": "oklch(0.585 0.22 17)",
      "--sidebar-primary-foreground": "oklch(0.969 0.016 17)",
    },
    dark: {
      "--primary": "oklch(0.645 0.246 16)",
      "--primary-foreground": "oklch(0.969 0.016 17)",
      "--ring": "oklch(0.585 0.22 17)",
      "--sidebar-primary": "oklch(0.645 0.246 16)",
      "--sidebar-primary-foreground": "oklch(0.969 0.016 17)",
    },
  },
]

export function getThemeColorPreset(
  name: ThemeColor
): ThemeColorPreset | undefined {
  return THEME_COLOR_PRESETS.find((p) => p.name === name)
}
