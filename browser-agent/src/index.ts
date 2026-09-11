/**
 * The agent-facing half of a browser tab, running in the isolated world.
 *
 * `channel.ts`'s primitive carries page state back to Rust. This bundle is the
 * other direction: Rust evaluates `__codegAgent.<fn>(...)` in the same world
 * and reads the JSON it returns. Nothing here is reachable from the page — the
 * world is separate, and the page never sees `__codegAgent`.
 *
 * The tree itself is Playwright's, vendored under `../vendor/playwright`
 * (see VENDOR.md). We call it in `ai` mode, which is the mode Playwright MCP
 * uses, so the shape an agent reads here is the shape it already knows.
 */

import {
  generateAriaTree,
  renderAriaTreeAsJSON,
} from "../vendor/playwright/injected/ariaSnapshot"
import { renderAriaSnapshotAsYaml } from "../vendor/playwright/isomorphic/ariaSnapshotRenderer"

/**
 * Identifies the world this script is running in, and with it the document.
 *
 * Playwright hands out `e1`, `e2`, … from a counter that lives in the module,
 * so a fresh document starts over at `e1`. Two pages therefore use the same
 * names for different elements, and a ref an agent read before a navigation
 * would land on whatever happens to be first in the new page — silently, and
 * on an element the agent never saw. The generation makes that answerable:
 * every snapshot reports the one it was taken in, and a request carrying an
 * older one is refused instead of resolved.
 *
 * A page cannot influence it: the script is evaluated at document start in a
 * world the page cannot reach, before any page script runs.
 */
const GENERATION = generationToken()

function generationToken(): string {
  const buf = new Uint32Array(2)
  // `getRandomValues` is available on insecure origins too — it is
  // `crypto.subtle` that is not — so a plain-http dev server takes this path
  // like anything else. The fallback is there for the engine that surprises
  // us: a predictable generation still separates one document from the next,
  // which is all this value has to do, and the page cannot read it either way.
  if (globalThis.crypto?.getRandomValues) globalThis.crypto.getRandomValues(buf)
  else
    for (let i = 0; i < buf.length; i++)
      buf[i] = (Math.random() * 2 ** 32) >>> 0
  return `${buf[0].toString(36)}${buf[1].toString(36)}`
}

/**
 * The elements the last snapshot named, by ref.
 *
 * Replaced wholesale on every snapshot rather than accumulated: a ref only
 * means anything against the tree it was read from, and keeping older ones
 * around would let a stale name resolve to an element still in the document.
 *
 * The elements are held strongly, which is bounded — one tree's worth, dropped
 * at the next snapshot, and the whole world dies with the document. What a
 * WeakRef would buy is not the memory but the answer for an element that has
 * since been removed from the page, and `isConnected` answers that exactly,
 * without asking for a `WeakRef` the oldest WebKit we support may not have.
 */
let refs = new Map<string, Element>()

export type SnapshotOptions = {
  /** Cap on the rendered tree. Omitted or non-positive means no cap. */
  maxChars?: number
}

export type SnapshotResult = {
  generation: string
  url: string
  title: string
  viewport: { width: number; height: number; dpr: number }
  tree: string
  refsCount: number
  truncated: boolean
}

/** Reads the page into the tree an agent operates on. */
export function snapshot(options: SnapshotOptions = {}): SnapshotResult {
  const root = document.body ?? document.documentElement
  const next = new Map<string, Element>()
  let rendered = ""

  if (root) {
    const aria = generateAriaTree(root, { mode: "ai" })
    for (const [ref, info] of aria.info) next.set(ref, info.element)
    const { json } = renderAriaTreeAsJSON(aria, { mode: "ai" })
    rendered = renderAriaSnapshotAsYaml(json)
  }
  refs = next

  const { text, truncated } = truncate(rendered, options.maxChars)
  return {
    generation: GENERATION,
    url: location.href,
    title: document.title,
    viewport: {
      width: window.innerWidth,
      height: window.innerHeight,
      dpr: window.devicePixelRatio,
    },
    tree: text,
    refsCount: next.size,
    truncated,
  }
}

/**
 * The element a ref names, or `null` if the ref cannot be honoured.
 *
 * Three ways it cannot: the snapshot it came from was taken in another
 * document, the last snapshot did not name it, or the element it named has
 * since left the page. All three are one answer to the caller — take a new
 * snapshot — so they are one return value here.
 */
export function elementForRef(generation: string, ref: string): Element | null {
  if (generation !== GENERATION) return null
  const element = refs.get(ref)
  if (!element?.isConnected) return null
  return element
}

/**
 * Cuts the tree to `maxChars` on a line boundary.
 *
 * Mid-line would hand the agent a half-written node — a ref with no role, or a
 * role with half its name — which reads as a real entry rather than a cut. The
 * first line is kept whole even when it alone is over the cap, because a tree
 * cut to nothing says less than a tree cut to one node.
 */
export function truncate(
  text: string,
  maxChars: number | undefined
): { text: string; truncated: boolean } {
  if (!maxChars || maxChars <= 0 || text.length <= maxChars)
    return { text, truncated: false }
  const cut = text.lastIndexOf("\n", maxChars)
  return {
    text: cut > 0 ? text.slice(0, cut) : text.slice(0, maxChars),
    truncated: true,
  }
}

declare global {
  var __codegAgent: {
    snapshot: typeof snapshot
    elementForRef: typeof elementForRef
  }
}

globalThis.__codegAgent = { snapshot, elementForRef }
