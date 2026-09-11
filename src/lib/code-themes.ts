// src/lib/code-themes.ts
//
// The syntax-colour themes a preset may name for code blocks in messages.
// Every entry is a theme Shiki already bundles (the highlighter loads it on
// demand), so picking one costs no download and a preset can never point at
// code that is not in the app. `code-themes.test.ts` pins this list to
// Shiki's `bundledThemesInfo` so an upgrade that adds or drops a theme fails
// loudly instead of leaving a stale id here.

import type { BundledTheme } from "shiki"

export type CodeThemeType = "light" | "dark"

export type CodeThemeDef = { id: BundledTheme; type: CodeThemeType }

const light = (id: BundledTheme): CodeThemeDef => ({ id, type: "light" })
const dark = (id: BundledTheme): CodeThemeDef => ({ id, type: "dark" })

export const CODE_THEMES: readonly CodeThemeDef[] = [
  dark("andromeeda"),
  dark("aurora-x"),
  dark("ayu-dark"),
  light("ayu-light"),
  dark("ayu-mirage"),
  dark("catppuccin-frappe"),
  light("catppuccin-latte"),
  dark("catppuccin-macchiato"),
  dark("catppuccin-mocha"),
  dark("dark-plus"),
  dark("dracula"),
  dark("dracula-soft"),
  dark("everforest-dark"),
  light("everforest-light"),
  dark("github-dark"),
  dark("github-dark-default"),
  dark("github-dark-dimmed"),
  dark("github-dark-high-contrast"),
  light("github-light"),
  light("github-light-default"),
  light("github-light-high-contrast"),
  dark("gruvbox-dark-hard"),
  dark("gruvbox-dark-medium"),
  dark("gruvbox-dark-soft"),
  light("gruvbox-light-hard"),
  light("gruvbox-light-medium"),
  light("gruvbox-light-soft"),
  dark("horizon"),
  dark("houston"),
  dark("kanagawa-dragon"),
  light("kanagawa-lotus"),
  dark("kanagawa-wave"),
  dark("laserwave"),
  light("light-plus"),
  dark("material-theme"),
  dark("material-theme-darker"),
  light("material-theme-lighter"),
  dark("material-theme-ocean"),
  dark("material-theme-palenight"),
  dark("min-dark"),
  light("min-light"),
  dark("monokai"),
  dark("night-owl"),
  light("night-owl-light"),
  dark("nord"),
  dark("one-dark-pro"),
  light("one-light"),
  dark("plastic"),
  dark("poimandres"),
  dark("red"),
  dark("rose-pine"),
  light("rose-pine-dawn"),
  dark("rose-pine-moon"),
  dark("slack-dark"),
  light("slack-ochin"),
  light("snazzy-light"),
  dark("solarized-dark"),
  light("solarized-light"),
  dark("synthwave-84"),
  dark("tokyo-night"),
  dark("vesper"),
  dark("vitesse-black"),
  dark("vitesse-dark"),
  light("vitesse-light"),
]

export const CODE_THEME_BY_ID: Record<string, CodeThemeDef> =
  Object.fromEntries(CODE_THEMES.map((t) => [t.id, t]))

/** One theme per mode; Streamdown switches between them on the `.dark` class. */
export type CodeThemePair = { light: BundledTheme; dark: BundledTheme }

/** Streamdown's own defaults, so an unset preference renders as it always has. */
export const DEFAULT_CODE_THEME: CodeThemePair = {
  light: "github-light",
  dark: "github-dark",
}

export function isCodeThemeId(id: unknown): id is BundledTheme {
  return typeof id === "string" && id in CODE_THEME_BY_ID
}

/**
 * Display name for a theme id. Theme names are proper nouns (Dracula, Nord,
 * Catppuccin Mocha), so they are not translated; this only fixes the casing
 * the ids lose.
 */
export function codeThemeLabel(id: string): string {
  return id
    .split("-")
    .map((word) => {
      if (word === "github") return "GitHub"
      if (/^\d/.test(word)) return word
      return word.charAt(0).toUpperCase() + word.slice(1)
    })
    .join(" ")
}

/** Storage / preset value -> a pair of known ids, unknown halves falling back. */
export function sanitizeCodeTheme(input: unknown): CodeThemePair {
  if (!input || typeof input !== "object") return { ...DEFAULT_CODE_THEME }
  const record = input as Record<string, unknown>
  return {
    light: isCodeThemeId(record.light)
      ? record.light
      : DEFAULT_CODE_THEME.light,
    dark: isCodeThemeId(record.dark) ? record.dark : DEFAULT_CODE_THEME.dark,
  }
}

export function parseStoredCodeTheme(raw: string | null): CodeThemePair {
  if (!raw) return { ...DEFAULT_CODE_THEME }
  try {
    return sanitizeCodeTheme(JSON.parse(raw))
  } catch {
    return { ...DEFAULT_CODE_THEME }
  }
}

/** The `[light, dark]` tuple Streamdown's `shikiTheme` prop expects. */
export function codeThemeTuple(
  pair: CodeThemePair
): [BundledTheme, BundledTheme] {
  return [pair.light, pair.dark]
}
