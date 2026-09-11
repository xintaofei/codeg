/**
 * Drives the committed bundle in a real engine and prints what it produces.
 *
 *     pnpm browser:agent:probe
 *
 * The unit tests cover the part of `src/index.ts` that is pure. They cannot
 * cover the tree: jsdom reports every element as zero-sized, and `ai` mode
 * only names elements that are visible and receive pointer events, so under
 * jsdom the tree comes back with no refs and proves nothing. This asks a
 * browser instead.
 *
 * Chrome, because it is the one engine present on all three of our
 * development machines and it speaks CDP without a driver. It is not the
 * engine any platform actually ships — WKWebView, WebView2 and WebKitGTK are —
 * so a green run here says the bundle is sound, not that it is verified on a
 * platform. Manual, not part of `pnpm test`: it needs a browser on the box.
 */
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const BUNDLE = readFileSync(
  resolve(root, "src-tauri/src/browser/js/agent.bundle.js"),
  "utf8"
)

const CHROME =
  process.env.CHROME_PATH ??
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
const PORT = 9333

const PAGE = `<!doctype html><html><head><title>Probe</title></head><body>
<header><a href="/docs">Docs</a></header>
<main>
  <h1>Orders</h1>
  <label>Search <input type="search" name="q"></label>
  <button id="exp" style="cursor:pointer">Export</button>
  <div id="pointer" style="cursor:pointer">Pointer but no handler</div>
  <div id="handler" onclick="void 0">Handler but no pointer</div>
  <div id="focusable" tabindex="0">Focusable but neither</div>
  <div id="hidden" style="display:none"><button>Invisible</button></div>
  <ul><li>alpha</li><li>beta</li></ul>
</main></body></html>`

