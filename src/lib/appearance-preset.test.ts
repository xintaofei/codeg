import { describe, expect, it } from "vitest"

import {
  MAX_PRESET_BYTES,
  PRESET_SCHEMA_VERSION,
  findMatchingPreset,
  parseAppearancePreset,
  parseStoredAppearancePreset,
  presetFileName,
  presetMatches,
  presetToApplication,
  serializeAppearancePreset,
  snapshotToPreset,
  spacingMultiplierFromToken,
  spacingToken,
  validateAppearancePreset,
  type AppearancePreset,
  type PresetIssue,
} from "./appearance-preset"
import { CUSTOM_FONT_ID } from "./font-presets"

const valid: AppearancePreset = {
  schemaVersion: 1,
  id: "test-look",
  name: "Test look",
  description: "A preset used by the tests.",
  author: "tests",
  version: "1.0.0",
  base: "slate",
  mode: "dark",
  colors: {
    light: { primary: "#112233", "bubble-user-bg": "oklch(0.93 0.02 250)" },
    dark: { primary: "oklch(0.7 0.15 250)", "status-bar-bg": "transparent" },
  },
  shape: {
    radius: "0.25rem",
    userBubbleRadius: "4px",
    composerRadius: "0.5rem",
  },
  density: { spacing: 0.85 },
  typography: {
    ui: "geist",
    mono: "jetbrains-mono",
    chatFontSize: 0.8125,
    chatLineHeight: 1.4,
  },
  code: { light: "light-plus", dark: "dark-plus" },
}

function issuesOf(input: unknown): PresetIssue[] {
  const result = validateAppearancePreset(input)
  return result.ok ? [] : result.issues
}

function codesOf(input: unknown): string[] {
  return issuesOf(input).map((i) => `${i.path}:${i.code}`)
}

/** `valid` with one top-level field removed. */
function without(key: keyof AppearancePreset): Record<string, unknown> {
  const copy: Record<string, unknown> = { ...valid }
  delete copy[key]
  return copy
}

describe("validateAppearancePreset", () => {
  it("accepts a complete document and returns it normalized", () => {
    const result = validateAppearancePreset(valid)
    expect(result.ok).toBe(true)
    if (!result.ok) return
    expect(result.preset).toEqual(valid)
    expect(result.warnings).toEqual([])
  })

  it("accepts the minimal document", () => {
    const result = validateAppearancePreset({
      schemaVersion: 1,
      id: "min",
      name: "Min",
      base: "neutral",
      colors: {},
    })
    expect(result.ok).toBe(true)
  })

  it("names an unknown key by its path instead of dropping it", () => {
    // A typo must not import as "worked".
    expect(codesOf({ ...valid, colour: {} })).toEqual(["colour:unknown-key"])
    expect(codesOf({ ...valid, colors: { dark: { primry: "#000" } } })).toEqual(
      ["colors.dark.primry:unknown-token"]
    )
    expect(
      codesOf({ ...valid, shape: { radius: "1rem", corner: "1rem" } })
    ).toEqual(["shape.corner:unknown-key"])
    expect(codesOf({ ...valid, colors: { light: {}, night: {} } })).toEqual([
      "colors.night:unknown-key",
    ])
  })

  it("rejects bad colours with the exact field", () => {
    expect(
      codesOf({ ...valid, colors: { light: { primary: "red; } body {" } } })
    ).toEqual(["colors.light.primary:invalid-color"])
    expect(
      codesOf({ ...valid, colors: { light: { primary: "url(https://x/y)" } } })
    ).toEqual(["colors.light.primary:invalid-color"])
    expect(
      codesOf({ ...valid, colors: { light: { primary: "plum" } } })
    ).toEqual(["colors.light.primary:invalid-color"])
    expect(
      codesOf({ ...valid, colors: { light: { primary: "12px" } } })
    ).toEqual(["colors.light.primary:invalid-color"])
    expect(codesOf({ ...valid, colors: { light: { primary: 7 } } })).toEqual([
      "colors.light.primary:wrong-type",
    ])
    // `radius` inside colors is a length (shadcn cssVars carry it there).
    expect(
      codesOf({ ...valid, colors: { light: { radius: "#fff" } } })
    ).toEqual(["colors.light.radius:invalid-length"])
    expect(
      validateAppearancePreset({
        ...valid,
        colors: { light: { radius: "0.5rem" } },
      }).ok
    ).toBe(true)
  })

  it("range-checks lengths and numbers", () => {
    expect(codesOf({ ...valid, shape: { radius: "9rem" } })).toEqual([
      "shape.radius:out-of-range",
    ])
    expect(codesOf({ ...valid, shape: { radius: "10vw" } })).toEqual([
      "shape.radius:invalid-length",
    ])
    expect(codesOf({ ...valid, density: { spacing: 3 } })).toEqual([
      "density.spacing:out-of-range",
    ])
    expect(codesOf({ ...valid, density: { spacing: "1" } })).toEqual([
      "density.spacing:wrong-type",
    ])
    expect(codesOf({ ...valid, typography: { chatFontSize: 5 } })).toEqual([
      "typography.chatFontSize:out-of-range",
    ])
    expect(codesOf({ ...valid, typography: { chatLineHeight: 0.5 } })).toEqual([
      "typography.chatLineHeight:out-of-range",
    ])
  })

  it("only allows fonts and code themes the app ships", () => {
    expect(codesOf({ ...valid, typography: { ui: "comic-sans" } })).toEqual([
      "typography.ui:unknown-font",
    ])
    // A custom family is a local choice, never shareable.
    expect(codesOf({ ...valid, typography: { ui: CUSTOM_FONT_ID } })).toEqual([
      "typography.ui:unknown-font",
    ])
    // The code font must be monospace.
    expect(codesOf({ ...valid, typography: { mono: "inter" } })).toEqual([
      "typography.mono:unknown-font",
    ])
    expect(
      codesOf({ ...valid, code: { light: "nope", dark: "nord" } })
    ).toEqual(["code.light:unknown-code-theme"])
    expect(codesOf({ ...valid, code: { light: "one-light" } })).toEqual([
      "code.dark:missing",
    ])
  })

  it("checks identity fields", () => {
    expect(codesOf({ ...valid, id: "Not A Slug" })).toEqual([
      "id:invalid-value",
    ])
    expect(codesOf({ ...valid, id: "" })).toEqual(["id:invalid-value"])
    expect(codesOf({ ...valid, name: "   " })).toEqual(["name:invalid-value"])
    expect(codesOf({ ...valid, name: "x".repeat(81) })).toEqual([
      "name:out-of-range",
    ])
    expect(codesOf({ ...valid, base: "teal" })).toEqual(["base:unknown-base"])
    expect(codesOf({ ...valid, mode: "auto" })).toEqual(["mode:invalid-value"])
    expect(codesOf(without("base"))).toEqual(["base:missing"])
    expect(codesOf(without("colors"))).toEqual(["colors:missing"])
  })

  it("reports every problem at once, not just the first", () => {
    const codes = codesOf({
      ...valid,
      base: "teal",
      colors: { light: { primary: "plum" } },
      density: { spacing: 9 },
    })
    expect(codes).toHaveLength(3)
    expect(codes).toContain("base:unknown-base")
    expect(codes).toContain("colors.light.primary:invalid-color")
    expect(codes).toContain("density.spacing:out-of-range")
  })

  it("refuses non-objects", () => {
    expect(codesOf(null)).toEqual([":wrong-type"])
    expect(codesOf([])).toEqual([":not-object"])
    expect(codesOf("nope")).toEqual([":not-object"])
  })
})

