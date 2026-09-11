// src/lib/color-contrast.ts
//
// Dependency-free colour math for the appearance presets: parse the CSS colour
// syntaxes the theme tokens actually use (hex, rgb(), hsl(), oklch(), oklab()
// and the transparent keyword) into linear sRGB, resolve `var()` references,
// and compute WCAG 2 contrast ratios.
//
// This is deliberately NOT a general CSS colour parser. `color-mix()` and the
// relative colour syntax are unsupported on purpose: a preset that wants a
// contrast guarantee has to spell its text / surface pairs out as literal
// colours, which is also what makes them readable in the JSON.

/** Linear-light sRGB, every channel 0..1 (out-of-gamut input is clamped). */
export type LinearRgba = { r: number; g: number; b: number; a: number }

const KEYWORDS: Record<string, LinearRgba> = {
  transparent: { r: 0, g: 0, b: 0, a: 0 },
  white: { r: 1, g: 1, b: 1, a: 1 },
  black: { r: 0, g: 0, b: 0, a: 1 },
}

function clamp01(n: number): number {
  return n < 0 ? 0 : n > 1 ? 1 : n
}

/** sRGB transfer function, gamma-encoded 0..1 to linear 0..1. */
function toLinear(channel: number): number {
  const c = clamp01(channel)
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}

function parseNumber(token: string): number | null {
  if (!/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(token)) return null
  const n = Number(token)
  return Number.isFinite(n) ? n : null
}

/** `50%` -> 0.5, `0.5` -> 0.5, `none` -> 0; null when it is neither. */
function parseAlpha(token: string | undefined): number | null {
  if (token === undefined) return 1
  if (token === "none") return 0
  if (token.endsWith("%")) {
    const n = parseNumber(token.slice(0, -1))
    return n === null ? null : clamp01(n / 100)
  }
  const n = parseNumber(token)
  return n === null ? null : clamp01(n)
}

/**
 * Split a functional colour's arguments into positional channels plus the
 * optional alpha. Accepts both the modern space-separated form with `/` and
 * the legacy comma form (`rgba(1, 2, 3, 0.5)`).
 */
function splitArgs(
  body: string
): { channels: string[]; alpha: string | undefined } | null {
  const trimmed = body.trim()
  if (!trimmed) return null
  let channelsPart = trimmed
  let alpha: string | undefined
  const slash = trimmed.indexOf("/")
  if (slash >= 0) {
    channelsPart = trimmed.slice(0, slash)
    alpha = trimmed.slice(slash + 1).trim()
    if (!alpha || alpha.includes("/")) return null
  }
  const parts = channelsPart
    .split(/[\s,]+/)
    .map((p) => p.trim())
    .filter(Boolean)
  if (alpha === undefined && parts.length === 4) {
    // Legacy `rgba(r, g, b, a)` / `hsla(h, s, l, a)`.
    alpha = parts.pop()
  }
  if (parts.length !== 3) return null
  return { channels: parts, alpha }
}

function parseHex(hex: string): LinearRgba | null {
  const digits = hex.slice(1)
  if (!/^[0-9a-f]+$/i.test(digits)) return null
  let r: number, g: number, b: number, a: number
  if (digits.length === 3 || digits.length === 4) {
    const [rr, gg, bb, aa] = digits.split("").map((d) => parseInt(d + d, 16))
    r = rr
    g = gg
    b = bb
    a = digits.length === 4 ? aa : 255
  } else if (digits.length === 6 || digits.length === 8) {
    r = parseInt(digits.slice(0, 2), 16)
    g = parseInt(digits.slice(2, 4), 16)
    b = parseInt(digits.slice(4, 6), 16)
    a = digits.length === 8 ? parseInt(digits.slice(6, 8), 16) : 255
  } else {
    return null
  }
  return {
    r: toLinear(r / 255),
    g: toLinear(g / 255),
    b: toLinear(b / 255),
    a: a / 255,
  }
}

/** `255` or `100%` -> gamma-encoded 0..1. */
function parseRgbChannel(token: string): number | null {
  if (token === "none") return 0
  if (token.endsWith("%")) {
    const n = parseNumber(token.slice(0, -1))
    return n === null ? null : clamp01(n / 100)
  }
  const n = parseNumber(token)
  return n === null ? null : clamp01(n / 255)
}

function parseRgb(body: string): LinearRgba | null {
  const args = splitArgs(body)
  if (!args) return null
  const channels = args.channels.map(parseRgbChannel)
  const alpha = parseAlpha(args.alpha)
  if (channels.some((c) => c === null) || alpha === null) return null
  const [r, g, b] = channels as number[]
  return { r: toLinear(r), g: toLinear(g), b: toLinear(b), a: alpha }
}

function parseHue(token: string): number | null {
  if (token === "none") return 0
  const match = /^([+-]?(?:\d+\.?\d*|\.\d+))(deg|grad|rad|turn)?$/i.exec(token)
  if (!match) return null
  const n = Number(match[1])
  switch ((match[2] ?? "deg").toLowerCase()) {
    case "grad":
      return n * 0.9
    case "rad":
      return (n * 180) / Math.PI
    case "turn":
      return n * 360
    default:
      return n
  }
}

function parsePercentOrUnit(token: string, scale: number): number | null {
  if (token === "none") return 0
  if (token.endsWith("%")) {
    const n = parseNumber(token.slice(0, -1))
    return n === null ? null : (n / 100) * scale
  }
  return parseNumber(token)
}

