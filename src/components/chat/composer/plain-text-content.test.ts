import { describe, expect, it } from "vitest"

import {
  decidePastedContent,
  textToHydratedInlineContent,
  textToInlineContent,
  textToSeededDoc,
  textToSeededInlineContent,
} from "./plain-text-content"

describe("decidePastedContent", () => {
  it("inserts text/plain when the clipboard carries an external HTML fragment", () => {
    // What a browser puts on the clipboard when a URL is copied from the address
    // bar: the anchor text is the page <title>, not the URL.
    const decision = decidePastedContent({
      html: '<a href="https://github.com/">GitHub · Change is constant. GitHub keeps you ahead. · GitHub</a>',
      text: "https://github.com/",
    })
    expect(decision).toEqual(textToInlineContent("https://github.com/"))
    // Specifically the URL, never the page title.
    expect(decision).toEqual([{ type: "text", text: "https://github.com/" }])
  })

  it("defers to ProseMirror for a reference-free text/plain clipboard", () => {
    // Nothing to hydrate and no HTML flavor that could mislead the schema, so
    // keep the native paste path (multi-line paragraph handling) unchanged.
    expect(
      decidePastedContent({ html: "", text: "https://github.com/" })
    ).toBeNull()
    expect(decidePastedContent({ html: "", text: "plain\nprose" })).toBeNull()
  })

  it("hydrates serialized references in a pure text/plain paste into badges", () => {
    // Pasting the wire form of a sent message (what the transcript renders as
    // badges) must preview the same badges in the composer instead of literal
    // `[label](uri)` text.
    const decision = decidePastedContent({
      html: "",
      text: "see [app.ts](file:///repo/app.ts) now",
    })
    expect(decision).toEqual([
      { type: "text", text: "see " },
      {
        type: "reference",
        attrs: expect.objectContaining({
          refType: "file",
          label: "app.ts",
          uri: "file:///repo/app.ts",
        }),
      },
      { type: "text", text: " now" },
    ])
  })

  it("hydrates references when forcing text/plain over an external HTML fragment", () => {
    const decision = decidePastedContent({
      html: "<div>run [@Codex](codeg://agent/codex)</div>",
      text: "run [@Codex](codeg://agent/codex)",
    })
    expect(decision).toEqual([
      { type: "text", text: "run " },
      {
        type: "reference",
        attrs: expect.objectContaining({ refType: "agent", label: "Codex" }),
      },
    ])
  })

  it("hydrates a pasted agent link into a real agent badge", () => {
    // The badge a paste produces is indistinguishable from one the `@` panel
    // inserted, which is what makes a copied delegation message re-send with
    // its routing reminder intact.
    const content = textToHydratedInlineContent(
      "ask [@Antigravity](codeg://agent/antigravity)"
    )
    const agent = content?.find((node) => node.type === "reference")
    expect(agent).toMatchObject({
      attrs: {
        refType: "agent",
        id: "antigravity",
        meta: { agentType: "antigravity" },
      },
    })
  })

  it("defers to ProseMirror for HTML copied from within the editor (data-pm-slice)", () => {
    // ProseMirror's serializeForClipboard tags native copies with data-pm-slice.
    // Forcing text/plain here would corrupt structure: two paragraphs come across
    // as "one\n\ntwo" (a blank line the composer never had), and a hard break
    // comes across as "" (the line break lost). The native HTML round-trip is
    // exact, so we must defer.
    expect(
      decidePastedContent({
        html: '<p data-pm-slice="0 0 []">one</p><p>two</p>',
        text: "one\n\ntwo",
      })
    ).toBeNull()
  })

  it("defers to ProseMirror when the HTML carries our reference badges", () => {
    // Defensive fallback: a badge fragment without the slice wrapper must still
    // round-trip via the HTML parser rather than collapse to its plain-text
    // token — even though that token would hydrate.
    expect(
      decidePastedContent({
        html: '<span data-reference data-ref-type="file" data-ref-id="lib.rs" data-label="lib.rs">[lib.rs](file:///repo/lib.rs)</span>',
        text: "[lib.rs](file:///repo/lib.rs)",
      })
    ).toBeNull()
  })

  it("defers when an external HTML fragment has no text/plain flavor", () => {
    expect(
      decidePastedContent({ html: '<img src="x.png">', text: "" })
    ).toBeNull()
  })

  it("maps newlines in the pasted text to hard breaks", () => {
    const decision = decidePastedContent({
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

describe("textToSeededInlineContent", () => {
  it("hydrates the references a quick message carries into badges", () => {
    // A quick message is stored in the same wire format a sent message uses, so
    // filling one into the composer must show badges, not `[label](uri)` text.
    expect(
      textToSeededInlineContent(
        "/review [app.ts](file:///repo/app.ts) with [@Codex](codeg://agent/codex)"
      )
    ).toEqual([
      {
        type: "reference",
        attrs: expect.objectContaining({ refType: "skill", id: "review" }),
      },
      { type: "text", text: " " },
      {
        type: "reference",
        attrs: expect.objectContaining({
          refType: "file",
          label: "app.ts",
          uri: "file:///repo/app.ts",
        }),
      },
      { type: "text", text: " with " },
      {
        type: "reference",
        attrs: expect.objectContaining({ refType: "agent", label: "Codex" }),
      },
    ])
  })

  it("hydrates a session link", () => {
    expect(
      textToSeededInlineContent("续上次 [排查登录](codeg://session/42)")
    ).toEqual([
      { type: "text", text: "续上次 " },
      {
        type: "reference",
        attrs: expect.objectContaining({
          refType: "session",
          id: "42",
          label: "排查登录",
        }),
      },
    ])
  })

  it("falls back to literal text (with hard breaks) when nothing hydrates", () => {
    expect(textToSeededInlineContent("plain\nprose")).toEqual(
      textToInlineContent("plain\nprose")
    )
    expect(textToSeededInlineContent("")).toEqual([])
  })

  it("keeps an embedded-attachment link literal (it would be dropped on send)", () => {
    expect(
      textToSeededInlineContent("see [report.pdf](codeg://embedded/abc-123)")
    ).toEqual(textToInlineContent("see [report.pdf](codeg://embedded/abc-123)"))
  })
})

describe("textToSeededDoc", () => {
  it("wraps the seeded inline content in a single paragraph", () => {
    const text = "看 [app.ts](file:///repo/app.ts)"
    expect(textToSeededDoc(text)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", content: textToSeededInlineContent(text) },
      ],
    })
  })

  it("keeps an empty paragraph for empty text", () => {
    expect(textToSeededDoc("")).toEqual({
      type: "doc",
      content: [{ type: "paragraph", content: [] }],
    })
  })
})

describe("textToHydratedInlineContent", () => {
  it("returns null when the text contains no reference", () => {
    expect(textToHydratedInlineContent("")).toBeNull()
    expect(textToHydratedInlineContent("plain prose")).toBeNull()
    // A non-reference markdown link is not a badge (stays literal).
    expect(
      textToHydratedInlineContent("[docs](https://example.com)")
    ).toBeNull()
    // Path-like tokens don't hydrate (same heuristic as the transcript).
    expect(textToHydratedInlineContent("see /usr/bin for it")).toBeNull()
  })

  it("hydrates bare `/cmd` and `$skill` tokens, keeping their trigger", () => {
    const content = textToHydratedInlineContent("$deploy prod")
    expect(content).toEqual([
      {
        type: "reference",
        attrs: expect.objectContaining({
          refType: "skill",
          id: "deploy",
          label: "deploy",
          meta: { invocationPrefix: "$" },
        }),
      },
      { type: "text", text: " prod" },
    ])
    expect(textToHydratedInlineContent("/review it")?.[0]).toEqual({
      type: "reference",
      attrs: expect.objectContaining({
        refType: "skill",
        id: "review",
        meta: { invocationPrefix: "/" },
      }),
    })
  })

  it("hydrates session links and maps newlines around badges to hard breaks", () => {
    expect(
      textToHydratedInlineContent("re:\n[My chat](codeg://session/42)")
    ).toEqual([
      { type: "text", text: "re:" },
      { type: "hardBreak" },
      {
        type: "reference",
        attrs: expect.objectContaining({
          refType: "session",
          id: "42",
          label: "My chat",
          uri: "codeg://session/42",
        }),
      },
    ])
  })

  it("keeps an embedded-attachment link literal (it would be dropped on send)", () => {
    // The synthetic display link of a path-less attachment has no bytes behind
    // it here, and send serialization omits embedded badges — hydrating one
    // would silently delete the pasted text on send.
    expect(
      textToHydratedInlineContent("see [report.pdf](codeg://embedded/abc-123)")
    ).toBeNull()
    // Alongside a real reference the rest still hydrates; the embedded link
    // stays text.
    const mixed = textToHydratedInlineContent(
      "[report.pdf](codeg://embedded/abc-123) vs [app.ts](file:///repo/app.ts)"
    )
    expect(mixed?.[0]).toEqual({
      type: "text",
      text: "[report.pdf](codeg://embedded/abc-123)",
    })
    expect(mixed?.[2]).toEqual({
      type: "reference",
      attrs: expect.objectContaining({ refType: "file", label: "app.ts" }),
    })
  })
})
