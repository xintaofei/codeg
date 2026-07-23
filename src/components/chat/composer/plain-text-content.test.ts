import { describe, expect, it } from "vitest"

import {
  decidePastedPlainText,
  textToInlineContent,
} from "./plain-text-content"

describe("decidePastedPlainText", () => {
  it("inserts text/plain when the clipboard carries an external HTML fragment", () => {
    // What a browser puts on the clipboard when a URL is copied from the address
    // bar: the anchor text is the page <title>, not the URL.
    const decision = decidePastedPlainText({
      html: '<a href="https://github.com/">GitHub · Change is constant. GitHub keeps you ahead. · GitHub</a>',
      text: "https://github.com/",
    })
    expect(decision).toEqual(textToInlineContent("https://github.com/"))
    // Specifically the URL, never the page title.
    expect(decision).toEqual([{ type: "text", text: "https://github.com/" }])
  })

  it("defers to ProseMirror for a pure text/plain clipboard (no HTML)", () => {
    // Nothing can mislead the schema without an HTML flavor, so keep the native
    // paste path unchanged.
    expect(
      decidePastedPlainText({ html: "", text: "https://github.com/" })
    ).toBeNull()
  })

  it("defers to ProseMirror for HTML copied from within the editor (data-pm-slice)", () => {
    // ProseMirror's serializeForClipboard tags native copies with data-pm-slice.
    // Forcing text/plain here would corrupt structure: two paragraphs come across
    // as "one\n\ntwo" (a blank line the composer never had), and a hard break
    // comes across as "" (the line break lost). The native HTML round-trip is
    // exact, so we must defer.
    expect(
      decidePastedPlainText({
        html: '<p data-pm-slice="0 0 []">one</p><p>two</p>',
        text: "one\n\ntwo",
      })
    ).toBeNull()
  })

  it("defers to ProseMirror when the HTML carries our reference badges", () => {
    // Defensive fallback: a badge fragment without the slice wrapper must still
    // round-trip via the HTML parser rather than collapse to its plain-text token.
    expect(
      decidePastedPlainText({
        html: '<span data-reference data-ref-type="file" data-ref-id="lib.rs" data-label="lib.rs">lib.rs</span>',
        text: "lib.rs",
      })
    ).toBeNull()
  })

  it("defers when an external HTML fragment has no text/plain flavor", () => {
    expect(
      decidePastedPlainText({ html: '<img src="x.png">', text: "" })
    ).toBeNull()
  })

  it("maps newlines in the pasted text to hard breaks", () => {
    const decision = decidePastedPlainText({
      html: "<div>one<br>two</div>",
      text: "one\ntwo",
    })
    expect(decision).toEqual([
      { type: "text", text: "one" },
      { type: "hardBreak" },
      { type: "text", text: "two" },
    ])
  })
})
