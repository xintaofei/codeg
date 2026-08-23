import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http"

import { afterEach, describe, expect, it } from "vitest"

import { BrowserRuntime } from "../browser-runtime.js"
import { NativeWebViewBackend } from "../native-webview-backend.js"

interface NativeTab {
  id: string
  generation: number
  url: string
  title: string
  loading: boolean
  canGoBack: boolean
  canGoForward: boolean
  state: "ready" | "loading" | "error"
  errorCode?: string | null
}

interface NativeSnapshot {
  connectionId: string
  tabs: NativeTab[]
  activeTabId: string | null
  surfaceBounds: { x: number; y: number; width: number; height: number }
  surfaceVisible: boolean
}

class MockNativeBridge {
  readonly token = "n".repeat(64)
  readonly commands: Array<Record<string, unknown>> = []
  readonly snapshots = new Map<string, NativeSnapshot>()
  forcedError: { code: string; message: string; retryable: boolean } | null =
    null
  private server = createServer((request, response) => {
    void this.handle(request, response)
  })

  async start(): Promise<string> {
    await new Promise<void>((resolve, reject) => {
      this.server.once("error", reject)
      this.server.listen(0, "127.0.0.1", () => {
        this.server.off("error", reject)
        resolve()
      })
    })
    const address = this.server.address()
    if (!address || typeof address === "string") throw new Error("no_address")
    return `http://127.0.0.1:${address.port}`
  }

  async stop(): Promise<void> {
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }

  addPopup(connectionId: string): NativeTab {
    const snapshot = this.snapshots.get(connectionId)
    if (!snapshot) throw new Error("missing_session")
    const tab = nativeTab("popup", 3, "https://popup.example/")
    snapshot.tabs.push(tab)
    snapshot.activeTabId = tab.id
    return tab
  }

  markCrashed(connectionId: string): void {
    const snapshot = this.snapshots.get(connectionId)
    const tab = snapshot?.tabs.find(
      (candidate) => candidate.id === snapshot.activeTabId
    )
    if (!tab) throw new Error("missing_tab")
    tab.state = "error"
    tab.errorCode = "BROWSER_CRASHED"
  }

  private async handle(
    request: IncomingMessage,
    response: ServerResponse
  ): Promise<void> {
    if (request.headers.authorization !== `Bearer ${this.token}`) {
      this.json(response, 401, {
        ok: false,
        error: {
          code: "NATIVE_BRIDGE_UNAUTHORIZED",
          message: "unauthorized",
          retryable: false,
        },
      })
      return
    }
    if (request.method === "GET" && request.url === "/v1/health") {
      this.json(response, 200, {
        ok: true,
        backend: "embedded_webview2",
        protocolVersion: "1",
      })
      return
    }
    if (request.method !== "POST" || request.url !== "/v1/command") {
      this.json(response, 404, { ok: false })
      return
    }
    const command = (await readJson(request)) as Record<string, unknown>
    this.commands.push(command)
    if (this.forcedError) {
      const error = this.forcedError
      this.forcedError = null
      this.json(
        response,
        error.code === "NATIVE_GENERATION_MISMATCH" ? 409 : 503,
        {
          ok: false,
          error,
        }
      )
      return
    }

    const connectionId = String(command.connectionId ?? "")
    if (command.command === "snapshot") {
      const snapshot = this.snapshots.get(connectionId)
      if (!snapshot) {
        this.json(response, 404, {
          ok: false,
          error: {
            code: "NATIVE_SESSION_NOT_FOUND",
            message: "session not found",
            retryable: false,
          },
        })
        return
      }
      this.value(response, { snapshot })
      return
    }
    if (command.command === "ensure") {
      const snapshot = this.ensure(connectionId)
      this.value(response, { snapshot })
      return
    }
    if (command.command === "create") {
      let snapshot = this.snapshots.get(connectionId)
      if (!snapshot) {
        snapshot = {
          connectionId,
          tabs: [],
          activeTabId: null,
          surfaceBounds: { x: 0, y: 0, width: 1, height: 1 },
          surfaceVisible: false,
        }
        this.snapshots.set(connectionId, snapshot)
      }
      const tab = nativeTab(`tab-${snapshot.tabs.length + 1}`, 1)
      snapshot.tabs.push(tab)
      snapshot.activeTabId = tab.id
      this.value(response, { snapshot })
      return
    }
    if (command.command === "navigate") {
      const snapshot = this.ensure(connectionId)
      const tab = this.tab(snapshot, command)
      tab.url = String(command.url)
      tab.title = "Navigated"
      this.value(response, { snapshot })
      return
    }
    if (command.command === "focus") {
      const snapshot = this.ensure(connectionId)
      const tab = this.tab(snapshot, command)
      snapshot.activeTabId = tab.id
      this.value(response, { snapshot })
      return
    }
    if (command.command === "close") {
      const snapshot = this.ensure(connectionId)
      const tab = this.tab(snapshot, command)
      snapshot.tabs = snapshot.tabs.filter(
        (candidate) => candidate.id !== tab.id
      )
      snapshot.activeTabId = snapshot.tabs[snapshot.tabs.length - 1]?.id ?? null
      this.value(response, { snapshot })
      return
    }
    if (
      command.command === "history" ||
      command.command === "reload" ||
      command.command === "stop"
    ) {
      const snapshot = this.ensure(connectionId)
      this.tab(snapshot, command)
      this.value(response, { snapshot })
      return
    }
    if (command.command === "recover") {
      const snapshot = this.ensure(connectionId)
      const tab = this.tab(snapshot, command)
      tab.generation += 1
      tab.state = "ready"
      tab.errorCode = null
      this.value(response, { snapshot })
      return
    }
    if (command.command === "downloads") {
      this.value(response, {
        downloads: [
          {
            guid: "download-1",
            state: "completed",
            filename: "result.txt",
            receivedBytes: 12,
            totalBytes: 12,
            path: "C:\\managed\\result.txt",
          },
        ],
      })
      return
    }
    if (command.command === "release") {
      this.snapshots.delete(connectionId)
      this.value(response, { released: true })
      return
    }
    if (command.command === "cdp") {
      const snapshot = this.ensure(connectionId)
      this.tab(snapshot, command)
      this.value(response, {
        result: cdpResult(String(command.method), command.params),
      })
      return
    }
    this.json(response, 400, { ok: false })
  }

