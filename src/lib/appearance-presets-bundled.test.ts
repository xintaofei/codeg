import { readFileSync } from "node:fs"
import { resolve as resolvePath } from "node:path"
import { describe, expect, it } from "vitest"

import {
  presetMatches,
  presetToApplication,
  snapshotToPreset,
  validateAppearancePreset,
  type AppearancePreset,
} from "./appearance-preset"
import {
  BUNDLED_PRESETS,
  BUNDLED_PRESET_IDS,
  DEFAULT_PRESET_ID,
  isBundledPresetId,
} from "./appearance-presets-bundled"
import {
  contrastRatio,
  resolveCssColor,
  type LinearRgba,
} from "./color-contrast"
import { MONO_FALLBACK, resolveFontStack } from "./font-presets"
import { DEFAULT_CHAT_FONT_SIZE_REM } from "./appearance-preset"
import { THEME_COLORS, type ThemeColor } from "./theme-presets"

// ─── globals.css as data ───
//
// The base palettes and the surface defaults live in CSS, and only there. The
// checks below read the stylesheet rather than a copy, so a palette edit in
// globals.css is what the contrast check measures.

// vitest runs from the repo root (`pnpm test`); under jsdom `import.meta.url`
// is an http URL, so resolve from the working directory instead.
const globalsCss = readFileSync(
  resolvePath(process.cwd(), "src/app/globals.css"),
  "utf8"
).replace(/\/\*[\s\S]*?\*\//g, "")

type Declarations = Record<string, string>

function parseDeclarations(block: string): Declarations {
  const out: Declarations = {}
  for (const line of block.split(";")) {
    const idx = line.indexOf(":")
    if (idx < 0) continue
    const name = line.slice(0, idx).trim()
    if (!name.startsWith("--")) continue
    out[name.slice(2)] = line
      .slice(idx + 1)
      .replace(/\s+/g, " ")
      .trim()
  }
  return out
}

function paletteBlock(color: ThemeColor, dark: boolean): Declarations {
  const selector = `[data-theme="${color}"]${dark ? ".dark" : ""}`
  const pattern = new RegExp(
    `${selector.replace(/[[\]().*+?"]/g, (c) => `\\${c}`)}\\s*\\{([^}]*)\\}`
  )
  const match = pattern.exec(globalsCss)
  if (!match) throw new Error(`no ${selector} block in globals.css`)
  return parseDeclarations(match[1])
}

const rootDefaults = parseDeclarations(
  /:root\s*\{([^}]*)\}/.exec(globalsCss)![1]
)

const PALETTES = Object.fromEntries(
  THEME_COLORS.map((color) => [
    color,
    { light: paletteBlock(color, false), dark: paletteBlock(color, true) },
  ])
) as Record<ThemeColor, { light: Declarations; dark: Declarations }>

// ─── Contrast ───

/** Text on the surface it is written on: WCAG AA for normal text. */
const TEXT_PAIRS: Array<[string, string]> = [
  ["foreground", "background"],
  ["card-foreground", "card"],
  ["popover-foreground", "popover"],
  ["primary-foreground", "primary"],
  ["secondary-foreground", "secondary"],
  ["accent-foreground", "accent"],
  ["sidebar-foreground", "sidebar"],
  ["sidebar-accent-foreground", "sidebar-accent"],
  ["sidebar-primary-foreground", "sidebar-primary"],
  ["bubble-user-fg", "bubble-user-bg"],
  ["bubble-assistant-fg", "bubble-assistant-bg"],
  ["muted-foreground", "background"],
]

/**
 * De-emphasised text (`muted-foreground` on a muted surface, the status bar):
 * held to 3:1, the AA floor for large text and UI components. shadcn's own
 * neutral palette sits at 4.4:1 for muted-on-muted, so 4.5 here would fail
 * the stock look; 3:1 is the honest bar for text that is dimmed on purpose.
 */
const MUTED_PAIRS: Array<[string, string]> = [
  ["muted-foreground", "muted"],
  ["status-bar-fg", "status-bar-bg"],
]

const AA_TEXT = 4.5
const AA_MUTED = 3

type ContrastFailure = { mode: "light" | "dark"; pair: string; ratio: number }

function resolver(preset: AppearancePreset, mode: "light" | "dark") {
  const overrides = presetToApplication(preset).customTheme[mode]
  const palette = PALETTES[preset.base][mode]
  return (name: string): string | undefined =>
    overrides[name as keyof typeof overrides] ??
    palette[name] ??
    rootDefaults[name]
}

function resolve(
  lookup: (name: string) => string | undefined,
  token: string
): LinearRgba {
  const color = resolveCssColor(`var(--${token})`, lookup)
  if (!color) throw new Error(`cannot resolve --${token} to a literal colour`)
  return color
}

function contrastFailures(preset: AppearancePreset): ContrastFailure[] {
  const failures: ContrastFailure[] = []
  for (const mode of ["light", "dark"] as const) {
    const lookup = resolver(preset, mode)
    const background = resolve(lookup, "background")
    const check = (pairs: Array<[string, string]>, floor: number) => {
      for (const [fg, bg] of pairs) {
        const ratio = contrastRatio(
          resolve(lookup, fg),
          resolve(lookup, bg),
          background
        )
        if (ratio < floor) {
          failures.push({ mode, pair: `${fg} on ${bg}`, ratio })
        }
      }
    }
    check(TEXT_PAIRS, AA_TEXT)
    check(MUTED_PAIRS, AA_MUTED)
  }
  return failures
}