describe("schema versions", () => {
  it("requires schemaVersion and refuses documents from a newer format", () => {
    expect(codesOf(without("schemaVersion"))).toEqual(["schemaVersion:missing"])
    expect(codesOf({ ...valid, schemaVersion: "1" })).toEqual([
      "schemaVersion:wrong-type",
    ])
    expect(
      codesOf({ ...valid, schemaVersion: PRESET_SCHEMA_VERSION + 1 })
    ).toEqual(["schemaVersion:unsupported-version"])
    expect(codesOf({ ...valid, schemaVersion: 0 })).toEqual([
      "schemaVersion:unsupported-version",
    ])
  })

  it("walks the migration chain up to the current version", () => {
    // Version 1 is the first version, so the chain is the identity. When a
    // version 2 lands, this is where its migration gets exercised.
    const result = validateAppearancePreset({ ...valid, schemaVersion: 1 })
    expect(result.ok && result.preset.schemaVersion).toBe(PRESET_SCHEMA_VERSION)
  })
})

describe("parseAppearancePreset", () => {
  it("refuses an oversized document before parsing it", () => {
    const padded = JSON.stringify({
      ...valid,
      description: "x".repeat(MAX_PRESET_BYTES),
    })
    const result = parseAppearancePreset(padded)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.issues.map((i) => i.code)).toEqual(["too-large"])
  })

  it("reports invalid JSON", () => {
    const result = parseAppearancePreset("{not json")
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.issues[0].code).toBe("invalid-json")
  })

  it("round-trips through serialize", () => {
    const text = serializeAppearancePreset(valid)
    expect(text.endsWith("\n")).toBe(true)
    const result = parseAppearancePreset(text)
    expect(result.ok && result.preset).toEqual(valid)
    expect(parseStoredAppearancePreset(text)).toEqual(valid)
    expect(parseStoredAppearancePreset("{broken")).toBeNull()
    expect(parseStoredAppearancePreset(null)).toBeNull()
  })

  it("names the file after the id", () => {
    expect(presetFileName(valid)).toBe("test-look.codeg-preset.json")
  })
})

