import { RELATIVE_FILE_HREF_PROPERTY } from "./rehype-relative-file-links"

// Local-file markdown links are otherwise rendered as `… [blocked]`. Two
// distinct sanitize/harden rules cause this, both sidestepped here in the mdast
// layer (before remark-rehype) while keeping the link clickable through the
// existing link-safety + open-file-dialog flow:
//
//   1. `file://` hrefs — rehype-harden hard-codes `file:` in its blocked-
//      protocol list. Rewritten to a bare local path (POSIX `/…`, `/C:/…` for
//      Windows drives, or a `\\server\share` UNC form).
//   2. Bare Windows drive paths (`C:/…`, `C:\…`) — rehype-sanitize reads the
//      leading `C:` as a URL protocol and strips the href, after which harden
//      blocks the now-hrefless `<a>`. Rewritten to `/C:/…` so `C:` is no longer
//      in protocol position (see {@link windowsDrivePathToSafe}).
//
// Relative local links (`./index.html`, `../a/b.md`, and bare `index.html`)
// hit a third rule, in rehype-harden itself: a schemeless url is parsed
// against a placeholder origin and only its `pathname` is kept, so
// `./index.html` leaves as `/index.html` — a path at the filesystem ROOT — and a
// bare `index.html` does not parse at all and is `[blocked]`. harden rewrites
// only `href` (and `target`/`rel`), so the original is carried past it in a
// data attribute and put back afterwards by `rehypeRestoreRelativeFileLinks`
// (see ./rehype-relative-file-links).
//
// Image destinations are handled by remarkLocalImages, which preserves their
// original path until the workspace-confined image reader can resolve it.

type MdastNodeLike = {
  type: string
  url?: unknown
  identifier?: unknown
  children?: unknown
  data?: { hProperties?: Record<string, unknown> }
}

