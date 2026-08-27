import { Editor } from "@tiptap/core"
import type { JSONContent } from "@tiptap/core"
import { describe, expect, it } from "vitest"

import { buildComposerExtensions } from "@/components/chat/composer/editor-config"

import { sanitizeComposerDraftDoc } from "./composer-draft-sanitize"

const fileRef: JSONContent = {
  type: "reference",
  attrs: {
    refType: "file",
    id: "a.ts",
    label: "a.ts",
    uri: "file:///a.ts",
    meta: { fileKind: "file" },
  },
}

/** Types present anywhere in a doc tree. */
function typesIn(doc: JSONContent): Set<string> {
  const seen = new Set<string>()
  const walk = (node: JSONContent) => {
    if (node.type) seen.add(node.type)
    for (const child of node.content ?? []) walk(child)
  }
  walk(doc)
  return seen
}

describe("sanitizeComposerDraftDoc", () => {
  it("preserves an agent rule selection atom in a v2 draft", () => {
    const doc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            {
              type: "agentRuleSelection",
              attrs: {
                version: 1,
                ruleIds: ["tests"],
                sourceHash: "hash",
                sources: [".codeg/rules/team.md"],
                exactText: "Run tests.\n",
                envelopeNonce: "nonce",
              },
            },
          ],
        },
      ],
    }

    expect(sanitizeComposerDraftDoc(doc)).toBe(doc)
  })

  it("returns a plain-schema doc untouched (identity)", () => {
    const doc: JSONContent = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "text", text: "hi " }, fileRef],
        },
      ],
    }
    expect(sanitizeComposerDraftDoc(doc)).toBe(doc)
  })

  it("converts heading/list/blockquote/codeBlock to paragraphs and strips marks", () => {
    const stale: JSONContent = {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Title" }],
        },
        {
          type: "paragraph",
          content: [
            { type: "text", marks: [{ type: "bold" }], text: "bold" },
            { type: "text", text: " x" },
          ],
        },
        {
          type: "bulletList",
          content: [
            {
              type: "listItem",
              content: [
                { type: "paragraph", content: [{ type: "text", text: "i1" }] },
              ],
            },
            {
              type: "listItem",
              content: [
                { type: "paragraph", content: [{ type: "text", text: "i2" }] },
              ],
            },
          ],
        },
      ],
    }
    const clean = sanitizeComposerDraftDoc(stale)
    // Only plain-schema node types survive.
    expect(typesIn(clean)).toEqual(new Set(["doc", "paragraph", "text"]))
    // Heading text + both list items become their own paragraphs.
    expect(clean.content).toHaveLength(4)
    // Marks are gone (the text node carries no `marks`).
    const bold = clean.content?.[1]?.content?.[0]
    expect(bold).toEqual({ type: "text", text: "bold" })
  })

  it("preserves reference badges while flattening around them", () => {
    const stale: JSONContent = {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 2 },
          content: [{ type: "text", text: "see " }, fileRef],
        },
      ],
    }
    const clean = sanitizeComposerDraftDoc(stale)
    expect(typesIn(clean).has("reference")).toBe(true)
    expect(typesIn(clean).has("heading")).toBe(false)
  })

  it("never yields an empty doc", () => {
    const clean = sanitizeComposerDraftDoc({ type: "doc", content: [] })
    expect(clean.content?.length).toBeGreaterThan(0)
  })

  it("strips a stale mark on a hardBreak or reference (not just on text)", () => {
    // A paragraph whose ONLY offenders are marks on a hardBreak and a badge —
    // node types are all plain-schema, so this must NOT be mistaken for an
    // already-clean doc (which would skip sanitizing and later wipe the draft).
    const stale: JSONContent = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "a" },
            { type: "hardBreak", marks: [{ type: "bold" }] },
            { type: "text", text: "b " },
            { ...fileRef, marks: [{ type: "bold" }] },
          ],
        },
      ],
    }
    const clean = sanitizeComposerDraftDoc(stale)
    // It was re-built (not returned by identity)…
    expect(clean).not.toBe(stale)
    // …and no inline node carries marks anymore.
    const inline = clean.content?.[0]?.content ?? []
    expect(inline.every((n) => !n.marks || n.marks.length === 0)).toBe(true)
    // The badge and both text runs and the break are all preserved.
    expect(typesIn(clean)).toEqual(
      new Set(["doc", "paragraph", "text", "hardBreak", "reference"])
    )
  })

  it("survives setContent when a hardBreak/reference carried a stale mark", () => {
    // Without the fix this exact doc silently wipes to an empty document.
    const stale: JSONContent = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "keep one" },
            { type: "hardBreak", marks: [{ type: "bold" }] },
            { type: "text", text: "keep two " },
            { ...fileRef, marks: [{ type: "bold" }] },
          ],
        },
      ],
    }
    const editor = new Editor({ extensions: buildComposerExtensions({}) })
    try {
      editor.commands.setContent(sanitizeComposerDraftDoc(stale))
      expect(editor.getText()).toContain("keep one")
      expect(editor.getText()).toContain("keep two")
      let hasReference = false
      editor.state.doc.descendants((node) => {
        if (node.type.name === "reference") hasReference = true
      })
      expect(hasReference).toBe(true)
    } finally {
      editor.destroy()
    }
  })

  // The whole point: a sanitized stale doc must survive setContent under the
  // real plain-text schema WITHOUT being discarded.
  it("survives setContent — text and badge are kept (not dropped)", () => {
    const stale: JSONContent = {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { level: 1 },
          content: [{ type: "text", text: "Title" }],
        },
        {
          type: "paragraph",
          content: [
            { type: "text", marks: [{ type: "bold" }], text: "keep me " },
            fileRef,
          ],
        },
      ],
    }
    const editor = new Editor({ extensions: buildComposerExtensions({}) })
    try {
      editor.commands.setContent(sanitizeComposerDraftDoc(stale))
      expect(editor.getText()).toContain("Title")
      expect(editor.getText()).toContain("keep me")
      let hasReference = false
      editor.state.doc.descendants((node) => {
        if (node.type.name === "reference") hasReference = true
      })
      expect(hasReference).toBe(true)
    } finally {
      editor.destroy()
    }
  })
})