describe("presetToApplication", () => {
  it("fans mode-independent settings out to both modes as tokens", () => {
    const app = presetToApplication(valid)
    expect(app.themeColor).toBe("slate")
    expect(app.mode).toBe("dark")
    expect(app.uiFont).toBe("geist")
    expect(app.monoFont).toBe("jetbrains-mono")
    expect(app.codeTheme).toEqual({ light: "light-plus", dark: "dark-plus" })
    for (const mode of ["light", "dark"] as const) {
      const tokens = app.customTheme[mode]
      expect(tokens.radius).toBe("0.25rem")
      expect(tokens["bubble-user-radius"]).toBe("4px")
      expect(tokens["composer-radius"]).toBe("0.5rem")
      expect(tokens.spacing).toBe(spacingToken(0.85))
      expect(tokens["chat-font-size"]).toBe("0.8125rem")
      expect(tokens["chat-line-height"]).toBe("1.4")
    }
    expect(app.customTheme.light.primary).toBe("#112233")
    expect(app.customTheme.dark.primary).toBe("oklch(0.7 0.15 250)")
    expect(app.customTheme.light["status-bar-bg"]).toBeUndefined()
  })

  it("leaves unspecified settings alone", () => {
    const app = presetToApplication({
      schemaVersion: 1,
      id: "bare",
      name: "Bare",
      base: "neutral",
      colors: {},
    })
    expect(app).toEqual({
      themeColor: "neutral",
      customTheme: { light: {}, dark: {} },
    })
  })
})

describe("spacing tokens", () => {
  it("maps the multiplier to a rem value and back", () => {
    expect(spacingToken(1)).toBe("0.25rem")
    expect(spacingToken(0.85)).toBe("0.2125rem")
    expect(spacingMultiplierFromToken("0.2125rem")).toBe(0.85)
    expect(spacingMultiplierFromToken("4px")).toBeNull()
    expect(spacingMultiplierFromToken(undefined)).toBeNull()
  })
})

describe("snapshotToPreset", () => {
  it("is the inverse of presetToApplication for a complete preset", () => {
    const app = presetToApplication(valid)
    const { preset, warnings } = snapshotToPreset(
      {
        themeColor: app.themeColor,
        customTheme: app.customTheme,
        uiFontId: app.uiFont!,
        monoFontId: app.monoFont!,
        codeTheme: app.codeTheme!,
        mode: app.mode ?? null,
      },
      {
        id: valid.id,
        name: valid.name,
        description: valid.description,
        author: valid.author,
        version: valid.version,
      }
    )
    expect(warnings).toEqual([])
    expect(preset).toEqual(valid)
    // And the export validates as a document in its own right.
    expect(validateAppearancePreset(preset).ok).toBe(true)
  })

  it("leaves out what cannot travel and says so", () => {
    const { preset, warnings } = snapshotToPreset(
      {
        themeColor: "neutral",
        customTheme: {
          light: { spacing: "13px", "chat-font-size": "0.875rem" },
          dark: { spacing: "13px", "chat-font-size": "0.875rem" },
        },
        uiFontId: CUSTOM_FONT_ID,
        monoFontId: "system-mono",
        codeTheme: { light: "github-light", dark: "github-dark" },
        mode: null,
      },
      { id: "custom", name: "Custom" }
    )
    expect(preset.density).toBeUndefined()
    expect(preset.typography).toEqual({
      mono: "system-mono",
      chatFontSize: 0.875,
    })
    expect(preset.mode).toBeUndefined()
    expect(warnings.map((w) => `${w.path}:${w.code}`)).toEqual([
      "density.spacing:out-of-range",
      "typography.ui:unknown-font",
    ])
    expect(validateAppearancePreset(preset).ok).toBe(true)
  })
})

describe("presetMatches / findMatchingPreset", () => {
  const app = presetToApplication(valid)
  const current = {
    themeColor: app.themeColor,
    customTheme: app.customTheme,
    uiFontId: "geist",
    monoFontId: "jetbrains-mono",
    codeTheme: { light: "light-plus", dark: "dark-plus" } as const,
  }

  it("matches the state a preset produces, and only that", () => {
    expect(presetMatches(valid, current)).toBe(true)
    expect(
      presetMatches(valid, {
        ...current,
        customTheme: {
          ...current.customTheme,
          light: { ...current.customTheme.light, primary: "#000000" },
        },
      })
    ).toBe(false)
    expect(presetMatches(valid, { ...current, themeColor: "zinc" })).toBe(false)
    expect(presetMatches(valid, { ...current, uiFontId: "inter" })).toBe(false)
    expect(
      presetMatches(valid, {
        ...current,
        codeTheme: { light: "one-light", dark: "dark-plus" },
      })
    ).toBe(false)
  })

  it("ignores settings the preset does not specify", () => {
    const bare: AppearancePreset = {
      schemaVersion: 1,
      id: "bare",
      name: "Bare",
      base: "neutral",
      colors: {},
    }
    const stock = {
      themeColor: "neutral" as const,
      customTheme: { light: {}, dark: {} },
      uiFontId: "inter",
      monoFontId: "fira-code",
      codeTheme: { light: "nord", dark: "nord" } as const,
    }
    expect(presetMatches(bare, stock)).toBe(true)
    expect(findMatchingPreset([valid, bare], stock)).toBe(bare)
    expect(findMatchingPreset([valid, bare], current)).toBe(valid)
    expect(
      findMatchingPreset([valid, bare], { ...stock, themeColor: "rose" })
    ).toBeNull()
  })
})
