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

/**
 * The address the last snapshot was taken at.
 *
 * The generation answers for a *new document*, which is the only kind of
 * navigation that destroys this world. It is not the only kind of navigation:
 * `pushState`, `replaceState` and a hash change all leave the document, the
 * world and this module exactly where they were while the page becomes a
 * different page. That is the ordinary case for the dev servers these tabs
 * exist to show — a route change in a single-page app — and the elements a
 * framework keeps across one, a header's buttons and its nav, are precisely
 * the ones still `isConnected` afterwards. Without this an agent could act on
 * `e8` from the page it read while looking at the page it did not.
 *
 * Compared rather than subscribed to, because there is nothing here to
 * subscribe to. See `epoch` on `SnapshotOptions`: this world structurally
 * cannot observe a page-initiated history call, so a *floor* it can check by
 * looking is worth more than a hook it cannot install.
 *
 * The direction of the error matters: this refuses some refs that would still
 * be sound, because a caller told to take a new snapshot loses a round trip,
 * while a caller handed the wrong element loses the user's page.
 */
let refsTakenAt = ""

/**
 * The token the last snapshot handed out, and the one a ref must quote.
 *
 * The world's own generation plus whatever the host attached to that snapshot
 * (`SnapshotOptions.epoch`), so a caller echoes one opaque string back and
 * neither side has to agree on what it is made of.
 */
let refsToken = ""

export type SnapshotOptions = {
  /** Cap on the rendered tree. Omitted or non-positive means no cap. */
  maxChars?: number
  /**
   * An opaque token from the host, mixed into the generation this snapshot
   * hands out and required back on every `elementForRef`.
   *
   * What it buys is enforcement *by the host*: the epoch rides inside the one
   * string the caller echoes, so the host can refuse a ref the moment it knows
   * the page moved on, by comparing against the epoch it is issuing now —
   * without a side table mapping snapshots to navigations. This world refuses
   * an older token only from the next snapshot onwards, because until then it
   * has no way to learn that anything happened.
   *
   * It exists because there is a class of staleness this world cannot see. The
   * page's own `history.pushState` is not observable from here: patching
   * `History.prototype` in an isolated world patches *this world's* prototype,
   * and the page calls a different function object — the same isolation that
   * keeps `__codegAgent` out of the page's reach keeps the page's navigations
   * out of ours. Comparing `location.href` catches the settled result of most
   * of them, but an address is not an identity: a route that goes A → B → A
   * arrives back at a string that matches, on a page whose framework may have
   * kept the DOM node and given it new meaning.
   *
   * The host is the only party that can see those transitions, through the
   * navigation it already tracks for the tab. So the decision of *when* refs
   * die is the host's, and enforcing it is this world's. What is checked here
   * without a host token — a new document, a moved address, a departed
   * element — is a floor, not the contract.
   */
  epoch?: string
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
  refsTakenAt = location.href
  // `!== undefined`, not truthiness: an empty epoch is a value the host chose
  // and must stay distinguishable from one it never sent, or a host that
  // happens to render an epoch as "" would silently get the untagged token and
  // a ref issued under it would outlive the epoch change it was meant to die
  // with.
  refsToken =
    options.epoch !== undefined ? `${GENERATION}.${options.epoch}` : GENERATION

  const { text, truncated } = truncate(rendered, options.maxChars)
  return {
    generation: refsToken,
    url: refsTakenAt,
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
 * Four ways it cannot: the token it quotes is not the one the last snapshot
 * handed out — another document, or a host that has since declared the page
 * moved on — the address has changed since that snapshot, the last snapshot
 * did not name this ref, or the element it named has left the page. All four
 * are one answer to the caller, take a new snapshot, so they are one return
 * value here.
 */
export function elementForRef(generation: string, ref: string): Element | null {
  // Against the last snapshot's token, not the world's: a host that bumps its
  // epoch between two snapshots means the earlier one's refs are no longer
  // answerable, even though the document never changed.
  if (!refsToken || generation !== refsToken) return null
  if (location.href !== refsTakenAt) return null
  const element = refs.get(ref)
  if (!element?.isConnected) return null
  return element
}

/**
 * Cuts the tree to `maxChars` on a line boundary.
 *
 * Mid-line would hand the agent a half-written node — a ref with no role, or a
 * role with half its name — which reads as a real entry rather than as a cut.
 *
 * There is one case with no line boundary to use: a cap that lands inside the
 * very first line. The cap wins there and the line is cut where it falls,
 * because the cap is the caller's own bound and returning nothing would read
 * as an empty page rather than as a tree that was too long. `truncated` says
 * which of the two happened either way.
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