const dir = mkdtempSync(join(tmpdir(), "codeg-agent-probe-"))
const pageFile = join(dir, "probe.html")
writeFileSync(pageFile, PAGE)

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const chrome = spawn(
  CHROME,
  [
    "--headless=new",
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=${join(dir, "profile")}`,
    "--no-first-run",
    "--disable-gpu",
    "about:blank",
  ],
  { stdio: "ignore" }
)

let ws
let failures = 0
const check = (label, actual, expected) => {
  const ok = JSON.stringify(actual) === JSON.stringify(expected)
  if (!ok) failures++
  console.log(`${ok ? "ok  " : "FAIL"}  ${label}: ${JSON.stringify(actual)}`)
}

try {
  for (let i = 0; i < 100 && !ws; i++) {
    try {
      const list = await (
        await fetch(`http://127.0.0.1:${PORT}/json/list`)
      ).json()
      const page = list.find((t) => t.type === "page")
      if (page) ws = new WebSocket(page.webSocketDebuggerUrl)
    } catch {
      await sleep(100)
    }
  }
  if (!ws) throw new Error(`no page target — is Chrome at ${CHROME}?`)
  await new Promise((r) => (ws.onopen = r))

  let id = 0
  const pending = new Map()
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data)
    pending.get(m.id)?.(m)
    pending.delete(m.id)
  }
  const send = (method, params = {}) =>
    new Promise((r) => {
      const i = ++id
      pending.set(i, r)
      ws.send(JSON.stringify({ id: i, method, params }))
    })

  await send("Page.enable")
  await send("Page.navigate", { url: `file://${pageFile}` })
  await sleep(700)

  // An isolated world, created the way the shims create one.
  const { result: frameTree } = await send("Page.getFrameTree")
  const { result: world } = await send("Page.createIsolatedWorld", {
    frameId: frameTree.frameTree.frame.id,
    worldName: "codeg",
    grantUniveralAccess: false,
  })

  const run = async (expression, contextId = world.executionContextId) => {
    const { result } = await send("Runtime.evaluate", {
      expression,
      contextId,
      returnByValue: true,
    })
    if (result.exceptionDetails)
      throw new Error(JSON.stringify(result.exceptionDetails, null, 2))
    return result.result.value
  }

  await run(BUNDLE)

  const snap = JSON.parse(
    await run("JSON.stringify(__codegAgent.snapshot({}))")
  )
  console.log("\n=== tree ===\n" + snap.tree + "\n")
  console.log(`url=${snap.url} title=${snap.title} refs=${snap.refsCount}\n`)

  // Roleless divs that behave like controls have to be namable, or an agent
  // cannot act on the many pages that are built out of them. This is the whole
  // reason there is no promotion pass of our own: `ai` mode refs everything
  // visible that receives pointer events, so each of these is already named,
  // whichever single attribute makes it interesting. Kept as three separate
  // elements so that one of them regressing cannot hide behind another.
  const named = (id, text) =>
    check(
      `a roleless div is namable — ${id}`,
      new RegExp(`generic \\[ref=e\\d+\\][^\\n]*: ${text}`).test(snap.tree),
      true
    )
  named("cursor:pointer only", "Pointer but no handler")
  named("onclick only", "Handler but no pointer")
  named("tabindex only", "Focusable but neither")

  // …and `cursor: pointer` is additionally marked, which is how an agent tells
  // "this looks clickable" from "this is merely visible".
  check(
    "cursor:pointer is reported, and only where it applies",
    [
      /\[cursor=pointer\][^\n]*: Pointer but no handler/.test(snap.tree),
      /\[cursor=pointer\][^\n]*: Handler but no pointer/.test(snap.tree),
    ],
    [true, false]
  )

  check(
    "a display:none subtree is left out",
    snap.tree.includes("Invisible"),
    false
  )
  check("the link keeps its href", snap.tree.includes("/url: /docs"), true)

  // The page must not be able to see, call or forge the agent surface.
  const mainWorld = await send("Runtime.evaluate", {
    expression: "typeof globalThis.__codegAgent",
    returnByValue: true,
  })
  check(
    "the page cannot see __codegAgent",
    mainWorld.result.result.value,
    "undefined"
  )

  const cut = JSON.parse(
    await run("JSON.stringify(__codegAgent.snapshot({maxChars: 40}))")
  )
  check("a capped tree reports the cut", cut.truncated, true)
  check("a capped tree ends on a line boundary", cut.tree.endsWith(":"), true)

  const g = JSON.stringify(snap.generation)
  // Two refs: one to spend on the removal case, one that must stay in the page
  // so the same-document case cannot pass for the wrong reason.
  const kept = JSON.stringify(
    snap.tree.match(/button "Export" \[ref=(e\d+)\]/)[1]
  )
  const spent = JSON.stringify(
    snap.tree.match(/listitem \[ref=(e\d+)\]: alpha/)[1]
  )

  check(
    "a live ref resolves",
    await run(`!!__codegAgent.elementForRef(${g}, ${kept})`),
    true
  )
  check(
    "a ref from another document does not",
    await run(`__codegAgent.elementForRef("other", ${kept})`),
    null
  )
  check(
    "a ref for a removed element does not",
    await run(
      `(() => { __codegAgent.elementForRef(${g}, ${spent}).remove();
                return __codegAgent.elementForRef(${g}, ${spent}) })()`
    ),
    null
  )

  // A single-page app's route change: same document, same world, same
  // generation, and the element is still in the page. Only the address moved.
  // Asserting that it is still connected is the point — otherwise a `null`
  // here would prove nothing about the address and everything about the node.
  check(
    "a ref does not survive a pushState, though its element does",
    await run(
      `(() => { const el = __codegAgent.elementForRef(${g}, ${kept});
                history.pushState({}, "", "?routed");
                return [el.isConnected, __codegAgent.elementForRef(${g}, ${kept})] })()`
    ),
    [true, null]
  )
  check(
    "and a snapshot at the new address hands out refs that work again",
    await run(
      `(() => { const s = __codegAgent.snapshot({});
                const m = s.tree.match(/button "Export" \\[ref=(e\\d+)\\]/);
                return !!__codegAgent.elementForRef(s.generation, m[1]) })()`
    ),
    true
  )

  // The boundary of what this world can know, pinned so that it reads as
  // known rather than as overlooked. An address is not an identity: a route
  // that leaves and comes back arrives at a string that matches, and a
  // framework may have kept the node and changed what it means. The world
  // cannot see the transition — the page's own `pushState` is invisible from
  // an isolated world — so the ref still resolves here.
  check(
    "an address that leaves and returns defeats the address check",
    await run(
      `(() => { const here = location.href;
                const s = __codegAgent.snapshot({});
                const m = s.tree.match(/button "Export" \\[ref=(e\\d+)\\]/);
                history.pushState({}, "", "?elsewhere");
                history.pushState({}, "", here);
                return !!__codegAgent.elementForRef(s.generation, m[1]) })()`
    ),
    true
  )

  // …which is why the token carries whatever the host puts in it. The host
  // does see the transition, and a ref quoting an epoch it has moved past is
  // refused — by the host on the spot, and by this world from the next
  // snapshot on, which is what these two assert.
  check(
    "a host epoch reaches the token an agent echoes",
    await run(
      `__codegAgent.snapshot({epoch: "nav-7"}).generation.endsWith(".nav-7")`
    ),
    true
  )
  check(
    "and a ref from an earlier epoch dies at the next snapshot",
    await run(
      `(() => { const s = __codegAgent.snapshot({epoch: "nav-7"});
                const m = s.tree.match(/button "Export" \\[ref=(e\\d+)\\]/);
                __codegAgent.snapshot({epoch: "nav-8"});
                return __codegAgent.elementForRef(s.generation, m[1]) })()`
    ),
    null
  )

  // The premise the whole design rests on, measured instead of assumed: this
  // world cannot intercept the page's own history calls, which is why
  // deciding when refs die has to be the host's job. Patch
  // `History.prototype.pushState` here, then have the *page* navigate, and
  // watch the patch not fire. Last, because it leaves the page elsewhere.
  await run(`globalThis.__patchFired = false;
             History.prototype.pushState = new Proxy(History.prototype.pushState, {
               apply(t, self, args) { globalThis.__patchFired = true;
                                      return Reflect.apply(t, self, args) } })`)
  const before = await run("location.href")
  await send("Runtime.evaluate", {
    // No contextId: the page's own world, holding its own History.prototype.
    expression: 'history.pushState({}, "", "?from-the-page")',
    returnByValue: true,
  })
  check(
    "a page's own pushState is invisible to a patch in this world",
    [
      await run("globalThis.__patchFired"),
      (await run("location.href")) !== before,
    ],
    // Did not fire, yet the address did move — so the page really navigated
    // and the patch really did not see it.
    [false, true]
  )

  console.log(failures ? `\n${failures} failed` : "\nall checks passed")
} finally {
  ws?.close()
  chrome.kill()
}

process.exit(failures ? 1 : 0)
