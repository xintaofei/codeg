import { describe, expect, it, vi } from "vitest"

import {
  configureLanguageValidation,
  defineMonacoThemes,
  defineWorkspaceBgTheme,
  EDITOR_CANVAS_BG,
  EDITOR_LINE_HIGHLIGHT,
  monacoThemeName,
  MONACO_UNICODE_HIGHLIGHT_OPTIONS,
} from "./monaco-themes"
import { THEME_COLORS } from "./theme-presets"

// Regression guard for issue #329: Monaco boxed ordinary CJK full-width
// punctuation (`：` `；` `，` `！` `？` `（` `）` …) because its unicode-highlight
// feature flags characters that look confusable with / are non-basic ASCII.
// Both mechanisms must stay disabled so Chinese/Japanese prose renders cleanly.
describe("MONACO_UNICODE_HIGHLIGHT_OPTIONS", () => {
  it("disables the mechanisms that box visible CJK punctuation", () => {
    expect(MONACO_UNICODE_HIGHLIGHT_OPTIONS.ambiguousCharacters).toBe(false)
    expect(MONACO_UNICODE_HIGHLIGHT_OPTIONS.nonBasicASCII).toBe(false)
  })

  it("keeps invisible-character highlighting at its default (still useful)", () => {
    // Intentionally untouched: surfacing zero-width / BOM characters never
    // boxes legible text and helps catch copy-paste gremlins.
    expect(MONACO_UNICODE_HIGHLIGHT_OPTIONS.invisibleCharacters).toBeUndefined()
  })
})

// The editor never loads a project's build context (tsconfig / node_modules /
// remote schemas), so Monaco's semantic + schema checks are always false
// positives here. This guards that we turn them off while keeping the
// context-free syntax checks that are still worth showing.
describe("configureLanguageValidation", () => {
  function makeMonaco() {
    const tsDefaults = {
      setCompilerOptions: vi.fn(),
      setDiagnosticsOptions: vi.fn(),
    }
    const jsDefaults = {
      setCompilerOptions: vi.fn(),
      setDiagnosticsOptions: vi.fn(),
    }
    const jsonDefaults = { setDiagnosticsOptions: vi.fn() }
    const monaco = {
      // Enough of the surface for both `configureLanguageValidation` and the
      // full `defineMonacoThemes` (which also registers the diff language, the
      // python tokenizer, and the themes) to run against the same mock.
      languages: {
        getLanguages: () => [] as { id: string }[],
        register: vi.fn(),
        setMonarchTokensProvider: vi.fn(),
        typescript: {
          JsxEmit: { Preserve: 1 },
          ScriptTarget: { ESNext: 99 },
          ModuleKind: { ESNext: 99 },
          ModuleResolutionKind: { NodeJs: 2 },
          typescriptDefaults: tsDefaults,
          javascriptDefaults: jsDefaults,
        },
        json: { jsonDefaults },
      },
      editor: { defineTheme: vi.fn() },
    }
    return { monaco, tsDefaults, jsDefaults, jsonDefaults }
  }

  it("disables TS/JS semantic + suggestion checks but keeps syntax errors", () => {
    const { monaco, tsDefaults, jsDefaults } = makeMonaco()

    configureLanguageValidation(
      monaco as unknown as Parameters<typeof configureLanguageValidation>[0]
    )

    for (const defaults of [tsDefaults, jsDefaults]) {
      const diagnostics = defaults.setDiagnosticsOptions.mock.calls[0][0]
      expect(diagnostics.noSemanticValidation).toBe(true)
      expect(diagnostics.noSuggestionDiagnostics).toBe(true)
      // Genuine malformed-code errors are context-free — still shown.
      expect(diagnostics.noSyntaxValidation).toBe(false)

      // JSX must parse without a compiler-flag diagnostic.
      const compiler = defaults.setCompilerOptions.mock.calls[0][0]
      expect(compiler.jsx).toBe(1)
    }
  })

  it("stops JSON remote-schema fetching but keeps structural validation", () => {
    const { monaco, jsonDefaults } = makeMonaco()

    configureLanguageValidation(
      monaco as unknown as Parameters<typeof configureLanguageValidation>[0]
    )

    const options = jsonDefaults.setDiagnosticsOptions.mock.calls[0][0]
    expect(options.validate).toBe(true)
    expect(options.enableSchemaRequest).toBe(false)
    expect(options.schemaRequest).toBe("ignore")
    expect(options.schemaValidation).toBe("ignore")
  })

  it("is wired into the shared defineMonacoThemes beforeMount hook", () => {
    // All three editor surfaces (file editor, diff viewer, merge editor) mount
    // through `defineMonacoThemes`, so this call is the only thing that carries
    // the validation config onto them. Guard against it being dropped in a
    // refactor — the per-function unit tests above would still pass without it.
    const { monaco, tsDefaults, jsonDefaults } = makeMonaco()

    defineMonacoThemes(
      monaco as unknown as Parameters<typeof defineMonacoThemes>[0]
    )

    expect(tsDefaults.setDiagnosticsOptions).toHaveBeenCalled()
    expect(jsonDefaults.setDiagnosticsOptions).toHaveBeenCalled()
  })

  it("registers one theme per (color, mode) with the canvas background applied", () => {
    const { monaco } = makeMonaco()
    const defineTheme = monaco.editor.defineTheme

    defineMonacoThemes(
      monaco as unknown as Parameters<typeof defineMonacoThemes>[0]
    )

    // 12 theme colors x {light, dark}.
    expect(defineTheme).toHaveBeenCalledTimes(THEME_COLORS.length * 2)

    const call = defineTheme.mock.calls.find(
      (c) => c[0] === monacoThemeName("blue", true)
    )
    expect(call).toBeDefined()
    expect(call?.[1].colors["editor.background"]).toBe(
      EDITOR_CANVAS_BG.blue.dark
    )
    expect(call?.[1].colors["editorGutter.background"]).toBe(
      EDITOR_CANVAS_BG.blue.dark
    )
    // Sticky scroll (pinned parent-scope lines) tracks the canvas: in the opaque
    // base theme it equals the canvas bg (unchanged). The workspace-bg variant
    // makes it transparent and frosts the band in CSS instead (see the wsbg test
    // below) — so it stops being a stark opaque slab over a background image.
    expect(call?.[1].colors["editorStickyScroll.background"]).toBe(
      EDITOR_CANVAS_BG.blue.dark
    )
    expect(call?.[1].colors["editorStickyScrollGutter.background"]).toBe(
      EDITOR_CANVAS_BG.blue.dark
    )
    // The current-line highlight follows the theme's tinted --muted, not a fixed
    // gray, so the focused line carries the accent hue.
    expect(call?.[1].colors["editor.lineHighlightBackground"]).toBe(
      EDITOR_LINE_HIGHLIGHT.blue.dark
    )
  })

  it("lets the sticky-scroll band go transparent with the canvas in the workspace-bg variant", () => {
    // With a workspace background image the canvas is transparent (alpha 0) and
    // the sticky band tracks it, rather than staying an opaque slab: a single
    // full-width frosted surface is painted on `.sticky-widget` in CSS instead
    // (covering the scrollbar strip Monaco's content-width inner layer leaves
    // bare). So the sticky bg must carry the same transparent canvas value.
    const { monaco } = makeMonaco()
    const defineTheme = monaco.editor.defineTheme
    const base = monacoThemeName("blue", true)

    const name = defineWorkspaceBgTheme(
      monaco as unknown as Parameters<typeof defineWorkspaceBgTheme>[0],
      base,
      0
    )

    const call = defineTheme.mock.calls.find((c) => c[0] === name)
    expect(call).toBeDefined()
    // Fully transparent (`…dark` + "00"), matching `editor.background`, so the
    // frosted CSS band shows through uniformly across content, gutter and strip.
    const transparentCanvas = `${EDITOR_CANVAS_BG.blue.dark}00`
    expect(call?.[1].colors["editor.background"]).toBe(transparentCanvas)
    expect(call?.[1].colors["editorStickyScroll.background"]).toBe(
      transparentCanvas
    )
    expect(call?.[1].colors["editorStickyScrollGutter.background"]).toBe(
      transparentCanvas
    )
  })
})