function fileUriToLocalPath(uri: string): string | null {
  if (!/^file:\/\//i.test(uri)) return null
  let parsed: URL
  try {
    parsed = new URL(uri)
  } catch {
    return null
  }
  // A non-empty host is a UNC authority: file://server/share/x parses as
  // host="server", pathname="/share/x". Emit the BACKSLASH UNC form
  // \\server\share\x — unambiguously LOCAL. A forward-slash //server/share
  // would be indistinguishable from a protocol-relative WEB url once the
  // file: scheme is gone, and downstream (classifyResourceKind /
  // link-safety) route bare // to the browser; backslashes never appear in
  // a web url, so they reliably tag the target as a local file. The click
  // path normalizes the separators back to // before opening.
  if (parsed.host) {
    const body = `${parsed.host}${parsed.pathname}`.replace(/\//g, "\\")
    return `\\\\${body}${parsed.search}${parsed.hash}`
  }
  // Keep the pathname verbatim, INCLUDING the leading slash before a Windows
  // drive letter (`/C:/…`). Stripping it to a bare `C:/…` makes the downstream
  // rehype-sanitize step read `C:` as a URL protocol and drop the href, after
  // which rehype-harden replaces the link with a `… [blocked]` span. The
  // leading slash is stripped back off before the file is opened
  // (link-safety's `stripLeadingSlashOnWindows`). POSIX paths already start
  // with a slash, so they are unaffected. URL-encoded form is preserved so
  // `%23` / `%3F` don't collide with fragment/query boundaries when the click
  // handler later splits on `#` / `?`.
  return `${parsed.pathname}${parsed.search}${parsed.hash}`
}

// A bare Windows drive-letter path (`C:/…` or `C:\…`, no `file://` scheme) hits
// the same wall as the rewritten `file://` drive path above: rehype-sanitize
// parses the leading `C:` as a URL protocol and strips the href → harden emits
// `… [blocked]`. Prefixing a single slash — `/C:/…` — pushes the colon past the
// first `/`, so sanitize sees a schemeless, path-absolute URL and keeps it.
// Downstream (`classifyResourceKind`, link-safety's `stripLeadingSlashOnWindows`,
// `normalizeAbsPath`) already strips that leading slash back off before opening,
// so the opener still receives `C:/…`. This adds no new allowed protocol.
const WINDOWS_DRIVE_PATH = /^[a-zA-Z]:[\\/]/

function windowsDrivePathToSafe(url: string): string | null {
  return WINDOWS_DRIVE_PATH.test(url) ? `/${url}` : null
}

/** Rewrite a `file://` URI or a bare Windows drive path to a sanitize-safe form. */
function rewriteLocalFileUrl(url: string): string | null {
  return fileUriToLocalPath(url) ?? windowsDrivePathToSafe(url)
}

// Anything with a scheme, a fragment-only `#anchor`, an absolute or home path,
// a UNC/protocol-relative `//`/`\\` start, or a `www.` host is not a relative
// local path.
const NOT_BARE_RELATIVE = /^(?:[a-zA-Z][a-zA-Z\d+\-.]*:|[/#\\~]|www\.)/i
// A bare path whose first segment ends in one of these is a scheme-less web
// address (`example.com`, `github.com/a/b`), not a file: `index.html` and
// `example.com` are the same shape, and only the suffix tells them apart. Such
// a link stays as it is. `sh` is left out on purpose: in a coding transcript
// `deploy.sh` is a script far more often than a `*.sh` host linked without its
// scheme.
const DOMAIN_LIKE_SUFFIXES = new Set([
  "com",
  "net",
  "org",
  "io",
  "dev",
  "ai",
  "app",
  "co",
  "cn",
  "me",
  "xyz",
  "info",
  "edu",
  "gov",
  "uk",
  "de",
  "jp",
  "kr",
  "tw",
  "hk",
])

/**
 * The explicitly-relative form (`./…` / `../…`) of a relative local link, or
 * `null` when `url` is not one. An explicit `./` / `../` always counts, spaces
 * included (`[x](<./my notes.md>)`). A bare path (`index.html`, `src/main.rs`)
 * counts only when it is shaped like a file: a slash somewhere, or an
 * extension on its last segment — and its first segment doesn't end in a
 * domain suffix.
 */
export function explicitRelativeFileHref(url: string): string | null {
  const trimmed = url.trim()
  if (!trimmed) return null
  if (trimmed.startsWith("./") || trimmed.startsWith("../")) return trimmed
  if (/\s/.test(trimmed) || NOT_BARE_RELATIVE.test(trimmed)) return null
  const firstSegment = trimmed.split(/[/?#]/, 1)[0]
  const hostSuffix = firstSegment.match(/\.([A-Za-z0-9]+)$/)?.[1]?.toLowerCase()
  if (hostSuffix && DOMAIN_LIKE_SUFFIXES.has(hostSuffix)) return null
  const hasSlash = trimmed.includes("/")
  const hasExtension = /\.[A-Za-z0-9]{1,8}(?:[#?]|$)/.test(trimmed)
  return hasSlash || hasExtension ? `./${trimmed}` : null
}

/** Carry the explicit relative href past harden on the element this node becomes. */
function markRelative(node: MdastNodeLike, href: string): void {
  node.data = {
    ...node.data,
    hProperties: {
      ...node.data?.hProperties,
      [RELATIVE_FILE_HREF_PROPERTY]: href,
    },
  }
}

function walk(node: MdastNodeLike, fn: (n: MdastNodeLike) => void): void {
  fn(node)
  const { children } = node
  if (Array.isArray(children)) {
    for (const child of children) {
      walk(child as MdastNodeLike, fn)
    }
  }
}

export function remarkRewriteFileUriLinks() {
  return (tree: MdastNodeLike) => {
    // Definitions are shared between linkReference and imageReference. Skip
    // any definition whose identifier is consumed by an imageReference so
    // image blocking still wins for those cases.
    const imageRefIds = new Set<string>()
    walk(tree, (node) => {
      if (
        node.type === "imageReference" &&
        typeof node.identifier === "string"
      ) {
        imageRefIds.add(node.identifier.toLowerCase())
      }
    })

    // Relative definitions, by identifier: the `<a>` of a `[text][id]` link is
    // built from its linkReference node, so that node carries the mark. Only
    // the FIRST definition of an identifier counts — it is the one CommonMark
    // (and mdast-util-to-hast) resolves the link to, so a later duplicate must
    // not put its path on the link: `[doc]: /a.md` then `[doc]: a.md` would
    // otherwise turn the absolute `/a.md` into `./a.md`.
    const seenDefinitionIds = new Set<string>()
    const relativeDefinitions = new Map<string, string>()

    walk(tree, (node) => {
      if (typeof node.url !== "string") return
      if (node.type === "link") {
        const rewritten = rewriteLocalFileUrl(node.url)
        if (rewritten != null) {
          node.url = rewritten
          return
        }
        const relative = explicitRelativeFileHref(node.url)
        if (relative != null) {
          node.url = relative
          markRelative(node, relative)
        }
        return
      }
      if (node.type === "definition") {
        const id =
          typeof node.identifier === "string"
            ? node.identifier.toLowerCase()
            : ""
        if (imageRefIds.has(id)) return
        const effective = !seenDefinitionIds.has(id)
        seenDefinitionIds.add(id)
        const rewritten = rewriteLocalFileUrl(node.url)
        if (rewritten != null) {
          node.url = rewritten
          return
        }
        const relative = explicitRelativeFileHref(node.url)
        if (relative != null) {
          node.url = relative
          if (effective) relativeDefinitions.set(id, relative)
        }
      }
    })

    if (relativeDefinitions.size === 0) return
    walk(tree, (node) => {
      if (node.type !== "linkReference") return
      const id =
        typeof node.identifier === "string" ? node.identifier.toLowerCase() : ""
      const relative = relativeDefinitions.get(id)
      if (relative != null) markRelative(node, relative)
    })
  }
}
