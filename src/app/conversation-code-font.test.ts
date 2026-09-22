import { readFileSync } from "node:fs"
import { resolve } from "node:path"

import {
  DEFAULT_EDITOR_FONT_ID,
  conversationCodeFontStack,
} from "@/lib/font-presets"

const globalsCss = readFileSync(
  resolve(process.cwd(), "src/app/globals.css"),
  "utf8"
)
const codeBlock = readFileSync(
  resolve(process.cwd(), "src/components/ai-elements/code-block.tsx"),
  "utf8"
)

/**
 * Issue #761: conversation code (Streamdown fences and inline code, plus the
 * shared CodeBlock used for tool output) must follow the editor monospace
 * stack published as `--font-code`. The fallback in CSS has to equal the
 * default stack or the first paint flashes.
 */
describe("conversation code font wiring", () => {
  it("points chat code surfaces at --font-code with the default editor stack as fallback", () => {
    const fallback = conversationCodeFontStack(DEFAULT_EDITOR_FONT_ID, "")
    const rule = globalsCss.match(
      /\[data-streamdown="code-block-body"\][\s\S]*?\{[^}]*font-family:\s*([^;]+);/
    )

    const declared = rule?.[1]
      .replace(/\s+/g, " ")
      .trim()
      .replace(/\(\s+/g, "(")
      .replace(/\s+\)/g, ")")
    expect(declared).toBe(`var(--font-code, ${fallback})`)
    expect(globalsCss).toContain('[data-streamdown="inline-code"]')
    expect(globalsCss).toContain(".codeg-code-font")
    expect(codeBlock).toContain("codeg-code-font")
  })
})
