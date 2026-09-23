import { describe, expect, it } from "vitest"
import { RELATIVE_FILE_HREF_PROPERTY } from "./rehype-relative-file-links"
import { remarkRewriteFileUriLinks } from "./remark-file-uri-links"

// Minimal mdast node shapes for the transform.
type Node = {
  type: string
  url?: string
  identifier?: string
  children?: Node[]
  data?: { hProperties?: Record<string, unknown> }
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

  it("makes a bare relative file path explicitly relative", () => {
    // `C:` needs a following slash to be a drive path, so these are relative
    // paths — which harden cannot parse without a `./` in front.
    expect(rewrite("src/main.rs")).toBe("./src/main.rs")
    expect(rewrite("notes.md")).toBe("./notes.md")
    expect(rewrite("index.html#L3")).toBe("./index.html#L3")
    // `sh` is a TLD too, but here it is a script far more often than a host.
    expect(rewrite("deploy.sh")).toBe("./deploy.sh")
    expect(rewrite(".github/workflows/ci.yml")).toBe(
      "./.github/workflows/ci.yml"
    )
  })

  it("leaves domain-shaped and non-path targets alone", () => {
    expect(rewrite("www.example.com")).toBe("www.example.com")
    expect(rewrite("example.com")).toBe("example.com")
    expect(rewrite("foo.io")).toBe("foo.io")
    // A domain in the host position is a web address even with a path after it.
    expect(rewrite("github.com/foo/bar")).toBe("github.com/foo/bar")
    expect(rewrite("example.com/docs/a.md")).toBe("example.com/docs/a.md")
    expect(rewrite("README")).toBe("README")
    expect(rewrite("#section")).toBe("#section")
    expect(rewrite("mailto:a@b.c")).toBe("mailto:a@b.c")
    expect(rewrite("/abs/path.md")).toBe("/abs/path.md")
  })

  it("marks a relative link with its explicit href for the rehype restore", () => {
    for (const [url, href] of [
      ["./index.html", "./index.html"],
      ["../site/index.html", "../site/index.html"],
      ["index.html", "./index.html"],
      // An explicit `./` is a path even with a space in it (`<./my notes.md>`).
      ["./my notes.md", "./my notes.md"],
    ]) {
      const tree = linkTree(url)
      remarkRewriteFileUriLinks()(tree)
      const link = tree.children![0].children![0]
      expect(link.data?.hProperties?.[RELATIVE_FILE_HREF_PROPERTY]).toBe(href)
    }
  })

  it("does not mark absolute, file:// or web links", () => {
    for (const url of [
      "/abs/path.md",
      "file:///Users/a/b.ts",
      "https://example.com/x",
    ]) {
      const tree = linkTree(url)
      remarkRewriteFileUriLinks()(tree)
      expect(tree.children![0].children![0].data).toBeUndefined()
    }
  })

  it("marks a reference-style link through its relative definition", () => {
    const tree: Node = {
      type: "root",
      children: [
        {
          type: "paragraph",
          children: [
            {
              type: "linkReference",
              identifier: "page",
              children: [{ type: "text" }],
            },
          ],
        },
        { type: "definition", identifier: "page", url: "index.html" },
      ],
    }
    remarkRewriteFileUriLinks()(tree)
    expect(tree.children![1].url).toBe("./index.html")
    expect(
      tree.children![0].children![0].data?.hProperties?.[
        RELATIVE_FILE_HREF_PROPERTY
      ]
    ).toBe("./index.html")
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
