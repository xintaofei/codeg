"use client"

import { useEffect, useState } from "react"
import type { BeforeMount, Monaco } from "@monaco-editor/react"
import type { editor as MonacoEditorNs } from "monaco-editor"
import {
  THEME_COLORS,
  DEFAULT_THEME_COLOR,
  type ThemeColor,
} from "./theme-presets"
import { useWorkspaceBackground } from "@/hooks/use-appearance"

// Editor canvas background per theme color = that theme's `--card` token (see the
// [data-theme="…"] blocks in globals.css). The file editor is a card surface, so it
// stays pure white for the neutral/gray presets in light mode and #171717
// (= oklch(0.205 0 0)) for neutral dark — exactly today's values — while the accent
// presets carry their hue. Keep in sync with the --card values in globals.css.
export const EDITOR_CANVAS_BG: Record<
  ThemeColor,
  { light: string; dark: string }
> = {
  neutral: { light: "#ffffff", dark: "#171717" },
  zinc: { light: "#ffffff", dark: "#18181b" },
  slate: { light: "#ffffff", dark: "#0f172b" },
  stone: { light: "#ffffff", dark: "#1c1917" },
  gray: { light: "#ffffff", dark: "#101828" },
  red: { light: "#fffcfc", dark: "#1d1514" },
  rose: { light: "#fffcfc", dark: "#1d1515" },
  orange: { light: "#fffcfb", dark: "#1d1512" },
  green: { light: "#fbfefc", dark: "#131914" },
  blue: { light: "#fcfdff", dark: "#14171e" },
  yellow: { light: "#fefdfa", dark: "#1a1710" },
  violet: { light: "#fdfdff", dark: "#17161d" },
}

// Current-line highlight per theme color = that theme's `--muted` token (see the
// [data-theme="…"] blocks in globals.css). Follows the accent hue instead of a
// fixed zinc gray, so the focused line reads as the app's muted surface — plain
// gray for the neutral/zinc/slate/stone/gray presets, faintly tinted for the
// accent presets. (Zinc reproduces today's #f4f4f5 / #27272a exactly.) Keep in
// sync with the --muted values in globals.css.
export const EDITOR_LINE_HIGHLIGHT: Record<
  ThemeColor,
  { light: string; dark: string }
> = {
  neutral: { light: "#f5f5f5", dark: "#262626" },
  zinc: { light: "#f4f4f5", dark: "#27272a" },
  slate: { light: "#f1f5f9", dark: "#1d293d" },
  stone: { light: "#f5f5f4", dark: "#292524" },
  gray: { light: "#f3f4f6", dark: "#1e2939" },
  red: { light: "#fdf1f0", dark: "#2f2423" },
  rose: { light: "#fdf1f1", dark: "#2f2425" },
  orange: { light: "#fcf2ed", dark: "#2e2521" },
  green: { light: "#eef7ef", dark: "#222a23" },
  blue: { light: "#eff4fd", dark: "#23282f" },
  yellow: { light: "#f8f4eb", dark: "#2b271f" },
  violet: { light: "#f4f3fc", dark: "#27262f" },
}

// A Monaco theme name encodes both axes: light/dark mode and the active theme color.
// `defineMonacoThemes` registers one per (color, mode); `useMonacoThemeSync` returns
// the matching name so flipping either axis re-applies through the editor `theme` prop.
export function monacoThemeName(color: ThemeColor, dark: boolean): string {
  return `codeg-${dark ? "dark" : "light"}-${color}`
}

// Neutral-preset names, exported as the stable defaults (e.g. the initial value
// before the DOM is read).
export const MONACO_LIGHT_THEME = monacoThemeName(DEFAULT_THEME_COLOR, false)
export const MONACO_DARK_THEME = monacoThemeName(DEFAULT_THEME_COLOR, true)

