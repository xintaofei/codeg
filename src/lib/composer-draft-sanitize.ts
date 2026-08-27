import type { JSONContent } from "@tiptap/core"

/**
 * Down-migrate a persisted v2 composer draft (a Tiptap JSON doc) to the
 * plain-text composer schema. A draft saved BEFORE the composer became
 * plain-text can hold `heading` / `bulletList` / `codeBlock` / `blockquote`
 * nodes and `bold`/`italic`/`link`/… marks that the narrowed schema no longer
 * defines. Handing such a doc to `editor.commands.setContent` does NOT throw —
 * it silently discards the ENTIRE document (verified against Tiptap 3.26),
 * losing the user's words AND their reference badges. This flattens the doc to
 * valid nodes first so nothing is lost but the formatting:
 *
 * - `heading` / `codeBlock` → a `paragraph` (their inline text kept).
 * - `blockquote` / `bulletList` / `orderedList` / `listItem` / task lists →
 *   their inner block(s), so a list becomes one paragraph per item.
 * - marks (`bold`, `italic`, `code`, `link`, …) are stripped; the text stays.
 * - `paragraph` / `text` / `hardBreak` / `reference` (badges) are preserved.
 *
 * A doc already in the plain-text schema (the common case for drafts saved after
 * this change) is returned untouched, so structure and object identity are
 * preserved and this is idempotent.
 */

// Inline leaf types the plain-text schema keeps verbatim (see editor-config.ts).
// `text` additionally has its marks stripped.
const KEPT_INLINE = new Set([
  "text",
  "hardBreak",
  "reference",
  "agentRuleSelection",
])

// Block types whose children are INLINE (fold into one paragraph) vs BLOCK
// (recurse). Unknown types are classified by inspecting their children.
const INLINE_CONTENT_BLOCK = new Set(["paragraph", "heading", "codeBlock"])
const BLOCK_CONTENT_BLOCK = new Set([
  "blockquote",
  "bulletList",
  "orderedList",
  "listItem",
  "taskList",
  "taskItem",
])

function isBlockType(type: string | undefined): boolean {
  return (
    !!type && (INLINE_CONTENT_BLOCK.has(type) || BLOCK_CONTENT_BLOCK.has(type))
  )
}

/** Collect valid inline nodes (text without marks, hard breaks, references),
 *  salvaging leaves nested under any unknown inline wrapper. */
function sanitizeInline(nodes: JSONContent[] | undefined): JSONContent[] {
  const out: JSONContent[] = []
  for (const node of nodes ?? []) {
    const type = node?.type
    if (type === "text") {
      if (typeof node.text === "string" && node.text.length > 0) {
        out.push({ type: "text", text: node.text })
      }
    } else if (type === "hardBreak") {
      out.push({ type: "hardBreak" })
    } else if (type === "reference") {
      out.push({ type: "reference", attrs: node.attrs })
    } else if (type === "agentRuleSelection") {
      out.push({ type: "agentRuleSelection", attrs: node.attrs })
    } else if (Array.isArray(node?.content)) {
      out.push(...sanitizeInline(node.content))
    }
  }
  return out
}

/** Flatten block nodes into the allowed `paragraph`-only block level. */
function sanitizeBlocks(
  nodes: JSONContent[] | undefined,
  out: JSONContent[]
): void {
  for (const node of nodes ?? []) {
    const type = node?.type
    if (type && INLINE_CONTENT_BLOCK.has(type)) {
      out.push({ type: "paragraph", content: sanitizeInline(node.content) })
    } else if (type && BLOCK_CONTENT_BLOCK.has(type)) {
      sanitizeBlocks(node.content, out)
    } else if (Array.isArray(node?.content)) {
      // Unknown node: recurse when it wraps blocks, else treat its content as a
      // paragraph's inline. (A childless unknown node contributes nothing.)
      if (node.content.some((child) => isBlockType(child?.type))) {
        sanitizeBlocks(node.content, out)
      } else {
        out.push({ type: "paragraph", content: sanitizeInline(node.content) })
      }
    }
  }
}

/** Whether `doc` already fits the plain-text schema exactly (at least one
 *  paragraph, only text/hardBreak/reference inline, no marks on ANY of them) —
 *  returned untouched by the sanitizer. Empty/missing content falls through so
 *  the sanitizer guarantees a non-empty doc for `setContent`.
 *
 *  Marks must be rejected on every inline kind, not just `text`: a persisted
 *  draft can carry a `hardBreak` or `reference` bearing a stale `bold`/`link`
 *  mark (e.g. a Shift+Enter break typed inside bold text, or a badge selected
 *  and emphasized in the old rich composer). Those mark types no longer exist,
 *  and handing such a node to `setContent` silently discards the ENTIRE document
 *  (verified) — the same catastrophe as an unknown node type. */
function isPlainSchemaDoc(doc: JSONContent): boolean {
  if (!Array.isArray(doc.content) || doc.content.length === 0) return false
  for (const block of doc.content) {
    if (block?.type !== "paragraph") return false
    for (const inline of block.content ?? []) {
      if (!KEPT_INLINE.has(inline?.type ?? "")) return false
      if (Array.isArray(inline.marks) && inline.marks.length > 0) return false
    }
  }
  return true
}

export function sanitizeComposerDraftDoc(doc: JSONContent): JSONContent {
  if (isPlainSchemaDoc(doc)) return doc
  const content: JSONContent[] = []
  sanitizeBlocks(doc.content, content)
  // setContent needs at least one block; never hand back an empty doc.
  return {
    type: "doc",
    content: content.length > 0 ? content : [{ type: "paragraph" }],
  }
}