  private ensure(connectionId: string): NativeSnapshot {
    let snapshot = this.snapshots.get(connectionId)
    if (!snapshot) {
      snapshot = {
        connectionId,
        tabs: [nativeTab("tab-1", 1)],
        activeTabId: "tab-1",
        surfaceBounds: { x: 0, y: 0, width: 1, height: 1 },
        surfaceVisible: false,
      }
      this.snapshots.set(connectionId, snapshot)
    }
    return snapshot
  }

  private tab(
    snapshot: NativeSnapshot,
    command: Record<string, unknown>
  ): NativeTab {
    const tab = snapshot.tabs.find(
      (candidate) => candidate.id === command.tabId
    )
    if (!tab) throw new Error("missing_tab")
    if (tab.generation !== command.generation)
      throw new Error("stale_generation")
    return tab
  }

  private value(response: ServerResponse, value: unknown): void {
    this.json(response, 200, { ok: true, value })
  }

  private json(response: ServerResponse, status: number, body: unknown): void {
    const text = JSON.stringify(body)
    response.statusCode = status
    response.setHeader("Content-Type", "application/json")
    response.end(text)
  }
}

const bridges: MockNativeBridge[] = []

afterEach(async () => {
  await Promise.all(
    bridges.splice(0).map(async (bridge) => await bridge.stop())
  )
})

