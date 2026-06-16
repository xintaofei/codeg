import { describe, expect, it } from "vitest"

import { unescapeComposerText } from "./composer-copy-text"

describe("unescapeComposerText", () => {
  it("collapses the doubled backslashes of a Windows path (the reported bug)", () => {
    expect(unescapeComposerText("C:\\\\tools\\\\codeg\\\\xxx.xx")).toBe(
      "C:\\tools\\codeg\\xxx.xx"
    )
  })

  it("drops the backslash from escaped inline Markdown punctuation", () => {
    expect(
      unescapeComposerText("\\*a\\* \\_b\\_ \\`c\\` \\[d\\] \\~e\\~")
    ).toBe("*a* _b_ `c` [d] ~e~")
  })

  it("decodes the HTML entities the serializer emits for < > &", () => {
    expect(unescapeComposerText("a &lt; b &amp; c &gt; d")).toBe(
      "a < b & c > d"
    )
  })

  it("round-trips a literal entity (decodes &amp; last)", () => {
    // The serializer encodes a typed `&lt;` as `&amp;lt;`; decoding `&amp;` last
    // recovers the literal `&lt;` rather than the `<` character.
    expect(unescapeComposerText("&amp;lt;")).toBe("&lt;")
  })

  it("leaves plain text untouched", () => {
    expect(unescapeComposerText("just a normal sentence.")).toBe(
      "just a normal sentence."
    )
  })

  it("unescapes an escaped backtick in prose without opening a code span", () => {
    expect(unescapeComposerText("use \\`grep\\` here")).toBe("use `grep` here")
  })

  it("leaves an inline code span verbatim (no unescaping inside)", () => {
    expect(unescapeComposerText("\\*a\\* `\\*lit\\* &amp;`")).toBe(
      "*a* `\\*lit\\* &amp;`"
    )
  })

  it("treats code-span scanning as backslash-insensitive (`` `a\\` `` stays a span)", () => {
    expect(unescapeComposerText("see `a\\` now")).toBe("see `a\\` now")
  })

  it("leaves a fenced code block verbatim while unescaping the prose around it", () => {
    const input =
      "a \\*note\\*\n```\nC:\\tools &lt;div&gt; &amp;\n```\nb \\_end\\_"
    const expected = "a *note*\n```\nC:\\tools &lt;div&gt; &amp;\n```\nb _end_"
    expect(unescapeComposerText(input)).toBe(expected)
  })

  it("keeps a fenced block's info string and inner blank lines verbatim", () => {
    const input = "```ts\nconst x = 1\n\nconst y = 2\n```"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("unescapes prose on both sides of a code block", () => {
    expect(unescapeComposerText("C:\\\\a\n```\nC:\\b\n```\nC:\\\\c")).toBe(
      "C:\\a\n```\nC:\\b\n```\nC:\\c"
    )
  })

  it("keeps an over-indented (4-space) fence line inside the code block verbatim", () => {
    const input = "```\ncode\n    ```\nC:\\\\tools\\\\x\n```"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("recognizes a CRLF closing fence and unescapes the prose after it", () => {
    expect(unescapeComposerText("```\r\ncode\r\n```\r\nC:\\\\x")).toBe(
      "```\r\ncode\r\n```\r\nC:\\x"
    )
  })

  it("keeps an inline reference link verbatim while unescaping prose around it", () => {
    // The escaped label `a\]b.ts` must survive byte-for-byte (unescaping it would
    // corrupt the reference), but the Windows path in the prose is unescaped.
    expect(
      unescapeComposerText("open [a\\]b.ts](file:///x/a]b.ts) now C:\\\\x")
    ).toBe("open [a\\]b.ts](file:///x/a]b.ts) now C:\\x")
  })

  it("keeps a reference label's escaped punctuation verbatim", () => {
    const input = "see [a\\*b\\_c\\~d.ts](file:///x/y.ts) end"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("unescapes user-typed literal brackets (escaped, not a reference link)", () => {
    expect(unescapeComposerText("a \\[not a link\\] b")).toBe(
      "a [not a link] b"
    )
  })

  it("preserves a reference link and a code span together, unescaping the rest", () => {
    expect(
      unescapeComposerText("`C:\\x` [foo.ts](file:///x/foo.ts) \\*p\\* C:\\\\y")
    ).toBe("`C:\\x` [foo.ts](file:///x/foo.ts) *p* C:\\y")
  })

  it("keeps a blockquoted fenced code block verbatim, unescaping prose after it", () => {
    const input = "> ```\n> C:\\x \\*y\\*\n> ```\n\nafter C:\\\\z"
    const expected = "> ```\n> C:\\x \\*y\\*\n> ```\n\nafter C:\\z"
    expect(unescapeComposerText(input)).toBe(expected)
  })

  it("unescapes a Windows path inside blockquoted prose (the `>` is passthrough)", () => {
    expect(unescapeComposerText("> see C:\\\\tools\\\\x.txt")).toBe(
      "> see C:\\tools\\x.txt"
    )
  })

  it("keeps a nested-blockquote fenced code block verbatim", () => {
    const input = "> > ```\n> > C:\\*.txt\n> > ```"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("keeps a nested-list (≥4-space indented) code block verbatim", () => {
    const input = "- a\n  - b\n\n    ```\n    C:\\*.txt <x>\n    ```"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("unescapes a path in a deeply-nested list item (parser disambiguates from code)", () => {
    // The case a line scanner can't get right: `    - path …` is a 3rd-level list
    // ITEM (prose, unescape), not indented code — the real parser knows the diff.
    expect(
      unescapeComposerText("- a\n  - b\n    - path C:\\\\tools\\\\x.txt")
    ).toBe("- a\n  - b\n    - path C:\\tools\\x.txt")
  })

  it("bounds a backtick-run storm via the delimiter budget (no hang)", () => {
    // Thousands of distinct-length backtick runs make the parser's code-span
    // resolution super-linear; the delimiter budget short-circuits to the raw text
    // instead of stalling the copy click (the raw text has no escapes anyway).
    const parts: string[] = []
    for (let k = 1; k <= 1500; k += 1) parts.push("`".repeat(k) + "x")
    const input = parts.join("")
    const start = Date.now()
    expect(unescapeComposerText(input)).toBe(input)
    expect(Date.now() - start).toBeLessThan(1000)
  })

  it("bounds a block-heavy (thousands of list items) message via the line budget", () => {
    // Many block markers also drive parse cost; the line budget catches it even
    // though such input has zero inline delimiters.
    const input = "- item C:\\\\x\n".repeat(8000)
    const start = Date.now()
    expect(unescapeComposerText(input)).toBe(input)
    expect(Date.now() - start).toBeLessThan(1000)
  })

  it("bounds an oversized message via the byte budget", () => {
    const input = "x word ".repeat(20000) + " C:\\\\tools\\\\x" // ~140 KB > 128 KB
    const start = Date.now()
    expect(unescapeComposerText(input)).toBe(input)
    expect(Date.now() - start).toBeLessThan(1000)
  })

  it("falls back to the raw text for a marker-dense message (safe, no corruption)", () => {
    // Over a budget the path is left escaped (the original-bug direction) rather
    // than risk a slow parse — a deliberate, safe trade for pasted storms.
    const input = "`".repeat(2100) + " C:\\\\tools\\\\x"
    expect(unescapeComposerText(input)).toBe(input)
  })

  it("still unescapes a large-but-ordinary message (under every budget)", () => {
    // A long, normal message must NOT trip the guards: 1000 short prose lines with
    // a path on each, well under the byte/line/delimiter ceilings — all unescaped.
    const input = "see C:\\\\tools\\\\x.txt here\n".repeat(1000)
    const expected = "see C:\\tools\\x.txt here\n".repeat(1000)
    expect(unescapeComposerText(input)).toBe(expected)
  })
})
