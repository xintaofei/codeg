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
  <div id="clickable" style="cursor:pointer" onclick="void 0">A div nobody gave a role</div>
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

  // A roleless div that behaves like a button has to be namable, or an agent
  // cannot act on the many pages that are built out of them.
  check(
    "a clickable div gets a ref",
    /generic \[ref=e\d+\] \[cursor=pointer\]: A div nobody gave a role/.test(
      snap.tree
    ),
    true
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

  const ref = snap.tree.match(/\[ref=(e\d+)\]/)[1]
  const g = JSON.stringify(snap.generation)
  const r = JSON.stringify(ref)
  check(
    "a live ref resolves",
    await run(`!!__codegAgent.elementForRef(${g}, ${r})`),
    true
  )
  check(
    "a ref from another document does not",
    await run(`__codegAgent.elementForRef("other", ${r})`),
    null
  )
  check(
    "a ref for a removed element does not",
    await run(
      `(() => { __codegAgent.elementForRef(${g}, ${r}).remove();
                return __codegAgent.elementForRef(${g}, ${r}) })()`
    ),
    null
  )

  console.log(failures ? `\n${failures} failed` : "\nall checks passed")
} finally {
  ws?.close()
  chrome.kill()
}

process.exit(failures ? 1 : 0)
