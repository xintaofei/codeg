import { describe, it, expect } from "vitest"
import {
  inlineHtmlResources,
  withSandboxCsp,
  resolveAbsPath,
  extractHtmlTitle,
  type Base64Reader,
} from "./html-preview-inline"

const folderPath = "/proj"
const fileDir = "/proj/docs"

// Reader backed by plain-text files (base64-encoded on read); optionally
// records the absolute paths it was asked for.
function makeReader(
  files: Record<string, string>,
  calls?: string[]
): Base64Reader {
  return async (absPath) => {
    if (calls) calls.push(absPath)
    if (absPath in files) return btoa(files[absPath])
    throw new Error("not found: " + absPath)
  }
}

const inline = (html: string, files: Record<string, string>) =>
  inlineHtmlResources(html, {
    fileDir,
    folderPath,
    readFileBase64: makeReader(files),
  })

describe("resolveAbsPath", () => {
  it("resolves . and ..", () => {
    expect(resolveAbsPath("/proj/docs", "./a.png")).toBe("/proj/docs/a.png")
    expect(resolveAbsPath("/proj/docs", "../img/a.png")).toBe("/proj/img/a.png")
    expect(resolveAbsPath("/proj", "/x/y")).toBe("/proj/x/y")
  })
})

describe("extractHtmlTitle", () => {
  it("extracts the document title", () => {
    expect(
      extractHtmlTitle(
        `<html><head><title>Hello</title></head><body></body></html>`
      )
    ).toBe("Hello")
  })

  it("ignores attributes on the title tag and decodes entities", () => {
    expect(extractHtmlTitle(`<title lang="en">A &amp; B &lt;3</title>`)).toBe(
      "A & B <3"
    )
  })

  it("collapses and trims whitespace", () => {
    expect(extractHtmlTitle(`<title>  Multi\n  line   title  </title>`)).toBe(
      "Multi line title"
    )
  })

  it("returns an empty string when there is no title", () => {
    expect(extractHtmlTitle(`<html><body><p>x</p></body></html>`)).toBe("")
  })

  it("takes the first title and treats its content as RCDATA", () => {
    expect(extractHtmlTitle(`<title>a < b</title><title>two</title>`)).toBe(
      "a < b"
    )
  })

  it("ignores a <title> inside a comment", () => {
    expect(
      extractHtmlTitle(`<!-- <title>fake</title> --><title>real</title>`)
    ).toBe("real")
  })

  it("ignores a <title> inside a <script> string", () => {
    expect(
      extractHtmlTitle(
        `<script>var s = "<title>fake</title>"</script><title>real</title>`
      )
    ).toBe("real")
  })

  it("ignores a <title> inside <template> content", () => {
    expect(
      extractHtmlTitle(
        `<template><title>fake</title></template><title>real</title>`
      )
    ).toBe("real")
  })
})

