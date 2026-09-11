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
older one. A navigation destroys the world, so the next snapshot is a new
generation and every ref an agent still holds is refused rather than resolved
onto whatever now happens to be `e1`.

**The tree can be capped.** `maxChars` cuts on a line boundary, so an agent
never reads half a node, and the result says `truncated` so it knows to narrow
the question rather than believe the page ended.

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
