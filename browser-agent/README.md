# browser-agent

The script a browser tab's **isolated world** runs when an agent needs to read
the page. Built by esbuild into `src-tauri/src/browser/js/agent.bundle.js`,
which is committed.

```
pnpm browser:agent          # build the bundle
pnpm browser:agent:check    # is the committed bundle the one this source makes?
pnpm browser:agent:types    # typecheck (esbuild does not)
pnpm browser:agent:probe    # drive the bundle in real Chrome
```

## What it is

`vendor/playwright/` is Playwright's aria tree, copied byte-for-byte at v1.63.0
(see `vendor/playwright/VENDOR.md`). `src/index.ts` calls it in **`ai` mode** —
the mode Playwright MCP uses — and puts `snapshot` and `elementForRef` on
`globalThis.__codegAgent` for Rust to call through world-scoped eval.

`ai` mode is why there is no second pass over the DOM here. It gives a ref to
every element that is _visible and receives pointer events_, so a `<div>` with
`cursor: pointer` and a click handler and no ARIA role is namable, and carries
`[cursor=pointer]` to say why. An earlier plan for this package described
writing that promotion ourselves; upstream had already made it unnecessary.

## Two things it does that upstream does not

**Refs are answerable across a navigation.** Playwright's ref counter lives in
the module, so a fresh document starts again at `e1` — two pages use the same
names for different elements. Each world here draws a random `generation` and
reports it with every snapshot; `elementForRef` refuses a ref that quotes an
older one. A new document destroys the world, so the next snapshot is a new
generation and every ref an agent still holds is refused rather than resolved
onto whatever now happens to be `e1`.

A new document is not the only kind of navigation, and the other kind is the
common one here: `pushState`, `replaceState` and hash changes leave the
document, the world and the generation exactly as they were while the page
becomes a different page. That is a route change in a single-page app — which
is most of what a dev server serves — and the elements a framework keeps
across one, the header and its buttons, are exactly the ones that would still
resolve. So a snapshot also records the address it was taken at, and a ref is
refused once the page has moved. The error runs in the safe direction: a
caller told to snapshot again loses a round trip, a caller handed the wrong
element loses the user's page.

**Where that stops, and who takes over.** An address is not an identity. A
route that goes A → B → A arrives back at a string that matches, on a page
whose framework may have kept the DOM node and given it new meaning, and this
world cannot see that it happened: the page's own `history.pushState` is
invisible from an isolated world, because patching `History.prototype` here
patches _this_ world's prototype while the page calls a different function
object — the same isolation that keeps `__codegAgent` out of the page's reach.

So the world enforces three floors it can check by looking — a new document, a
moved address, a departed element — and `snapshot({ epoch })` mixes a host
token into the generation an agent echoes back. Deciding _when_ refs die is
the host's, because the host is the only party that sees the navigation.

The probe measures the world's side of that, including the A → B → A case that
the floors do not catch, and the premise underneath it: it patches
`History.prototype.pushState` in the world, has the _page_ navigate, and
checks that the patch never fired while the address moved anyway. It cannot
measure the host's side, because there is no host here yet.

**The tree can be capped.** `maxChars` cuts on a line boundary, so an agent
never reads half a node, and the result says `truncated` so it knows to narrow
the question rather than believe the page ended. A cap that lands inside the
very first line has no boundary to use; the cap wins there and the line is cut
where it falls.

## What is not here

Nothing reads this bundle yet. The Rust seam is where **authorization** lives —
`none / read / control`, per tab and origin, with no automatic grants — and
until that exists there must be no code path that reads a page on an agent's
behalf. The bundle and the grant model land together, in the package that adds
the `browser_*` tools.

## Where it runs

The world is separate from the page: `__codegAgent` is not reachable from page
script, and page code cannot forge a ref or observe a snapshot. The probe
asserts this. What the probe cannot assert is the engine — it drives Chrome,
while the three platforms ship WKWebView, WebView2 and WebKitGTK.
