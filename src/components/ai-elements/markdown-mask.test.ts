import { describe, expect, it } from "vitest"

import { maskForTranslation, maskLiteralSpans } from "./markdown-mask"

describe("maskForTranslation", () => {
  it("restores code, link destinations, math, and HTML byte-for-byte", () => {
    const source = [
      "Run `pnpm test` and read [the docs](https://example.com/a?q=1).",
      "Solve $x + y$ and $$z = 1$$, then press <kbd>Enter</kbd>.",
      "```ts",
      "const url = 'https://example.com'",
      "```",
    ].join("\n")

    const { masked, restore } = maskForTranslation(source)
    const rewritten = masked.replace(/Run/, "执行").replace(/read/, "阅读")
    const restored = restore(rewritten)

    expect(masked).not.toContain("pnpm test")
    expect(masked).not.toContain("https://example.com/a?q=1")
    expect(masked).not.toContain("$x + y$")
    expect(masked).not.toContain("<kbd>")
    expect(restored).toContain("`pnpm test`")
    expect(restored).toContain("](https://example.com/a?q=1)")
    expect(restored).toContain("$x + y$")
    expect(restored).toContain("$$z = 1$$")
    expect(restored).toContain("<kbd>Enter</kbd>")
    expect(restored).toContain("const url = 'https://example.com'")
  })

  it("round-trips unmatched markdown without changing it", () => {
    const source = "An unfinished `code span and [plain label]."
    const masked = maskForTranslation(source)

    expect(masked.restore(masked.masked)).toBe(source)
  })

  it("does not consume a literal placeholder already present in prose", () => {
    const source = "literal [[CBLK0]] then `protected`"
    const masked = maskLiteralSpans(source)

    expect(masked.restore(masked.masked)).toBe(source)
  })

  it("masks translation-bound text with the ASCII sentinel", () => {
    // The ASCII token survives relay sanitization and is copyable by the
    // model; the NUL default stays in-process only.
    const source = "Run `pnpm test` now"
    const { masked, restore } = maskForTranslation(source)

    expect(masked).toContain("[[CBLK0]]")
    expect(masked).not.toMatch(/\0/)
    expect(restore("执行 [[CBLK0]]")).toBe("执行 `pnpm test`")
  })

  it("keeps the NUL sentinel for in-process rewrites", () => {
    const { masked, restore } = maskLiteralSpans("a `code` b")

    expect(masked).toMatch(/\0CBLK0\0/)
    expect(restore(`x \0CBLK0\0 y`)).toBe("x `code` y")
  })

  it("escalates the collision prefix independently per sentinel", () => {
    const bracket = maskForTranslation("literal [[CBLK0]] then `protected`")
    expect(bracket.masked).toContain("[[_CBLK0]]")
    expect(bracket.restore(bracket.masked)).toBe(
      "literal [[CBLK0]] then `protected`"
    )

    const nul = maskLiteralSpans("literal \0CBLK0\0 then `protected`")
    expect(nul.masked).toContain("\0_CBLK0\0")
    expect(nul.restore(nul.masked)).toBe("literal \0CBLK0\0 then `protected`")
  })
})