// Monaco's "unicode highlight" feature boxes characters it deems ambiguous with
// ASCII or non-basic-ASCII. Its default flags ordinary CJK full-width
// punctuation — `：` `；` `，` `！` `？` `（` `）` etc. — turning normal
// Chinese/Japanese prose into a wall of orange boxes (issue #329).
//
// We disable the two mechanisms that flag *visible* characters
// (`ambiguousCharacters`, `nonBasicASCII`) so CJK punctuation renders as plain
// text on every surface. `invisibleCharacters` is left at its default (on):
// surfacing zero-width / BOM characters is genuinely useful and never boxes
// legible text.
//
// Tradeoff, made deliberately: this also stops highlighting genuine homoglyph
// look-alikes (e.g. a Cyrillic `а` posing as `a` in an identifier). For a
// CJK-first editor the false-positive noise on every line of prose far outweighs
// that rare hint. Shared by the file editor, diff viewer, and merge editor so
// they behave consistently.
export const MONACO_UNICODE_HIGHLIGHT_OPTIONS: MonacoEditorNs.IUnicodeHighlightOptions =
  {
    ambiguousCharacters: false,
    nonBasicASCII: false,
  }

export const monacoTokenRules = {
  light: [
    { token: "diff.header", foreground: "52525B", fontStyle: "bold" },
    { token: "diff.meta", foreground: "71717A" },
    { token: "diff.range", foreground: "0369A1", fontStyle: "bold" },
    { token: "diff.file", foreground: "334155" },
    { token: "diff.inserted", foreground: "166534" },
    { token: "diff.deleted", foreground: "991B1B" },
    { token: "diff.context", foreground: "3F3F46" },
  ],
  dark: [
    { token: "diff.header", foreground: "D4D4D8", fontStyle: "bold" },
    { token: "diff.meta", foreground: "A1A1AA" },
    { token: "diff.range", foreground: "7DD3FC", fontStyle: "bold" },
    { token: "diff.file", foreground: "D4D4D8" },
    { token: "diff.inserted", foreground: "86EFAC" },
    { token: "diff.deleted", foreground: "FDA4AF" },
    { token: "diff.context", foreground: "E4E4E7" },
  ],
}

