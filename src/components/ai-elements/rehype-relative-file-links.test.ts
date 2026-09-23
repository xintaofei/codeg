import { describe, expect, it } from "vitest"
import { defaultRehypePlugins } from "streamdown"

import {
  RELATIVE_FILE_HREF_PROPERTY,
  rehypeRestoreRelativeFileLinks,
  withRelativeFileLinks,
} from "./rehype-relative-file-links"

type El = {
  type: string
  tagName?: string
  properties?: Record<string, unknown>
  children?: El[]
}

function anchor(properties: Record<string, unknown>): El {
  return {
    type: "root",
    children: [{ type: "element", tagName: "a", properties, children: [] }],
  }
}

function restore(properties: Record<string, unknown>): Record<string, unknown> {
  const tree = anchor(properties)
  rehypeRestoreRelativeFileLinks()(tree)
  return tree.children![0].properties!
}

describe("rehypeRestoreRelativeFileLinks", () => {
  it("puts back the relative href harden flattened into a root path", () => {
    expect(
      restore({
        href: "/index.html",
        [RELATIVE_FILE_HREF_PROPERTY]: "./index.html",
      })
    ).toEqual({ href: "./index.html" })
    expect(
      restore({
        href: "/site/a.md#L2",
        [RELATIVE_FILE_HREF_PROPERTY]: "../site/a.md#L2",
      })
    ).toEqual({ href: "../site/a.md#L2" })
  })

  it("keeps harden's href when it is not what harden makes of the carried one", () => {
    expect(
      restore({
        href: "/other.html",
        [RELATIVE_FILE_HREF_PROPERTY]: "./index.html",
      })
    ).toEqual({ href: "/other.html" })
    // A web link never matches: harden keeps its origin, the carried path
    // flattens to a bare pathname.
    expect(
      restore({
        href: "https://example.com/index.html",
        [RELATIVE_FILE_HREF_PROPERTY]: "./index.html",
      })
    ).toEqual({ href: "https://example.com/index.html" })
  })

  it("never restores a value that is not explicitly relative", () => {
    for (const stored of ["javascript:alert(1)", "https://x.test/", "/abs"]) {
      expect(
        restore({ href: "/abs", [RELATIVE_FILE_HREF_PROPERTY]: stored })
      ).toEqual({ href: "/abs" })
    }
    // These DO flatten to the very href harden produced, so only the `./` /
    // `../` requirement keeps a web address from replacing it.
    for (const stored of ["https://evil.test/abs", "//evil.test/abs"]) {
      expect(
        restore({ href: "/abs", [RELATIVE_FILE_HREF_PROPERTY]: stored })
      ).toEqual({ href: "/abs" })
    }
  })

  it("leaves links without the carrier untouched", () => {
    expect(restore({ href: "https://example.com/" })).toEqual({
      href: "https://example.com/",
    })
  })
})

describe("withRelativeFileLinks", () => {
  it("allows the carrier on <a> in sanitize and appends the restore last", () => {
    const plugins = withRelativeFileLinks(defaultRehypePlugins)
    expect(Object.keys(plugins)).toEqual([
      ...Object.keys(defaultRehypePlugins),
      "restoreRelativeFileLinks",
    ])
    const schema = (plugins.sanitize as unknown[])[1] as {
      attributes: Record<string, unknown[]>
    }
    expect(schema.attributes.a).toEqual(
      expect.arrayContaining([
        RELATIVE_FILE_HREF_PROPERTY,
        "data-codeg-relative-href",
      ])
    )
  })
})