// The focused line's highlight tracks each theme's `--muted` token so it follows
// the accent hue instead of a fixed zinc gray.
describe("EDITOR_LINE_HIGHLIGHT", () => {
  it("has a valid light + dark hex for every theme color", () => {
    for (const color of THEME_COLORS) {
      expect(EDITOR_LINE_HIGHLIGHT[color].light).toMatch(/^#[0-9a-f]{6}$/)
      expect(EDITOR_LINE_HIGHLIGHT[color].dark).toMatch(/^#[0-9a-f]{6}$/)
    }
  })

  it("keeps zinc identical to the previous fixed line highlight", () => {
    // #f4f4f5 / #27272a — the zinc --muted, which was the hard-coded neutral
    // highlight before theming, so the zinc preset is a zero-visual-diff baseline.
    expect(EDITOR_LINE_HIGHLIGHT.zinc.light).toBe("#f4f4f5")
    expect(EDITOR_LINE_HIGHLIGHT.zinc.dark).toBe("#27272a")
  })
})

// The file editor is a card surface: its canvas background tracks each theme's
// `--card` token, so it is no longer a fixed pure-white / #171717 regardless of theme.
describe("EDITOR_CANVAS_BG", () => {
  it("has a valid light + dark hex for every theme color", () => {
    for (const color of THEME_COLORS) {
      expect(EDITOR_CANVAS_BG[color].light).toMatch(/^#[0-9a-f]{6}$/)
      expect(EDITOR_CANVAS_BG[color].dark).toMatch(/^#[0-9a-f]{6}$/)
    }
  })

  it("keeps neutral identical to the previous fixed editor background", () => {
    // #ffffff / #171717 (= oklch(0.205 0 0), the --card dark) — a zero-visual-diff
    // baseline so only the accent presets change appearance.
    expect(EDITOR_CANVAS_BG.neutral.light).toBe("#ffffff")
    expect(EDITOR_CANVAS_BG.neutral.dark).toBe("#171717")
  })

  it("encodes color and mode in the theme name", () => {
    expect(monacoThemeName("neutral", false)).toBe("codeg-light-neutral")
    expect(monacoThemeName("blue", true)).toBe("codeg-dark-blue")
  })
})