export const monacoThemeColors = {
  light: {
    focusBorder: "#a1a1aa",
    "editor.background": "#ffffff",
    "editor.foreground": "#09090b",
    "editorGutter.background": "#ffffff",
    "editorLineNumber.foreground": "#a1a1aa",
    "editorLineNumber.activeForeground": "#18181b",
    "editor.lineHighlightBackground": "#f4f4f5",
    "editor.selectionBackground": "#e4e4e7",
    "editor.inactiveSelectionBackground": "#f4f4f5",
    "editorWidget.background": "#ffffff",
    "editorWidget.foreground": "#09090b",
    "editorWidget.border": "#e4e4e7",
    "editorHoverWidget.background": "#ffffff",
    "editorHoverWidget.foreground": "#09090b",
    "editorHoverWidget.border": "#e4e4e7",
    "editorHoverWidget.statusBarBackground": "#f4f4f5",
    "editorSuggestWidget.background": "#ffffff",
    "editorSuggestWidget.border": "#e4e4e7",
    "editorSuggestWidget.foreground": "#09090b",
    "editorSuggestWidget.highlightForeground": "#18181b",
    "editorSuggestWidget.selectedBackground": "#f4f4f5",
    "menu.background": "#ffffff",
    "menu.foreground": "#09090b",
    "menu.selectionBackground": "#f4f4f5",
    "menu.selectionForeground": "#09090b",
    "menu.separatorBackground": "#e4e4e7",
    "menu.border": "#e4e4e7",
    "input.background": "#ffffff",
    "input.foreground": "#09090b",
    "input.border": "#e4e4e7",
    "dropdown.background": "#ffffff",
    "dropdown.foreground": "#09090b",
    "dropdown.border": "#e4e4e7",
    "list.hoverBackground": "#f4f4f5",
    "list.activeSelectionBackground": "#f4f4f5",
    "list.activeSelectionForeground": "#09090b",
    "list.inactiveSelectionBackground": "#f4f4f5",
    "list.inactiveSelectionForeground": "#09090b",
    "list.focusOutline": "#a1a1aa",
    "peekView.border": "#e4e4e7",
    "peekViewEditor.background": "#ffffff",
    "peekViewEditor.matchHighlightBackground": "#e4e4e7",
    "peekViewEditorGutter.background": "#ffffff",
    "peekViewResult.background": "#ffffff",
    "peekViewResult.fileForeground": "#09090b",
    "peekViewResult.lineForeground": "#71717a",
    "peekViewResult.matchHighlightBackground": "#e4e4e7",
    "peekViewResult.selectionBackground": "#f4f4f5",
    "peekViewResult.selectionForeground": "#09090b",
    "peekViewTitle.background": "#f4f4f5",
    "peekViewTitleLabel.foreground": "#09090b",
    "peekViewTitleDescription.foreground": "#71717a",
  },
  dark: {
    focusBorder: "#71717a",
    "editor.background": "#171717",
    "editor.foreground": "#fafafa",
    "editorGutter.background": "#171717",
    "editorLineNumber.foreground": "#71717a",
    "editorLineNumber.activeForeground": "#fafafa",
    "editor.lineHighlightBackground": "#27272a",
    "editor.selectionBackground": "#3f3f46",
    "editor.inactiveSelectionBackground": "#27272a",
    "editorWidget.background": "#18181b",
    "editorWidget.foreground": "#fafafa",
    "editorWidget.border": "#27272a",
    "editorHoverWidget.background": "#18181b",
    "editorHoverWidget.foreground": "#fafafa",
    "editorHoverWidget.border": "#27272a",
    "editorHoverWidget.statusBarBackground": "#27272a",
    "editorSuggestWidget.background": "#18181b",
    "editorSuggestWidget.border": "#27272a",
    "editorSuggestWidget.foreground": "#fafafa",
    "editorSuggestWidget.highlightForeground": "#ffffff",
    "editorSuggestWidget.selectedBackground": "#27272a",
    "menu.background": "#18181b",
    "menu.foreground": "#fafafa",
    "menu.selectionBackground": "#27272a",
    "menu.selectionForeground": "#fafafa",
    "menu.separatorBackground": "#3f3f46",
    "menu.border": "#27272a",
    "input.background": "#18181b",
    "input.foreground": "#fafafa",
    "input.border": "#27272a",
    "dropdown.background": "#18181b",
    "dropdown.foreground": "#fafafa",
    "dropdown.border": "#27272a",
    "list.hoverBackground": "#27272a",
    "list.activeSelectionBackground": "#27272a",
    "list.activeSelectionForeground": "#fafafa",
    "list.inactiveSelectionBackground": "#27272a",
    "list.inactiveSelectionForeground": "#fafafa",
    "list.focusOutline": "#71717a",
    "peekView.border": "#27272a",
    "peekViewEditor.background": "#171717",
    "peekViewEditor.matchHighlightBackground": "#3f3f46",
    "peekViewEditorGutter.background": "#171717",
    "peekViewResult.background": "#18181b",
    "peekViewResult.fileForeground": "#fafafa",
    "peekViewResult.lineForeground": "#a1a1aa",
    "peekViewResult.matchHighlightBackground": "#3f3f46",
    "peekViewResult.selectionBackground": "#27272a",
    "peekViewResult.selectionForeground": "#fafafa",
    "peekViewTitle.background": "#27272a",
    "peekViewTitleLabel.foreground": "#fafafa",
    "peekViewTitleDescription.foreground": "#a1a1aa",
  },
}

export const defineDiffLanguage: BeforeMount = (monaco) => {
  const hasDiffLanguage = monaco.languages
    .getLanguages()
    .some((language: { id: string }) => language.id === "diff")

  if (!hasDiffLanguage) {
    monaco.languages.register({ id: "diff" })
  }

  monaco.languages.setMonarchTokensProvider("diff", {
    defaultToken: "diff.context",
    tokenizer: {
      root: [
        [/^diff --git .*$/, "diff.header"],
        [/^index .*$/, "diff.meta"],
        [/^@@ .*@@.*$/, "diff.range"],
        [/^(?:\+\+\+|---) .*$/, "diff.file"],
        [/^\+.*$/, "diff.inserted"],
        [/^-.*$/, "diff.deleted"],
        [/^\\ No newline at end of file$/, "diff.meta"],
        [/^Binary files .* differ$/, "diff.meta"],
        [/^.*$/, "diff.context"],
      ],
    },
  })
}

/**
 * Override Monaco's built-in Python tokenizer to fix triple-quoted string
 * handling. The default monarch tokenizer doesn't correctly parse `f"""..."""`
 * or `"""..."""`, causing everything after the closing quotes to be highlighted
 * as a string.
 */
