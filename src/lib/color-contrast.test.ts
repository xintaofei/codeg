import { describe, expect, it } from "vitest"

import {
  compositeOver,
  contrastRatio,
  parseCssColor,
  relativeLuminance,
  resolveCssColor,
} from "./color-contrast"

const white = parseCssColor("#ffffff")!
const black = parseCssColor("#000000")!

describe("parseCssColor", () => {
  it("reads every hex form", () => {
    expect(parseCssColor("#fff")).toEqual(white)
    expect(parseCssColor("#FFFFFF")).toEqual(white)
    expect(parseCssColor("#0000")?.a).toBe(0)
    expect(parseCssColor("#00000080")?.a).toBeCloseTo(128 / 255, 5)
    expect(parseCssColor("#12345")).toBeNull()
    expect(parseCssColor("#gggggg")).toBeNull()
  })

  it("reads rgb() and hsl() in the modern and legacy forms", () => {
    expect(parseCssColor("rgb(255 255 255)")).toEqual(white)
    expect(parseCssColor("rgba(255, 255, 255, 1)")).toEqual(white)
    expect(parseCssColor("rgb(100% 100% 100% / 50%)")?.a).toBe(0.5)
    expect(parseCssColor("hsl(0 0% 100%)")).toEqual(white)
    expect(parseCssColor("hsl(0, 0%, 0%, 0.25)")?.a).toBe(0.25)
    // Pure red in hsl is pure red in rgb.
    const red = parseCssColor("hsl(0 100% 50%)")!
    expect(red.r).toBeCloseTo(1, 5)
    expect(red.g).toBeCloseTo(0, 5)
    expect(red.b).toBeCloseTo(0, 5)
  })

  it("converts oklch() and oklab() to sRGB", () => {
    // oklch(1 0 0) is white, oklch(0 0 0) is black.
    const w = parseCssColor("oklch(1 0 0)")!
    expect(w.r).toBeCloseTo(1, 3)
    expect(w.g).toBeCloseTo(1, 3)
    expect(w.b).toBeCloseTo(1, 3)
    const b = parseCssColor("oklch(0 0 0)")!
    expect(b.r).toBeCloseTo(0, 3)
    expect(b.g).toBeCloseTo(0, 3)
    expect(b.b).toBeCloseTo(0, 3)
    // A mid grey: luminance is L cubed for an achromatic colour.
    const grey = parseCssColor("oklch(0.5 0 0)")!
    expect(relativeLuminance(grey)).toBeCloseTo(0.125, 2)
    // Percent lightness and an explicit hue unit.
    expect(parseCssColor("oklch(50% 0 90deg)")?.r).toBeCloseTo(grey.r, 5)
    // Alpha survives the slash syntax.
    expect(parseCssColor("oklch(1 0 0 / 10%)")?.a).toBeCloseTo(0.1, 5)
    // oklab agrees with oklch at zero chroma.
    expect(parseCssColor("oklab(0.5 0 0)")?.r).toBeCloseTo(grey.r, 5)
  })

  it("keeps a chromatic oklch inside the sRGB cube", () => {
    // shadcn's own violet primary is in gamut; a wildly saturated one is not,
    // and must come back clamped rather than negative.
    const violet = parseCssColor("oklch(0.606 0.25 292.717)")!
    for (const c of [violet.r, violet.g, violet.b]) {
      expect(c).toBeGreaterThanOrEqual(0)
      expect(c).toBeLessThanOrEqual(1)
    }
    const wild = parseCssColor("oklch(0.9 0.4 150)")!
    for (const c of [wild.r, wild.g, wild.b]) {
      expect(c).toBeGreaterThanOrEqual(0)
      expect(c).toBeLessThanOrEqual(1)
    }
  })

  it("refuses what it does not understand instead of guessing", () => {
    expect(parseCssColor("plum")).toBeNull()
    expect(parseCssColor("color-mix(in oklab, #fff 40%, #000)")).toBeNull()
    expect(parseCssColor("var(--primary)")).toBeNull()
    expect(parseCssColor("oklch(0.5 0)")).toBeNull()
    expect(parseCssColor("rgb(1 2 3 4 5)")).toBeNull()
    expect(parseCssColor("")).toBeNull()
    expect(parseCssColor("url(https://example.com/x.png)")).toBeNull()
  })
})

describe("resolveCssColor", () => {
  const vars: Record<string, string> = {
    background: "#ffffff",
    "bubble-user-bg": "var(--secondary)",
    secondary: "var(--muted)",
    muted: "oklch(0.97 0 0)",
    loop: "var(--loop)",
  }
  const lookup = (name: string) => vars[name]

  it("follows var() chains to the literal at the end", () => {
    const resolved = resolveCssColor("var(--bubble-user-bg)", lookup)!
    expect(relativeLuminance(resolved)).toBeCloseTo(0.97 ** 3, 2)
  })

  it("uses the fallback when a name is unknown, and null when there is none", () => {
    expect(resolveCssColor("var(--nope, #000000)", lookup)).toEqual(black)
    expect(resolveCssColor("var(--nope)", lookup)).toBeNull()
  })

  it("does not spin on a cycle", () => {
    expect(resolveCssColor("var(--loop)", lookup)).toBeNull()
  })
})

describe("contrastRatio", () => {
  it("is 21:1 for black on white and 1:1 for a colour on itself", () => {
    expect(contrastRatio(black, white)).toBeCloseTo(21, 5)
    expect(contrastRatio(white, black)).toBeCloseTo(21, 5)
    expect(contrastRatio(white, white)).toBeCloseTo(1, 5)
  })

  it("matches the published value for a known pair", () => {
    // #767676 on white is the classic 4.54:1 AA boundary example.
    const grey = parseCssColor("#767676")!
    expect(contrastRatio(grey, white)).toBeCloseTo(4.54, 1)
  })

  it("composites translucent text and surfaces before comparing", () => {
    const halfBlack = parseCssColor("rgb(0 0 0 / 50%)")!
    // Half-black text on white reads as a mid grey, not as black.
    expect(contrastRatio(halfBlack, white)).toBeLessThan(21)
    expect(contrastRatio(halfBlack, white)).toBeGreaterThan(1)
    // A transparent surface is judged against the base it sits on.
    const transparent = parseCssColor("transparent")!
    expect(contrastRatio(black, transparent, white)).toBeCloseTo(21, 5)
    expect(contrastRatio(white, transparent, black)).toBeCloseTo(21, 5)
    expect(compositeOver(transparent, white)).toEqual(white)
  })
})
