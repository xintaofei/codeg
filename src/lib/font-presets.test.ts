import { describe, expect, it } from "vitest"

import {
  CUSTOM_FONT_ID,
  DEFAULT_EDITOR_FONT_ID,
  DEFAULT_EDITOR_FONT_SIZE,
  DEFAULT_TERMINAL_FONT_ID,
  DEFAULT_TERMINAL_FONT_SIZE,
  DEFAULT_UI_FONT_ID,
  FONTS,
  FONT_BY_ID,
  FONT_SIZES,
  MONO_FALLBACK,
  MONO_FONTS,
  SANS_FALLBACK,
  UI_FONTS,
  fontSupportsLigatures,
  isValidFontId,
  isValidFontSize,
  resolveFontStack,
  sanitizeFontFamily,
} from "./font-presets"

describe("font catalog integrity", () => {
  it("has unique ids and non-empty stacks", () => {
    const ids = FONTS.map((f) => f.id)
    expect(new Set(ids).size).toBe(ids.length)
    for (const f of FONTS) {
      expect(f.stack.trim().length).toBeGreaterThan(0)
      expect(["sans", "mono"]).toContain(f.category)
      expect(["system", "bundled"]).toContain(f.source)
    }
  })

  it("does not contain the custom sentinel as a real entry", () => {
    expect(FONT_BY_ID[CUSTOM_FONT_ID]).toBeUndefined()
  })

  it("FONT_BY_ID maps every font", () => {
    for (const f of FONTS) expect(FONT_BY_ID[f.id]).toBe(f)
  })

  it("UI_FONTS exposes every font; MONO_FONTS only monospace", () => {
    expect(UI_FONTS).toEqual(FONTS)
    expect(MONO_FONTS.length).toBeGreaterThan(0)
    for (const f of MONO_FONTS) expect(f.category).toBe("mono")
  })

  it("bundled stacks end with the matching fallback", () => {
    for (const f of FONTS.filter((f) => f.source === "bundled")) {
      const fallback = f.category === "mono" ? MONO_FALLBACK : SANS_FALLBACK
      expect(f.stack.endsWith(fallback)).toBe(true)
    }
  })
})

describe("defaults", () => {
  it("all default font ids resolve to catalog entries", () => {
    for (const id of [
      DEFAULT_UI_FONT_ID,
      DEFAULT_EDITOR_FONT_ID,
      DEFAULT_TERMINAL_FONT_ID,
    ]) {
      expect(FONT_BY_ID[id]).toBeDefined()
    }
  })

  it("default UI font is Inter (bundled sans-serif)", () => {
    expect(DEFAULT_UI_FONT_ID).toBe("inter")
    expect(FONT_BY_ID[DEFAULT_UI_FONT_ID].category).toBe("sans")
    expect(FONT_BY_ID[DEFAULT_UI_FONT_ID].source).toBe("bundled")
  })

  it("default editor/terminal fonts are monospace", () => {
    expect(FONT_BY_ID[DEFAULT_EDITOR_FONT_ID].category).toBe("mono")
    expect(FONT_BY_ID[DEFAULT_TERMINAL_FONT_ID].category).toBe("mono")
  })

  it("default sizes are valid", () => {
    expect(isValidFontSize(DEFAULT_EDITOR_FONT_SIZE)).toBe(true)
    expect(isValidFontSize(DEFAULT_TERMINAL_FONT_SIZE)).toBe(true)
  })
})

describe("isValidFontId", () => {
  it("accepts known ids and the custom sentinel, rejects junk", () => {
    expect(isValidFontId(DEFAULT_UI_FONT_ID)).toBe(true)
    expect(isValidFontId(CUSTOM_FONT_ID)).toBe(true)
    expect(isValidFontId("nope")).toBe(false)
    expect(isValidFontId(null)).toBe(false)
    expect(isValidFontId(undefined)).toBe(false)
  })
})

describe("isValidFontSize", () => {
  it("only accepts listed sizes", () => {
    expect(isValidFontSize(FONT_SIZES[0])).toBe(true)
    expect(isValidFontSize(13)).toBe(true)
    expect(isValidFontSize(7)).toBe(false)
    expect(isValidFontSize(99)).toBe(false)
  })
})

describe("sanitizeFontFamily", () => {
  it("strips CSS-breaking characters and trims/caps length", () => {
    expect(sanitizeFontFamily("Fira Code; }")).toBe("Fira Code")
    expect(sanitizeFontFamily("  Inter  ")).toBe("Inter")
    expect(sanitizeFontFamily('"<script>"')).toBe("script")
    expect(sanitizeFontFamily("a".repeat(200)).length).toBe(64)
  })

  it("strips backslashes and control characters (newline/tab/bell)", () => {
    expect(sanitizeFontFamily("Fira\\Code\n\t")).toBe("FiraCode")
    expect(sanitizeFontFamily(`Mono${String.fromCharCode(7)}X`)).toBe("MonoX")
  })
})

describe("resolveFontStack", () => {
  it("returns the catalog stack for a known id", () => {
    const jb = FONT_BY_ID["jetbrains-mono"]
    expect(resolveFontStack("jetbrains-mono", "", "mono")).toBe(jb.stack)
  })

  it("wraps a custom family and appends the category fallback", () => {
    expect(resolveFontStack(CUSTOM_FONT_ID, "Comic Mono", "mono")).toBe(
      `"Comic Mono", ${MONO_FALLBACK}`
    )
    expect(resolveFontStack(CUSTOM_FONT_ID, "My Sans", "sans")).toBe(
      `"My Sans", ${SANS_FALLBACK}`
    )
  })

  it("falls back to the category stack for empty custom or unknown id", () => {
    expect(resolveFontStack(CUSTOM_FONT_ID, "   ", "mono")).toBe(MONO_FALLBACK)
    expect(resolveFontStack("does-not-exist", "", "sans")).toBe(SANS_FALLBACK)
  })

  it("never emits CSS-breaking characters from custom input", () => {
    const stack = resolveFontStack(CUSTOM_FONT_ID, "evil;\\ }<x>", "mono")
    expect(stack).not.toMatch(/[;{}<>\\]/)
  })
})

describe("fontSupportsLigatures", () => {
  it("reflects catalog flags and treats custom as supported", () => {
    expect(fontSupportsLigatures("jetbrains-mono")).toBe(true)
    expect(fontSupportsLigatures("system-mono")).toBe(false)
    expect(fontSupportsLigatures(CUSTOM_FONT_ID)).toBe(true)
    expect(fontSupportsLigatures("unknown")).toBe(false)
  })
})
