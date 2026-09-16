import { describe, expect, it } from "vitest"
import { bundledThemesInfo } from "shiki"

import {
  CODE_THEMES,
  DEFAULT_CODE_THEME,
  codeThemeLabel,
  codeThemeTuple,
  isCodeThemeId,
  parseStoredCodeTheme,
  sanitizeCodeTheme,
} from "./code-themes"

describe("CODE_THEMES", () => {
  it("is exactly Shiki's bundled theme list, with the same light/dark type", () => {
    // The allowlist is hand-written so the settings page and the preset
    // validator never import Shiki's engine. Pinning it to what the installed
    // Shiki actually bundles means a version bump that renames or drops a
    // theme fails here instead of leaving a preset pointing at nothing.
    const ours = CODE_THEMES.map((t) => `${t.id}:${t.type}`).sort()
    const shiki = bundledThemesInfo.map((t) => `${t.id}:${t.type}`).sort()
    expect(ours).toEqual(shiki)
  })

  it("has unique ids and includes the defaults", () => {
    expect(new Set(CODE_THEMES.map((t) => t.id)).size).toBe(CODE_THEMES.length)
    expect(isCodeThemeId(DEFAULT_CODE_THEME.light)).toBe(true)
    expect(isCodeThemeId(DEFAULT_CODE_THEME.dark)).toBe(true)
    expect(isCodeThemeId("github-darker-than-dark")).toBe(false)
    expect(isCodeThemeId(42)).toBe(false)
  })
})

describe("sanitizeCodeTheme / parseStoredCodeTheme", () => {
  it("keeps known halves and falls back per half", () => {
    expect(
      sanitizeCodeTheme({ light: "min-light", dark: "not-a-theme" })
    ).toEqual({ light: "min-light", dark: DEFAULT_CODE_THEME.dark })
    expect(parseStoredCodeTheme('{"light":"one-light","dark":"nord"}')).toEqual(
      {
        light: "one-light",
        dark: "nord",
      }
    )
  })

  it("never throws on garbage", () => {
    expect(parseStoredCodeTheme("{oops")).toEqual(DEFAULT_CODE_THEME)
    expect(parseStoredCodeTheme(null)).toEqual(DEFAULT_CODE_THEME)
    expect(sanitizeCodeTheme("nord")).toEqual(DEFAULT_CODE_THEME)
  })

  it("orders the tuple light-first for Streamdown", () => {
    expect(codeThemeTuple({ light: "one-light", dark: "nord" })).toEqual([
      "one-light",
      "nord",
    ])
  })
})

describe("codeThemeLabel", () => {
  it("restores casing without translating proper nouns", () => {
    expect(codeThemeLabel("github-dark-high-contrast")).toBe(
      "GitHub Dark High Contrast"
    )
    expect(codeThemeLabel("catppuccin-mocha")).toBe("Catppuccin Mocha")
    expect(codeThemeLabel("synthwave-84")).toBe("Synthwave 84")
  })
})
