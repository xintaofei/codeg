import type { ComponentProps } from "react"
import type { Streamdown } from "streamdown"
import { visit } from "unist-util-visit"

type RehypePlugins = NonNullable<
  ComponentProps<typeof Streamdown>["rehypePlugins"]
>
type RehypePlugin = RehypePlugins[number]

/** Minimal view of the sanitize schema's attribute allow-list we extend. */
type SanitizeSchema = {
  attributes?: Record<string, unknown[]>
  [key: string]: unknown
}

type HastElementLike = {
  type: string
  tagName?: string
  properties?: Record<string, unknown>
  children?: unknown
}

/**
 * hast property carrying a relative local link's original href past
 * rehype-harden. Set by `remarkRewriteFileUriLinks` (as `hProperties`), read and
 * removed by {@link rehypeRestoreRelativeFileLinks}.
 */
export const RELATIVE_FILE_HREF_PROPERTY = "dataCodegRelativeHref"
const RELATIVE_FILE_HREF_ATTRIBUTE = "data-codeg-relative-href"

/**
 * What rehype-harden turns a path-relative href into: it resolves the url
 * against a placeholder origin and keeps `pathname + search + hash`, so
 * `./index.html` becomes `/index.html`.
 */
function hardenedForm(href: string): string | null {
  try {
    const url = new URL(href, "http://example.com")
    return url.pathname + url.search + url.hash
  } catch {
    return null
  }
}

/**
 * Put back the relative href rehype-harden flattened into a root path.
 *
 * Runs after harden. The original is taken back only when it is still an
 * explicitly relative path (`./…` / `../…`) AND harden's output is exactly
 * what harden makes of it — so the attribute can neither introduce a scheme
 * nor redirect a link to somewhere its href never pointed. The attribute is
 * always removed, restored or not.
 */
export function rehypeRestoreRelativeFileLinks() {
  return (tree: HastElementLike) => {
    visit(tree as never, "element", (node: HastElementLike) => {
      if (node.tagName !== "a" || !node.properties) return
      const props = node.properties
      const stored =
        props[RELATIVE_FILE_HREF_PROPERTY] ??
        props[RELATIVE_FILE_HREF_ATTRIBUTE]
      if (stored === undefined) return
      delete props[RELATIVE_FILE_HREF_PROPERTY]
      delete props[RELATIVE_FILE_HREF_ATTRIBUTE]
      if (typeof stored !== "string") return
      if (!stored.startsWith("./") && !stored.startsWith("../")) return
      if (typeof props.href !== "string") return
      if (props.href !== hardenedForm(stored)) return
      props.href = stored
    })
  }
}

/**
 * Wire relative local links through a Streamdown rehype pipeline: allow the
 * carrier attribute on `<a>` in the sanitize schema (both hast spellings, as
 * for the local-image span attributes in ./rehype-allow-codeg), and append the
 * restore step after every existing plugin — harden included. Keys and order
 * of the existing plugins are preserved.
 */
export function withRelativeFileLinks(
  plugins: Record<string, RehypePlugin>
): Record<string, RehypePlugin> {
  const next: Record<string, RehypePlugin> = {}
  for (const [key, plugin] of Object.entries(plugins)) {
    if (key !== "sanitize") {
      next[key] = plugin
      continue
    }
    const [sanitizePlugin, schema] = (
      Array.isArray(plugin) ? plugin : [plugin]
    ) as [RehypePlugin, SanitizeSchema?]
    next[key] = [
      sanitizePlugin,
      {
        ...schema,
        attributes: {
          ...schema?.attributes,
          a: [
            ...(schema?.attributes?.a ?? []),
            RELATIVE_FILE_HREF_PROPERTY,
            RELATIVE_FILE_HREF_ATTRIBUTE,
          ],
        },
      },
    ] as RehypePlugin
  }
  next.restoreRelativeFileLinks = rehypeRestoreRelativeFileLinks as RehypePlugin
  return next
}