describe("bundled presets", () => {
  it("are valid documents with unique ids, and the default is one of them", () => {
    const ids = BUNDLED_PRESETS.map((p) => p.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids).toEqual([...BUNDLED_PRESET_IDS])
    expect(ids).toContain(DEFAULT_PRESET_ID)
    expect(isBundledPresetId("warm-terminal")).toBe(true)
    expect(isBundledPresetId("stranger")).toBe(false)
    for (const preset of BUNDLED_PRESETS) {
      const result = validateAppearancePreset(preset)
      expect(result.ok, preset.id).toBe(true)
      if (result.ok) {
        expect(result.preset, preset.id).toEqual(preset)
        expect(result.warnings, preset.id).toEqual([])
      }
    }
  })

  it("keep the default preset as the stock look", () => {
    const stock = BUNDLED_PRESETS.find((p) => p.id === DEFAULT_PRESET_ID)!
    expect(stock.base).toBe("neutral")
    expect(stock.colors).toEqual({})
    expect(stock.shape).toBeUndefined()
    expect(stock.density).toBeUndefined()
    expect(stock.typography).toBeUndefined()
    expect(stock.code).toEqual({ light: "github-light", dark: "github-dark" })
  })

  it.each(BUNDLED_PRESETS.map((p) => [p.id, p] as const))(
    "%s passes WCAG AA for text on every surface, in both modes",
    (_id, preset) => {
      expect(contrastFailures(preset)).toEqual([])
    }
  )

  it("the contrast check itself fails on an unreadable palette", () => {
    // Guard against a vacuous check: the same code must reject a preset whose
    // text is its background.
    const unreadable: AppearancePreset = {
      schemaVersion: 1,
      id: "unreadable",
      name: "Unreadable",
      base: "neutral",
      colors: {
        light: { foreground: "oklch(0.96 0 0)" },
        dark: { "bubble-user-fg": "oklch(0.2 0 0)" },
      },
    }
    const failures = contrastFailures(unreadable)
    // The light foreground also feeds both bubbles through the surface
    // defaults (`--bubble-*-fg: var(--foreground)`), so it fails three pairs.
    expect(failures.map((f) => `${f.mode}:${f.pair}`)).toEqual([
      "light:foreground on background",
      "light:bubble-user-fg on bubble-user-bg",
      "light:bubble-assistant-fg on bubble-assistant-bg",
      "dark:bubble-user-fg on bubble-user-bg",
    ])
    for (const failure of failures) expect(failure.ratio).toBeLessThan(4.5)
  })

  it("match the state they produce and survive an export round trip", () => {
    for (const preset of BUNDLED_PRESETS) {
      const app = presetToApplication(preset)
      const current = {
        themeColor: app.themeColor,
        customTheme: app.customTheme,
        uiFontId: app.uiFont ?? "inter",
        monoFontId: app.monoFont ?? "system-mono",
        codeTheme: app.codeTheme ?? {
          light: "github-light",
          dark: "github-dark",
        },
      }
      expect(presetMatches(preset, current), preset.id).toBe(true)
      // Every other bundled preset must NOT match this state, or the gallery
      // would light two cards at once.
      for (const other of BUNDLED_PRESETS) {
        if (other.id === preset.id) continue
        expect(
          presetMatches(other, current),
          `${other.id} vs ${preset.id}`
        ).toBe(false)
      }
      // A fully specified preset exports back to itself.
      if (preset.typography?.ui && preset.typography?.mono && preset.code) {
        const { preset: exported, warnings } = snapshotToPreset(
          { ...current, mode: app.mode ?? null },
          preset
        )
        expect(warnings, preset.id).toEqual([])
        expect(exported, preset.id).toEqual(preset)
      }
    }
  })
})

describe("globals.css defaults the presets rely on", () => {
  it("ship --font-mono as the same stack the system-mono font resolves to", () => {
    // With no stored choice the inline script writes nothing, so this default
    // is what code renders in. It must equal the "system-mono" catalog entry
    // (and Tailwind's own default stack) or an upgrade would change code
    // rendering for everyone.
    expect(rootDefaults["font-mono"]).toBe(MONO_FALLBACK)
    expect(resolveFontStack("system-mono", "", "mono")).toBe(MONO_FALLBACK)
  })

  it("ship message text at the stock text-sm size", () => {
    expect(rootDefaults["chat-font-size"]).toBe(
      `${DEFAULT_CHAT_FONT_SIZE_REM}rem`
    )
    expect(rootDefaults["chat-line-height"]).toBe("calc(1.25 / 0.875)")
  })

  it("point every surface token back at a shadcn token by default", () => {
    // A theme that never mentions the surface tokens must render exactly as
    // it did before they existed: the defaults are references, not colours.
    expect(rootDefaults["bubble-user-bg"]).toBe("var(--secondary)")
    expect(rootDefaults["bubble-user-fg"]).toBe("var(--foreground)")
    expect(rootDefaults["bubble-user-radius"]).toBe("var(--radius)")
    expect(rootDefaults["bubble-assistant-bg"]).toBe("transparent")
    expect(rootDefaults["bubble-assistant-radius"]).toBe("0px")
    expect(rootDefaults["bubble-assistant-padding"]).toBe("0px")
    expect(rootDefaults["composer-bg"]).toBe("transparent")
    expect(rootDefaults["composer-radius"]).toBe("calc(var(--radius) * 1.4)")
    expect(rootDefaults["status-bar-bg"]).toBe("var(--muted)")
    expect(rootDefaults["status-bar-fg"]).toBe("var(--muted-foreground)")
    expect(rootDefaults["status-bar-border"]).toBe("var(--border)")
  })
})