function hslToLinear(h: number, s: number, l: number, a: number): LinearRgba {
  const hue = (((h % 360) + 360) % 360) / 30
  const chroma = (1 - Math.abs(2 * l - 1)) * s
  const x = chroma * (1 - Math.abs((hue % 2) - 1))
  let r = 0
  let g = 0
  let b = 0
  if (hue < 1) [r, g, b] = [chroma, x, 0]
  else if (hue < 2) [r, g, b] = [x, chroma, 0]
  else if (hue < 3) [r, g, b] = [0, chroma, x]
  else if (hue < 4) [r, g, b] = [0, x, chroma]
  else if (hue < 5) [r, g, b] = [x, 0, chroma]
  else [r, g, b] = [chroma, 0, x]
  const m = l - chroma / 2
  return {
    r: toLinear(r + m),
    g: toLinear(g + m),
    b: toLinear(b + m),
    a,
  }
}

function parseHsl(body: string): LinearRgba | null {
  const args = splitArgs(body)
  if (!args) return null
  const h = parseHue(args.channels[0])
  const s = parsePercentOrUnit(args.channels[1], 1)
  const l = parsePercentOrUnit(args.channels[2], 1)
  const alpha = parseAlpha(args.alpha)
  if (h === null || s === null || l === null || alpha === null) return null
  return hslToLinear(h, clamp01(s), clamp01(l), alpha)
}

/** OKLab -> linear sRGB (Björn Ottosson's reference matrices). */
function oklabToLinear(L: number, a: number, b: number, alpha: number) {
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b
  const s_ = L - 0.0894841775 * a - 1.291485548 * b
  const l = l_ ** 3
  const m = m_ ** 3
  const s = s_ ** 3
  return {
    r: clamp01(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    g: clamp01(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    b: clamp01(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
    a: alpha,
  }
}

function parseOklch(body: string): LinearRgba | null {
  const args = splitArgs(body)
  if (!args) return null
  const L = parsePercentOrUnit(args.channels[0], 1)
  // The `%` reference for chroma is 0.4 per the spec.
  const C = parsePercentOrUnit(args.channels[1], 0.4)
  const H = parseHue(args.channels[2])
  const alpha = parseAlpha(args.alpha)
  if (L === null || C === null || H === null || alpha === null) return null
  const rad = (H * Math.PI) / 180
  return oklabToLinear(
    clamp01(L),
    Math.max(0, C) * Math.cos(rad),
    Math.max(0, C) * Math.sin(rad),
    alpha
  )
}

function parseOklab(body: string): LinearRgba | null {
  const args = splitArgs(body)
  if (!args) return null
  const L = parsePercentOrUnit(args.channels[0], 1)
  // `%` reference for a / b is 0.4 per the spec.
  const a = parsePercentOrUnit(args.channels[1], 0.4)
  const b = parsePercentOrUnit(args.channels[2], 0.4)
  const alpha = parseAlpha(args.alpha)
  if (L === null || a === null || b === null || alpha === null) return null
  return oklabToLinear(clamp01(L), a, b, alpha)
}

/**
 * Parse a literal CSS colour. Returns null for anything it does not
 * understand, including `var()` (see {@link resolveCssColor}) and
 * `color-mix()`.
 */
export function parseCssColor(value: string): LinearRgba | null {
  const v = value.trim().toLowerCase()
  if (!v) return null
  if (v in KEYWORDS) return KEYWORDS[v]
  if (v.startsWith("#")) return parseHex(v)
  const fn = /^([a-z]+)\((.*)\)$/s.exec(v)
  if (!fn) return null
  const body = fn[2]
  switch (fn[1]) {
    case "rgb":
    case "rgba":
      return parseRgb(body)
    case "hsl":
    case "hsla":
      return parseHsl(body)
    case "oklch":
      return parseOklch(body)
    case "oklab":
      return parseOklab(body)
    default:
      return null
  }
}

/**
 * Resolve `var(--name)` chains (with an optional fallback) through `lookup`
 * and parse the literal at the end. Cycles and unknown names resolve to null
 * rather than throwing, so a test can report the token that failed.
 */
export function resolveCssColor(
  value: string,
  lookup: (name: string) => string | undefined,
  depth = 0
): LinearRgba | null {
  const v = value.trim()
  const ref = /^var\(\s*(--[a-zA-Z0-9-]+)\s*(?:,\s*(.*))?\)$/s.exec(v)
  if (!ref) return parseCssColor(v)
  if (depth > 16) return null
  const next = lookup(ref[1].slice(2))
  if (next !== undefined) return resolveCssColor(next, lookup, depth + 1)
  return ref[2] !== undefined
    ? resolveCssColor(ref[2], lookup, depth + 1)
    : null
}

/** Source-over compositing in linear light; the backdrop is taken as opaque. */
export function compositeOver(
  top: LinearRgba,
  backdrop: LinearRgba
): LinearRgba {
  const a = clamp01(top.a)
  return {
    r: top.r * a + backdrop.r * (1 - a),
    g: top.g * a + backdrop.g * (1 - a),
    b: top.b * a + backdrop.b * (1 - a),
    a: 1,
  }
}

/** WCAG 2 relative luminance of an opaque colour. */
export function relativeLuminance(color: LinearRgba): number {
  return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b
}

/**
 * WCAG 2 contrast ratio between text and its surface. A translucent text
 * colour is composited over the surface first, and a translucent surface over
 * `base` (the page background) so `transparent` bubbles are judged against
 * what actually shows through.
 */
export function contrastRatio(
  text: LinearRgba,
  surface: LinearRgba,
  base: LinearRgba = KEYWORDS.white
): number {
  const bg = surface.a < 1 ? compositeOver(surface, base) : surface
  const fg = text.a < 1 ? compositeOver(text, bg) : text
  const l1 = relativeLuminance(fg)
  const l2 = relativeLuminance(bg)
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1]
  return (hi + 0.05) / (lo + 0.05)
}