describe("inlineHtmlResources", () => {
  it("inlines relative <img> but leaves http(s) and data: refs", async () => {
    const out = await inline(
      `<img src="img/a.png"><img src="https://x/y.png"><img src="data:image/png;base64,AAAA">`,
      { "/proj/docs/img/a.png": "PNGBYTES" }
    )
    expect(out).toContain(`data:image/png;base64,${btoa("PNGBYTES")}`)
    expect(out).toContain("https://x/y.png")
    expect(out).toContain("data:image/png;base64,AAAA")
  })

  it("resolves root-relative refs against folderPath", async () => {
    const out = await inline(`<img src="/assets/logo.png">`, {
      "/proj/assets/logo.png": "LOGO",
    })
    expect(out).toContain(`data:image/png;base64,${btoa("LOGO")}`)
  })

  it("does not read or inline resources outside the workspace root", async () => {
    const calls: string[] = []
    const out = await inlineHtmlResources(
      `<img src="../../etc/secret"><img src="ok.png">`,
      {
        fileDir,
        folderPath,
        readFileBase64: makeReader(
          { "/etc/secret": "SECRET", "/proj/docs/ok.png": "OK" },
          calls
        ),
      }
    )
    expect(out).not.toContain(btoa("SECRET"))
    expect(out).toContain(`data:image/png;base64,${btoa("OK")}`)
    expect(calls).toContain("/proj/docs/ok.png")
    expect(calls.some((c) => c.includes("secret"))).toBe(false)
  })

  it("reads each distinct resource only once", async () => {
    const calls: string[] = []
    await inlineHtmlResources(
      `<img src="a.png"><img src="a.png"><img src="a.png">`,
      {
        fileDir,
        folderPath,
        readFileBase64: makeReader({ "/proj/docs/a.png": "A" }, calls),
      }
    )
    expect(calls.filter((c) => c === "/proj/docs/a.png").length).toBe(1)
  })

  it("inlines relative srcset entries, keeping descriptors and external refs", async () => {
    const out = await inline(
      `<img srcset="a.png 1x, b.png 2x, https://x/c.png 3x">`,
      { "/proj/docs/a.png": "A", "/proj/docs/b.png": "B" }
    )
    expect(out).toContain(`data:image/png;base64,${btoa("A")} 1x`)
    expect(out).toContain(`data:image/png;base64,${btoa("B")} 2x`)
    expect(out).toContain("https://x/c.png 3x")
  })

  it("turns <link rel=stylesheet> into <style> and inlines url()", async () => {
    const out = await inline(`<link rel="stylesheet" href="styles.css">`, {
      "/proj/docs/styles.css": `body{background:url("bg.png")}`,
      "/proj/docs/bg.png": "BGBYTES",
    })
    expect(out).not.toContain("<link")
    expect(out).toContain("<style")
    expect(out).toContain(`url("data:image/png;base64,${btoa("BGBYTES")}")`)
  })

  it("skips url() that escapes the workspace root", async () => {
    const out = await inline(
      `<style>body{background:url("../../etc/x.png")}</style>`,
      {
        "/etc/x.png": "X",
      }
    )
    expect(out).not.toContain(btoa("X"))
    expect(out).toContain("../../etc/x.png")
  })

  it("inlines @import recursively and survives import cycles", async () => {
    const out = await inline(`<link rel="stylesheet" href="main.css">`, {
      "/proj/docs/main.css": `@import "a.css"; .x{}`,
      "/proj/docs/a.css": `@import "main.css"; .y{background:url(f.png)}`,
      "/proj/docs/f.png": "FBYTES",
    })
    expect(out).toContain(".x{}")
    expect(out).toContain(".y{")
    expect(out).toContain(`data:image/png;base64,${btoa("FBYTES")}`)
  })

  it("inlines <script src> as text and neutralizes nested </script>", async () => {
    const out = await inline(`<script src="app.js"></script>`, {
      "/proj/docs/app.js": `console.log("</script>")`,
    })
    expect(out).not.toContain("src=")
    expect(out).toContain("console.log")
    expect(out).toContain("<\\/script")
  })

  it("strips author-supplied CSP meta", async () => {
    const out = await inline(
      `<head><meta http-equiv="Content-Security-Policy" content="default-src *"></head><body></body>`,
      {}
    )
    expect(out).not.toContain("default-src *")
  })

  it("does not throw on malformed input or unreadable resources", async () => {
    const out = await inlineHtmlResources(`<<<x>>> <img src="missing.png">`, {
      fileDir,
      folderPath,
      readFileBase64: async () => {
        throw new Error("nope")
      },
    })
    expect(typeof out).toBe("string")
    expect(out).toContain("missing.png")
  })

  it("rejects Windows backslash traversal out of the workspace", async () => {
    const calls: string[] = []
    const out = await inlineHtmlResources(
      `<img src="..\\..\\outside\\secret.png">`,
      { fileDir, folderPath, readFileBase64: makeReader({}, calls) }
    )
    expect(calls.length).toBe(0)
    expect(out).toContain("outside")
  })

  it("stops inlining once the byte budget is exhausted", async () => {
    const out = await inlineHtmlResources(
      `<img src="a.png"><img src="b.png"><img src="c.png">`,
      {
        fileDir,
        folderPath,
        readFileBase64: makeReader({
          "/proj/docs/a.png": "AAAA",
          "/proj/docs/b.png": "AAAA",
          "/proj/docs/c.png": "AAAA",
        }),
        maxInlineBytes: btoa("AAAA").length, // room for exactly one
        concurrency: 1,
      }
    )
    const dataUris = out.match(/data:image\/png;base64,/g) ?? []
    expect(dataUris.length).toBe(1)
    expect(out).toContain('src="b.png"')
    expect(out).toContain('src="c.png"')
  })

  it("bounds total reads when resources do not fit the budget", async () => {
    const calls: string[] = []
    const files: Record<string, string> = {}
    for (let i = 0; i < 5; i++) files[`/proj/docs/big${i}.png`] = "AAAAAAAA"
    const html = Array.from(
      { length: 5 },
      (_, i) => `<img src="big${i}.png">`
    ).join("")
    const out = await inlineHtmlResources(html, {
      fileDir,
      folderPath,
      readFileBase64: makeReader(files, calls),
      maxInlineBytes: 4, // smaller than a single resource
      concurrency: 1,
    })
    // The first read already spends the read budget, so nothing else is read,
    // and nothing fits the embed budget.
    expect(calls.length).toBe(1)
    expect(out).not.toContain("data:image/png;base64,")
  })

  it("bounds reads across srcset fan-out", async () => {
    const calls: string[] = []
    const files: Record<string, string> = {}
    const entries: string[] = []
    for (let i = 0; i < 10; i++) {
      files[`/proj/docs/s${i}.png`] = "AAAAAAAA"
      entries.push(`s${i}.png ${i + 1}x`)
    }
    await inlineHtmlResources(`<img srcset="${entries.join(", ")}">`, {
      fileDir,
      folderPath,
      readFileBase64: makeReader(files, calls),
      maxInlineBytes: 4,
      concurrency: 1,
    })
    expect(calls.length).toBe(1)
  })

  it("bounds reads across css url() fan-out", async () => {
    const calls: string[] = []
    const files: Record<string, string> = {}
    let css = ""
    for (let i = 0; i < 10; i++) {
      files[`/proj/docs/u${i}.png`] = "AAAAAAAA"
      css += `.c${i}{background:url(u${i}.png)}`
    }
    await inlineHtmlResources(`<style>${css}</style>`, {
      fileDir,
      folderPath,
      readFileBase64: makeReader(files, calls),
      maxInlineBytes: 4,
      concurrency: 1,
    })
    expect(calls.length).toBe(1)
  })

  it("does not split srcset data: URLs on their internal comma", async () => {
    const out = await inline(
      `<img srcset="a.png 1x, data:image/png;base64,AAA 2x">`,
      { "/proj/docs/a.png": "A" }
    )
    expect(out).toContain(`data:image/png;base64,${btoa("A")} 1x`)
    expect(out).toContain("data:image/png;base64,AAA 2x")
  })

  it("preserves <html>/<body> wrapper attributes", async () => {
    const out = await inline(
      `<html lang="en"><head></head><body class="preview" data-theme="dark"><p>x</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.documentElement.getAttribute("lang")).toBe("en")
    expect(doc.body.getAttribute("class")).toBe("preview")
    expect(doc.body.getAttribute("data-theme")).toBe("dark")
  })

  it("ignores commented-out / string-literal wrapper tags when extracting attrs", async () => {
    const out = await inline(
      `<!-- <body class="wrong" onload="evil()"> -->` +
        `<html lang="en"><head><script>var s = "<body class=fake>"</script></head>` +
        `<body class="preview" data-theme="dark"></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.body.getAttribute("class")).toBe("preview")
    expect(doc.body.getAttribute("data-theme")).toBe("dark")
    expect(doc.body.hasAttribute("onload")).toBe(false)
  })

  it("does not truncate wrapper attrs containing '>' in a quoted value", async () => {
    const out = await inline(
      `<html><body data-x="a>b" class="c"></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.body.getAttribute("data-x")).toBe("a>b")
    expect(doc.body.getAttribute("class")).toBe("c")
  })

  it("keeps head metadata in <head> and content in <body>", async () => {
    const out = await inline(
      `<html><head><meta name="viewport" content="w=1"><title>T</title></head>` +
        `<body class="x"><p>hi</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.head.querySelector('meta[name="viewport"]')).toBeTruthy()
    expect(doc.head.querySelector("title")?.textContent).toBe("T")
    expect(doc.body.querySelector("p")?.textContent).toBe("hi")
    expect(doc.body.getAttribute("class")).toBe("x")
  })

  it("keeps body-leading <script> in <body>, not hoisted to <head>", async () => {
    const out = await inline(
      `<html><head><title>T</title></head>` +
        `<body><script>window.x = 1</script><p>hi</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.head.querySelector("title")?.textContent).toBe("T")
    expect(doc.body.querySelector("script")?.textContent).toBe("window.x = 1")
    expect(doc.head.querySelector("script")).toBeNull()
    expect(doc.body.querySelector("p")?.textContent).toBe("hi")
  })

  it("ignores wrapper tags inside RCDATA (<title>) content", async () => {
    const out = await inline(
      `<html><head><title><body class="fake"></title></head>` +
        `<body class="real"><p>x</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.body.getAttribute("class")).toBe("real")
  })

  it("ignores wrapper tags inside <template> content", async () => {
    const out = await inline(
      `<html><head><template><body class="fake"></template></head>` +
        `<body class="real"><p>x</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.body.getAttribute("class")).toBe("real")
  })

  it("does not end a <template> skip at a </template> inside a comment", async () => {
    const out = await inline(
      `<html><head><template><!-- </template> --><body class="fake"></body>` +
        `</template></head><body class="real"><p>x</p></body></html>`,
      {}
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.body.getAttribute("class")).toBe("real")
  })
})

describe("withSandboxCsp", () => {
  const base = "<html><head></head><body>x</body></html>"

  it("blocks scripts and network by default", () => {
    const out = withSandboxCsp(base)
    expect(out).toContain('http-equiv="Content-Security-Policy"')
    expect(out).toContain("default-src 'none'")
    expect(out).toContain("script-src 'none'")
    expect(out).not.toContain("connect-src")
  })

  it("allows scripts and network when trusted", () => {
    const out = withSandboxCsp(base, { trusted: true })
    expect(out).toContain("script-src 'unsafe-inline'")
    expect(out).toContain("connect-src https: http:")
  })

  it("inserts the CSP meta as the first head child", () => {
    const out = withSandboxCsp(base)
    expect(out).toMatch(/<head>\s*<meta http-equiv="Content-Security-Policy"/)
  })

  it("injects CSP correctly when <head> has an attribute containing '>'", () => {
    const out = withSandboxCsp(
      `<html><head data-x="a>b"></head><body></body></html>`
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    expect(doc.head.getAttribute("data-x")).toBe("a>b")
    const first = doc.head.firstElementChild
    expect(first?.tagName.toLowerCase()).toBe("meta")
    expect(first?.getAttribute("http-equiv")).toBe("Content-Security-Policy")
  })

  it("targets the real <head>, not a '<head' inside an attribute value", () => {
    const out = withSandboxCsp(
      `<html data-x="<head >"><head></head><body></body></html>`
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    const metas = doc.head.querySelectorAll(
      'meta[http-equiv="Content-Security-Policy"]'
    )
    expect(metas.length).toBe(1)
    expect(doc.head.firstElementChild?.getAttribute("http-equiv")).toBe(
      "Content-Security-Policy"
    )
  })

  it("ignores a fake '</scriptx>' and a '<head>' inside script text", () => {
    const out = withSandboxCsp(
      `<html><script>var s = "</scriptx><head></head>";</script>` +
        `<head></head><body></body></html>`
    )
    const doc = new DOMParser().parseFromString(out, "text/html")
    const metas = doc.head.querySelectorAll(
      'meta[http-equiv="Content-Security-Policy"]'
    )
    expect(metas.length).toBe(1)
  })
})