const fixPythonTripleQuotes: BeforeMount = (monaco) => {
  monaco.languages.setMonarchTokensProvider("python", {
    defaultToken: "",
    keywords: [
      "False",
      "None",
      "True",
      "and",
      "as",
      "assert",
      "async",
      "await",
      "break",
      "class",
      "continue",
      "def",
      "del",
      "elif",
      "else",
      "except",
      "finally",
      "for",
      "from",
      "global",
      "if",
      "import",
      "in",
      "is",
      "lambda",
      "nonlocal",
      "not",
      "or",
      "pass",
      "raise",
      "return",
      "try",
      "while",
      "with",
      "yield",
    ],
    builtins: [
      "abs",
      "all",
      "any",
      "bin",
      "bool",
      "breakpoint",
      "bytearray",
      "bytes",
      "callable",
      "chr",
      "classmethod",
      "compile",
      "complex",
      "delattr",
      "dict",
      "dir",
      "divmod",
      "enumerate",
      "eval",
      "exec",
      "filter",
      "float",
      "format",
      "frozenset",
      "getattr",
      "globals",
      "hasattr",
      "hash",
      "help",
      "hex",
      "id",
      "input",
      "int",
      "isinstance",
      "issubclass",
      "iter",
      "len",
      "list",
      "locals",
      "map",
      "max",
      "memoryview",
      "min",
      "next",
      "object",
      "oct",
      "open",
      "ord",
      "pow",
      "print",
      "property",
      "range",
      "repr",
      "reversed",
      "round",
      "set",
      "setattr",
      "slice",
      "sorted",
      "staticmethod",
      "str",
      "sum",
      "super",
      "tuple",
      "type",
      "vars",
      "zip",
    ],
    brackets: [
      { open: "{", close: "}", token: "delimiter.curly" },
      { open: "[", close: "]", token: "delimiter.bracket" },
      { open: "(", close: ")", token: "delimiter.parenthesis" },
    ],
    tokenizer: {
      root: [
        // decorators
        [/^(\s*)(@\w+)/, ["white", "tag"]],
        // triple-quoted strings (must come before single-quoted)
        [/(?:[fFrRbBuU]{1,2})?"""/, "string", "@tdqs"],
        [/(?:[fFrRbBuU]{1,2})?'''/, "string", "@tsqs"],
        // single-line strings
        [/(?:[fFrRbBuU]{1,2})?"([^"\\]|\\.)*$/, "string.invalid"],
        [/(?:[fFrRbBuU]{1,2})?'([^'\\]|\\.)*$/, "string.invalid"],
        [/(?:[fFrRbBuU]{1,2})?"/, "string", "@dqs"],
        [/(?:[fFrRbBuU]{1,2})?'/, "string", "@sqs"],
        // comments
        [/#.*$/, "comment"],
        // identifiers and keywords
        [
          /[a-zA-Z_]\w*/,
          {
            cases: {
              "@keywords": "keyword",
              "@builtins": "type.identifier",
              "@default": "identifier",
            },
          },
        ],
        // numbers
        [/0[xX][0-9a-fA-F](_?[0-9a-fA-F])*/, "number.hex"],
        [/0[oO][0-7](_?[0-7])*/, "number.octal"],
        [/0[bB][01](_?[01])*/, "number.binary"],
        [/\d[\d_]*(\.\d[\d_]*)?([eE][+-]?\d[\d_]*)?[jJ]?/, "number"],
        // operators
        [/[+\-*/%&|^~<>!=]=?/, "operator"],
        [/[{}()[\]]/, "@brackets"],
        [/[;,.]/, "delimiter"],
      ],
      // triple-double-quoted string
      tdqs: [
        [/[^"\\]+/, "string"],
        [/\\./, "string.escape"],
        [/"""/, "string", "@pop"],
        [/"/, "string"],
      ],
      // triple-single-quoted string
      tsqs: [
        [/[^'\\]+/, "string"],
        [/\\./, "string.escape"],
        [/'''/, "string", "@pop"],
        [/'/, "string"],
      ],
      // double-quoted string
      dqs: [
        [/[^"\\]+/, "string"],
        [/\\./, "string.escape"],
        [/"/, "string", "@pop"],
      ],
      // single-quoted string
      sqs: [
        [/[^'\\]+/, "string"],
        [/\\./, "string.escape"],
        [/'/, "string", "@pop"],
      ],
    },
  })
}

// Codeg renders files from arbitrary projects but never loads their build
// context — there is no tsconfig, no `node_modules`, no `--jsx` flag, and no
// network access to fetch a `$schema` URL. Monaco's bundled TypeScript and JSON
// language services don't know that, so they decorate ordinary files with
// squiggles that are *always* false positives here:
//
//   - "Cannot find namespace 'React'."              (no @types/react resolved)
//   - "Cannot find module '@/components/…'."         (path alias unresolved)
//   - "Cannot use JSX unless the '--jsx' flag …"     (no compiler config)
//   - "Unable to load schema from 'https://…'."      (no schema request service)
//
// In a read-oriented viewer these mislead far more than they help, so we switch
// off the *environment-dependent* checks (type/module resolution, remote-schema
// validation) while keeping the checks that are genuinely context-free and
// still useful: plain TS/JS *syntax* errors and JSON *structural* errors.
//
// These settings are global to Monaco (not per-editor) and idempotent, so
// running them from every surface's `beforeMount` is safe.
export const configureLanguageValidation: BeforeMount = (monaco) => {
  const ts = monaco.languages.typescript
  if (ts) {
    // Permissive baseline so JSX/TSX parses and no compiler-flag diagnostic can
    // fire. Module resolution is moot with semantic validation off, but the
    // values keep tokenization and other language features well-behaved.
    const compilerOptions = {
      allowJs: true,
      allowNonTsExtensions: true,
      jsx: ts.JsxEmit.Preserve,
      target: ts.ScriptTarget.ESNext,
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.NodeJs,
      esModuleInterop: true,
      noEmit: true,
    }
    const diagnosticsOptions = {
      // Type/module/namespace/JSX-flag errors all need a real project graph we
      // never load → false positives without exception.
      noSemanticValidation: true,
      // Genuinely malformed code is context-free; keep surfacing it.
      noSyntaxValidation: false,
      // Suggestion-level hints (unused symbol, "could be const", …) also lean
      // on project context.
      noSuggestionDiagnostics: true,
    }
    ts.typescriptDefaults.setCompilerOptions(compilerOptions)
    ts.javascriptDefaults.setCompilerOptions(compilerOptions)
    ts.typescriptDefaults.setDiagnosticsOptions(diagnosticsOptions)
    ts.javascriptDefaults.setDiagnosticsOptions(diagnosticsOptions)
  }

  const json = monaco.languages.json
  if (json) {
    json.jsonDefaults.setDiagnosticsOptions({
      // Keep JSON structural validation — a stray comma or missing brace is a
      // real, context-free error worth flagging.
      validate: true,
      // Never reach out for a remote schema: we're often offline, and the fetch
      // failure is exactly what surfaces as "No schema request service
      // available".
      enableSchemaRequest: false,
      // Suppress any leftover schema-resolution problems …
      schemaRequest: "ignore",
      // … and don't validate a perfectly valid config against a schema we can't
      // load or that doesn't apply outside its own project.
      schemaValidation: "ignore",
    })
  }
}

// Overlay the theme color's canvas background onto the base surface colors so the
// editor / gutter / peek surfaces read as the app's card instead of a fixed
// white/black, and tint the current-line highlight to the theme's `--muted` so the
// focused line follows the accent hue instead of a fixed zinc gray. Selection and
// widgets keep their neutral shadcn values. The line highlight stays opaque even
// when the canvas goes translucent (alphaHexSuffix), matching the pre-existing
// behaviour, so the focused line stays legible over a background image.
function withCanvasBackground(
  base: (typeof monacoThemeColors)["light"],
  color: ThemeColor,
  dark: boolean,
  // When set (a 2-hex-digit alpha like "b8"), the canvas backgrounds become
  // semi-transparent 8-digit hex so a workspace background image shows through
  // the editor. Empty (default) keeps them fully opaque — unchanged behaviour.
  alphaHexSuffix = ""
): Record<string, string> {
  const opaqueBg = EDITOR_CANVAS_BG[color][dark ? "dark" : "light"]
  const bg = opaqueBg + alphaHexSuffix
  return {
    ...base,
    "editor.background": bg,
    "editorGutter.background": bg,
    "peekViewEditor.background": bg,
    "peekViewEditorGutter.background": bg,
    // Sticky scroll (pinned parent-scope lines) defaults its background to
    // `editor.background`; keep it tracking the canvas here (so it follows `bg`,
    // not a fixed value). In the opaque base theme that's the solid canvas colour
    // — unchanged. When a workspace background image makes the canvas translucent,
    // a fully OPAQUE sticky band read as a stark dark slab floating over the image
    // ("一坨黑色"), and Monaco's inner `.sticky-widget-lines-scrollable` only spans
    // the content width — leaving the vertical-scrollbar strip bare so scrolled
    // code bled through on the right. So we let the band go transparent with the
    // canvas and instead paint ONE full-width frosted surface on `.sticky-widget`
    // in CSS (globals.css `[data-workspace-bg="on"] .monaco-editor .sticky-widget`):
    // a translucent tint + backdrop blur that covers the strip and blends the
    // header into the frosted-panel aesthetic while keeping the pinned lines
    // legible. `bg` = opaque canvas when off (zero regression), transparent when on.
    "editorStickyScroll.background": bg,
    "editorStickyScrollGutter.background": bg,
    "editor.lineHighlightBackground":
      EDITOR_LINE_HIGHLIGHT[color][dark ? "dark" : "light"],
  }
}

export const defineMonacoThemes: BeforeMount = (monaco) => {
  defineDiffLanguage(monaco)
  fixPythonTripleQuotes(monaco)
  configureLanguageValidation(monaco)

  // One theme per (color, mode). `defineTheme` just stores a config, so this is
  // cheap and idempotent even re-run on every editor mount.
  for (const color of THEME_COLORS) {
    monaco.editor.defineTheme(monacoThemeName(color, false), {
      base: "vs",
      inherit: true,
      rules: monacoTokenRules.light,
      colors: withCanvasBackground(monacoThemeColors.light, color, false),
    })
    monaco.editor.defineTheme(monacoThemeName(color, true), {
      base: "vs-dark",
      inherit: true,
      rules: monacoTokenRules.dark,
      colors: withCanvasBackground(monacoThemeColors.dark, color, true),
    })
  }
}

export function useMonacoThemeSync() {
  const [theme, setTheme] = useState(MONACO_LIGHT_THEME)

  useEffect(() => {
    if (typeof window === "undefined") return
    const root = document.documentElement

    const syncTheme = () => {
      const dark = root.classList.contains("dark")
      const attr = root.getAttribute("data-theme")
      const color = (THEME_COLORS as readonly string[]).includes(attr ?? "")
        ? (attr as ThemeColor)
        : DEFAULT_THEME_COLOR
      setTheme(monacoThemeName(color, dark))
    }

    syncTheme()

    // Watch both axes: `class` (light/dark via next-themes) and `data-theme`
    // (theme color via AppearanceProvider). Either flip re-applies the matching theme.
    const observer = new MutationObserver(syncTheme)
    observer.observe(root, {
      attributes: true,
      attributeFilter: ["class", "data-theme"],
    })
    return () => {
      observer.disconnect()
    }
  }, [])

  return theme
}

// ———————————————————————————————————————————————————————————————————————————
// Workspace 背景图片：编辑器画布全透明，与会话区 canvas 一致，透出背景图 + 遮罩
// ———————————————————————————————————————————————————————————————————————————

// 2-hex-digit alpha (00–ff) from a 0–1 opacity, for Monaco's 8-digit hex colors.
function editorAlphaHex(alpha: number): string {
  const a = Math.round(Math.min(1, Math.max(0, alpha)) * 255)
  return a.toString(16).padStart(2, "0")
}

// A distinct theme name per (base, alpha bucket), bucketed to whole percents so
// the name space — and the defineTheme churn — stays bounded even if the canvas
// alpha ever varies. Today the canvas is a fixed fully-transparent 0 (matching
// the conversation), so this resolves to a single `…-wsbg0` per (color, mode).
export function monacoWsbgThemeName(baseTheme: string, alpha: number): string {
  const pct = Math.round(Math.min(1, Math.max(0, alpha)) * 100)
  return `${baseTheme}-wsbg${pct}`
}

// Recover (color, dark) from a base name produced by `monacoThemeName`
// (`codeg-{light|dark}-{color}`).
function parseMonacoThemeName(baseTheme: string): {
  color: ThemeColor
  dark: boolean
} {
  const dark = baseTheme.includes("-dark-")
  const tail = baseTheme.slice(baseTheme.lastIndexOf("-") + 1)
  const color = (THEME_COLORS as readonly string[]).includes(tail)
    ? (tail as ThemeColor)
    : DEFAULT_THEME_COLOR
  return { color, dark }
}

// Per-instance record of already-defined wsbg theme names, so redefinition is a
// genuine no-op: re-`defineTheme`ing the ACTIVE theme re-fires Monaco's theme
// event (a synchronous CSS refresh), and this hook may run on every render. The
// name encodes (base, alpha bucket), so once a name is defined its colors are
// fixed — a plain "seen" set is sufficient. Keyed by the monaco instance (via
// WeakMap) so a fresh instance (HMR / reload) re-defines rather than trusting a
// stale flag.
const wsbgDefinedThemes = new WeakMap<Monaco, Set<string>>()

// Idempotently define a transparent-canvas variant of `baseTheme` whose
// editor/gutter/peek backgrounds carry `alpha`, and return its name. MUST run
// against the SAME monaco instance the editors use (the AMD loader in
// monaco-local.ts means an ESM import would be a different instance whose themes
// the live editor never sees) — callers pass the instance from the editor's
// onMount (or useMonaco()).
export function defineWorkspaceBgTheme(
  monaco: Monaco,
  baseTheme: string,
  alpha: number
): string {
  const name = monacoWsbgThemeName(baseTheme, alpha)
  let defined = wsbgDefinedThemes.get(monaco)
  if (!defined) {
    defined = new Set()
    wsbgDefinedThemes.set(monaco, defined)
  }
  if (defined.has(name)) return name

  const { color, dark } = parseMonacoThemeName(baseTheme)
  monaco.editor.defineTheme(name, {
    base: dark ? "vs-dark" : "vs",
    inherit: true,
    rules: dark ? monacoTokenRules.dark : monacoTokenRules.light,
    // Only the canvas surfaces go translucent; selection, line-highlight and
    // widgets keep their neutral opaque values so code stays readable.
    colors: withCanvasBackground(
      dark ? monacoThemeColors.dark : monacoThemeColors.light,
      color,
      dark,
      editorAlphaHex(alpha)
    ),
  })
  defined.add(name)
  return name
}

// The editor canvas is a fully transparent (alpha 0) surface, so the code area
// matches the conversation canvas exactly — both let the masked background image
// show straight through. Readability comes from the shared background mask (and
// the image-blur setting), not a per-editor tint, so the file side and the chat side
// stay consistent as those sliders move. (The panel-opacity slider governs only
// the frosted chrome — headers, toolbars, composer — not this canvas.)
const WSBG_CANVAS_ALPHA = 0

// Like `useMonacoThemeSync`, but when a workspace background image is enabled it
// swaps in a fully transparent-canvas theme (see `WSBG_CANVAS_ALPHA`) so the code
// area reads like the conversation canvas rather than a frosted panel. Disabled →
// the opaque base theme, visually unchanged (zero regression: the sticky-scroll
// keys equal `editor.background` when opaque). Shared by the file editor, diff
// viewer and merge editor.
//
// The caller supplies the loaded monaco instance (from the editor's onMount, or
// `useMonaco()` for conditionally-mounted editors) rather than this hook calling
// `useMonaco()` itself — so an always-mounted host (FileWorkspacePanel's empty
// state) never eagerly loads Monaco just to read a theme name. `monaco` is null
// until an editor mounts → we return the base theme; the editor's mount bumps
// state and re-runs this with the instance.
export function useMonacoWorkspaceTheme(monaco: Monaco | null): string {
  const baseTheme = useMonacoThemeSync()
  const { workspaceBgEnabled } = useWorkspaceBackground()

  // Off, or monaco not available yet → the opaque base theme (zero regression).
  if (!workspaceBgEnabled || !monaco) return baseTheme

  // The resolved name is derived state, computed in render (not an effect) so a
  // theme (color / dark) change re-applies via the `theme` prop with no setState
  // churn. `defineWorkspaceBgTheme` is idempotent (per-instance cache) and
  // registering the variant here guarantees it exists BEFORE @monaco-editor/react
  // applies the name, so the editor never falls back to a built-in vs/vs-dark
  // theme.
  return defineWorkspaceBgTheme(monaco, baseTheme, WSBG_CANVAS_ALPHA)
}