describe("NativeWebViewBackend", () => {
  it("routes the public tools to Rust-owned tabs and one controller generation", async () => {
    const { bridge, runtime } = await createRuntime()
    expect(
      (await runtime.callTool("session-a", "runtime.start", {})).envelope.ok
    ).toBe(true)

    const opened = await runtime.callTool("session-a", "tabs.open", {
      url: "https://example.com/",
    })
    expect(opened.envelope).toMatchObject({
      ok: true,
      tab: { id: "tab-1", url: "https://example.com/" },
    })

    const snapshot = await runtime.callTool("session-a", "page.snapshot", {})
    expect(snapshot.envelope).toMatchObject({
      ok: true,
      data: {
        nodes: [{ ref: "e1", role: "button", name: "Continue" }],
      },
    })
    expect(
      (await runtime.callTool("session-a", "action.click", { ref: "e1" }))
        .envelope.ok
    ).toBe(true)
    const screenshot = await runtime.callTool(
      "session-a",
      "page.screenshot",
      {}
    )
    expect(screenshot.image).toEqual({ data: "AA==", mimeType: "image/png" })

    bridge.addPopup("session-a")
    const listed = await runtime.callTool("session-a", "tabs.list", {})
    expect(listed.envelope.data).toMatchObject({
      tabs: [{ id: "tab-1" }, { id: "popup" }],
      activeTabId: "popup",
    })

    const downloads = await runtime.callTool("session-a", "download.list", {})
    expect(downloads.envelope.data).toMatchObject({
      downloads: [{ guid: "download-1", state: "completed" }],
    })
    const waited = await runtime.callTool("session-a", "download.wait", {
      guid: "download-1",
      timeoutMs: 500,
    })
    expect(waited.envelope.data).toMatchObject({
      download: { guid: "download-1", path: "C:\\managed\\result.txt" },
    })

    const cdpCalls = bridge.commands.filter(
      (command) => command.command === "cdp"
    )
    expect(cdpCalls.length).toBeGreaterThan(0)
    expect(cdpCalls).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          connectionId: "session-a",
          tabId: "tab-1",
          generation: 1,
        }),
      ])
    )
    expect(
      bridge.commands.some(
        (command) =>
          command.command === "create" && command.connectionId === "session-a"
      )
    ).toBe(true)
  })

  it("blocks an initial private target before sending native navigation", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})

    const opened = await runtime.callTool("session-a", "tabs.open", {
      url: "http://127.0.0.1/private",
    })

    expect(opened.envelope).toMatchObject({
      ok: false,
      error: { code: "SSRF_BLOCKED", retryable: false },
    })
    expect(
      bridge.commands.some((command) => command.command === "navigate")
    ).toBe(false)
    expect(bridge.snapshots.get("session-a")?.tabs).toMatchObject([
      { url: "about:blank" },
    ])
  })

  it("maps stale generations to a retryable session refresh", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})
    await runtime.callTool("session-a", "page.snapshot", {})
    bridge.forcedError = {
      code: "NATIVE_GENERATION_MISMATCH",
      message: "stale generation",
      retryable: true,
    }

    const result = await runtime.callTool("session-a", "page.snapshot", {})
    expect(result.envelope).toMatchObject({
      ok: false,
      error: { code: "SESSION_STALE", retryable: true, recovery: "retry" },
    })
  })

  it("maps ownership rejection without leaking another session's tab", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})
    await runtime.callTool("session-a", "tabs.open", {})
    bridge.forcedError = {
      code: "NATIVE_TAB_NOT_FOUND",
      message: "not owned",
      retryable: false,
    }

    const result = await runtime.callTool("session-a", "tabs.focus", {
      tabId: "tab-1",
    })
    expect(result.envelope).toMatchObject({
      ok: false,
      error: { code: "TAB_NOT_FOUND", retryable: false, recovery: "retry" },
    })
  })

  it("maps a WebView2 process failure to runtime recovery", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})
    await runtime.callTool("session-a", "tabs.open", {})
    bridge.markCrashed("session-a")

    const result = await runtime.callTool("session-a", "page.snapshot", {})
    expect(result.envelope).toMatchObject({
      ok: false,
      error: {
        code: "BROWSER_CRASHED",
        retryable: true,
        recovery: "recover_runtime",
      },
    })

    const recovered = await runtime.callTool("session-a", "runtime.recover", {})
    expect(recovered.envelope.ok).toBe(true)
    expect(
      (await runtime.callTool("session-a", "page.snapshot", {})).envelope.ok
    ).toBe(true)
  })

  it("maps bridge disconnects to runtime recovery", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})
    await runtime.callTool("session-a", "tabs.open", {})
    await bridge.stop()
    bridges.splice(bridges.indexOf(bridge), 1)

    const result = await runtime.callTool("session-a", "page.snapshot", {})
    expect(result.envelope).toMatchObject({
      ok: false,
      error: {
        code: "RUNTIME_NOT_READY",
        retryable: true,
        recovery: "recover_runtime",
      },
    })
  })

  it("preserves Rust tabs across sidecar shutdown and releases them on runtime stop", async () => {
    const { bridge, runtime } = await createRuntime()
    await runtime.callTool("session-a", "runtime.start", {})
    await runtime.callTool("session-a", "tabs.open", {})

    await runtime.shutdown()
    expect(bridge.snapshots.has("session-a")).toBe(true)

    await runtime.callTool("session-a", "runtime.start", {})
    const rebound = await runtime.callTool("session-a", "tabs.list", {})
    expect(rebound.envelope.data).toMatchObject({ tabs: [{ id: "tab-1" }] })
    await runtime.callTool("session-a", "runtime.stop", {})
    expect(bridge.snapshots.has("session-a")).toBe(false)
  })
})

async function createRuntime(): Promise<{
  bridge: MockNativeBridge
  runtime: BrowserRuntime
}> {
  const bridge = new MockNativeBridge()
  bridges.push(bridge)
  const endpoint = await bridge.start()
  const backend = new NativeWebViewBackend({
    endpoint,
    token: bridge.token,
  })
  return { bridge, runtime: new BrowserRuntime(backend) }
}

function nativeTab(
  id: string,
  generation: number,
  url = "about:blank"
): NativeTab {
  return {
    id,
    generation,
    url,
    title: "New Tab",
    loading: false,
    canGoBack: false,
    canGoForward: false,
    state: "ready",
    errorCode: null,
  }
}

function cdpResult(method: string, params: unknown): unknown {
  if (method === "Accessibility.getFullAXTree") {
    return {
      nodes: [
        {
          backendDOMNodeId: 11,
          role: { value: "button" },
          name: { value: "Continue" },
        },
      ],
    }
  }
  if (method === "DOM.resolveNode") return { object: { objectId: "object-1" } }
  if (method === "Runtime.callFunctionOn") {
    const declaration = String(
      (params as Record<string, unknown> | undefined)?.functionDeclaration ?? ""
    )
    return {
      result: {
        value: declaration.includes("getBoundingClientRect")
          ? { x: 10, y: 20 }
          : true,
      },
    }
  }
  if (method === "Runtime.evaluate") {
    return { result: { value: "complete" } }
  }
  if (method === "Page.getLayoutMetrics") {
    return { contentSize: { width: 800, height: 600 } }
  }
  if (method === "Page.captureScreenshot") return { data: "AA==" }
  return {}
}

async function readJson(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = []
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown
}
