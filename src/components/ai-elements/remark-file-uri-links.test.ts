import { describe, expect, it } from "vitest"
import { remarkRewriteFileUriLinks } from "./remark-file-uri-links"

// Minimal mdast node shapes for the transform.
type Node = {
  type: string
  url?: string
  identifier?: string
  children?: Node[]
}

function linkTree(url: string): Node {
  return {
    type: "root",
    children: [
      {
        type: "paragraph",
        children: [{ type: "link", url, children: [{ type: "text" }] }],
      },
    ],
  }
}

function firstLinkUrl(tree: Node): string | undefined {
  let found: string | undefined
  const walk = (n: Node) => {
    if (n.type === "link") found = n.url
    n.children?.forEach(walk)
  }
  walk(tree)
  return found
}

function rewrite(url: string): string | undefined {
  const tree = linkTree(url)
  remarkRewriteFileUriLinks()(tree)
  return firstLinkUrl(tree)
}

describe("remarkRewriteFileUriLinks", () => {
  it("rewrites a POSIX file:// URI to a bare local path", () => {
    expect(rewrite("file:///Users/a/b.ts")).toBe("/Users/a/b.ts")
  })

  it("keeps the leading slash before a Windows drive letter (sanitize-safe)", () => {
    // A bare `C:/…` would make rehype-sanitize read `C:` as a URL protocol and
    // strip the href (→ harden's "[blocked]"); `/C:/…` keeps `C:` out of
    // protocol position. Downstream link-safety strips the slash before opening.
    expect(rewrite("file:///C:/x/y.ts")).toBe("/C:/x/y.ts")
  })

  it("prefixes a slash onto a bare Windows drive path (forward slashes)", () => {
    expect(rewrite("E:/Desktop/docs/G.docx")).toBe("/E:/Desktop/docs/G.docx")
  })

  it("prefixes a slash onto a bare Windows drive path (backslashes)", () => {
    expect(rewrite("C:\\Users\\a\\b.docx")).toBe("/C:\\Users\\a\\b.docx")
  })

  it("prefixes a slash onto a Chinese/encoded bare Windows drive path", () => {
    expect(rewrite("E:/桌面/使用手册/G手册.docx")).toBe(
      "/E:/桌面/使用手册/G手册.docx"
    )
    expect(rewrite("E:/My%20Docs/%E6%89%8B%E5%86%8C.docx")).toBe(
      "/E:/My%20Docs/%E6%89%8B%E5%86%8C.docx"
    )
  })

  it("leaves a bare relative path untouched (not a drive path)", () => {
    // `C:` needs a following slash to be a drive path; `src/main.rs` and a
    // schemeless relative path stay as-is (not openable — existing behavior).
    expect(rewrite("src/main.rs")).toBe("src/main.rs")
    expect(rewrite("notes.md")).toBe("notes.md")
  })

  it("emits a UNC file:// URI as a backslash UNC path (unambiguously local)", () => {
    // //server/share would be indistinguishable from a protocol-relative
    // web url downstream; the backslash form tags it as a local file.
    expect(rewrite("file://server/share/doc.md")).toBe(
      "\\\\server\\share\\doc.md"
    )
  })

  it("preserves fragments on rewritten links", () => {
    expect(rewrite("file:///Users/a/b.ts#L12")).toBe("/Users/a/b.ts#L12")
  })

  it("leaves non-file URLs untouched", () => {
    expect(rewrite("https://example.com/x")).toBe("https://example.com/x")
  })
})
