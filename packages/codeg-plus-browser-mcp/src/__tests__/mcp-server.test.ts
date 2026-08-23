import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import { afterEach, describe, expect, it } from "vitest"

import { BrowserRuntime } from "../browser-runtime.js"
import { BrowserSidecarServer } from "../mcp-server.js"
import type {
  BrowserBackend,
  BrowserDoctorResult,
  BrowserDownload,
  BrowserRuntimeState,
  BrowserRuntimeStatus,
  PageSnapshot,
  BrowserTab,
  PageController,
} from "../runtime-types.js"

class SurfacePage implements PageController {
  readonly viewportRequests: Array<{ width: number; height: number }> = []

  constructor(readonly tab: BrowserTab) {}
  async info() {
    return this.tab
  }
  async navigate(url: string) {
    this.tab.url = url
    return this.tab
  }
  async snapshot(): Promise<PageSnapshot> {
    return { nodes: [], documentUrl: this.tab.url, title: this.tab.title }
  }
  async screenshot() {
    return { data: "AA==", mimeType: "image/png" as const }
  }
  async click() {}
  async type() {}
  async press() {}
  async scroll() {}
  async wait() {}
  async surfaceState() {
    return {
      tab: this.tab,
      loading: false,
      canGoBack: false,
      canGoForward: false,
    }
  }
  async goHistory() {}
  async reload() {}
  async stopLoading() {}
  async dispatchSurfaceInput() {}
  async setSurfaceViewport(width: number, height: number) {
    this.viewportRequests.push({ width, height })
  }
  async startScreencast() {
    return async () => undefined
  }
  async close() {}
}

class StatusOnlyBackend implements BrowserBackend {
  state: BrowserRuntimeState = "stopped"
  readonly pages = new Map<string, SurfacePage>()
  readonly owners = new Map<string, string>()
  private nextId = 1

  status(): BrowserRuntimeStatus {
    return {
      state: this.state,
      browserPid: null,
      browserName: null,
      browserVersion: null,
      sessionCount: 0,
      recoveryAttempt: 0,
      lastErrorCode: null,
    }
  }

  async doctor(): Promise<BrowserDoctorResult> {
    return { ok: false, checks: [] }
  }

  async start(): Promise<void> {
    this.state = "ready"
  }

  async stop(): Promise<void> {
    this.state = "stopped"
    this.pages.clear()
    this.owners.clear()
  }

  async shutdown(): Promise<void> {
    await this.stop()
  }

  async recover(): Promise<void> {
    this.state = "ready"
  }

  async sessionSnapshot(): Promise<null> {
    return null
  }

  async listTargets(): Promise<BrowserTab[]> {
    return [...this.pages.values()].map((page) => page.tab)
  }

  async openTarget(): Promise<PageController> {
    const id = `t${this.nextId++}`
    const page = new SurfacePage({ id, url: "about:blank", title: "New Tab" })
    this.pages.set(id, page)
    return page
  }

  async page(_sessionId: string, targetId: string): Promise<PageController> {
    const page = this.pages.get(targetId)
    if (!page) throw new Error("missing page")
    return page
  }

  async focusTarget(): Promise<void> {}
  async closeTarget(_sessionId: string, targetId: string): Promise<void> {
    this.pages.delete(targetId)
    this.owners.delete(targetId)
  }
  async downloads(): Promise<BrowserDownload[]> {
    return []
  }
  async waitForDownload(): Promise<BrowserDownload> {
    throw new Error("not used")
  }
  assignTarget(targetId: string, sessionId: string): void {
    this.owners.set(targetId, sessionId)
  }
  async releaseSession(sessionId: string): Promise<void> {
    for (const [targetId, owner] of [...this.owners]) {
      if (owner === sessionId) await this.closeTarget(sessionId, targetId)
    }
  }
}

const servers: BrowserSidecarServer[] = []
const clients: Client[] = []

afterEach(async () => {
  for (const client of clients.splice(0))
    await client.close().catch(() => undefined)
  for (const server of servers.splice(0))
    await server.stop().catch(() => undefined)
})

async function createTestServer() {
  const token = "t".repeat(32)
  const server = new BrowserSidecarServer({
    runtime: new BrowserRuntime(new StatusOnlyBackend()),
    token,
  })
  const port = await server.start()
  servers.push(server)
  return { server, token, url: `http://127.0.0.1:${port}` }
}

async function createSurfaceServer() {
  const token = "s".repeat(32)
  const backend = new StatusOnlyBackend()
  backend.state = "ready"
  const server = new BrowserSidecarServer({
    runtime: new BrowserRuntime(backend),
    token,
  })
  const port = await server.start()
  servers.push(server)
  return { backend, token, url: `http://127.0.0.1:${port}` }
}

async function postMcp(
  url: string,
  token: string,
  sessionId: string,
  body: Record<string, unknown>
) {
  return await fetch(`${url}/mcp`, {
    method: "POST",
    headers: {
      Accept: "application/json, text/event-stream",
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      "X-Codeg-Browser-Session": sessionId,
    },
    body: JSON.stringify(body),
  })
}

describe("BrowserSidecarServer", () => {
  it("rejects requests without the private bearer token", async () => {
    const { url } = await createTestServer()
    const response = await fetch(`${url}/health`)
    expect(response.status).toBe(401)
    await expect(response.json()).resolves.toEqual({ error: "unauthorized" })
  })

  it("requires an Agent session header for MCP", async () => {
    const { token, url } = await createTestServer()
    const response = await fetch(`${url}/mcp`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
    })
    expect(response.status).toBe(400)
    await expect(response.json()).resolves.toEqual({
      error: "invalid_agent_session",
    })
  })

  it("serves exactly the frozen tool catalog over authenticated MCP", async () => {
    const { token, url } = await createTestServer()
    const client = new Client({ name: "test-client", version: "1" })
    clients.push(client)
    const transport = new StreamableHTTPClientTransport(new URL(`${url}/mcp`), {
      requestInit: {
        headers: {
          Authorization: `Bearer ${token}`,
          "X-Codeg-Browser-Session": "session-a",
        },
      },
    })
    await client.connect(transport)
    const tools = await client.listTools()
    expect(tools.tools).toHaveLength(20)
    expect(tools.tools.map((tool) => tool.name)).toContain("page.screenshot")
    expect(tools.tools.map((tool) => tool.name)).not.toContain("page.evaluate")

    const result = await client.callTool({
      name: "runtime.status",
      arguments: {},
    })
    expect(result.isError).toBe(false)
    expect(result.content).toEqual([
      expect.objectContaining({
        type: "text",
        text: expect.stringContaining('"state":"stopped"'),
      }),
    ])
  })

  it("accepts a tool call when the Agent client does not retain MCP initialization state", async () => {
    const { token, url } = await createTestServer()
    const response = await postMcp(url, token, "session-a", {
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: { name: "runtime.status", arguments: {} },
    })

    expect(response.status).toBe(200)
    await expect(response.json()).resolves.toMatchObject({
      jsonrpc: "2.0",
      id: 1,
      result: {
        isError: false,
        content: [
          {
            type: "text",
            text: expect.stringContaining('"state":"stopped"'),
          },
        ],
      },
    })
  })

  it("keeps MCP tools usable after releasing Browser resources for a live Agent", async () => {
    const { token, url } = await createTestServer()
    const client = new Client({ name: "test-client", version: "1" })
    clients.push(client)
    const transport = new StreamableHTTPClientTransport(new URL(`${url}/mcp`), {
      requestInit: {
        headers: {
          Authorization: `Bearer ${token}`,
          "X-Codeg-Browser-Session": "session-a",
        },
      },
    })
    await client.connect(transport)

    const released = await fetch(`${url}/admin/release-session`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ sessionId: "session-a" }),
    })
    expect(released.status).toBe(200)

    const result = await client.callTool({
      name: "runtime.status",
      arguments: {},
    })
    expect(result.isError).toBe(false)
    expect(result.content).toEqual([
      expect.objectContaining({
        type: "text",
        text: expect.stringContaining('"state":"stopped"'),
      }),
    ])
  })

  it("handles concurrent first tool calls for the same Agent session", async () => {
    const { token, url } = await createTestServer()
    const responses = await Promise.all(
      [1, 2, 3].map((id) =>
        postMcp(url, token, "session-a", {
          jsonrpc: "2.0",
          id,
          method: "tools/call",
          params: { name: "runtime.status", arguments: {} },
        })
      )
    )

    expect(responses.map((response) => response.status)).toEqual([
      200, 200, 200,
    ])
    const bodies = await Promise.all(
      responses.map((response) => response.json())
    )
    expect(bodies.map((body) => body.id)).toEqual([1, 2, 3])
    expect(bodies.every((body) => body.result?.isError === false)).toBe(true)
  })

  it("serves a complete per-session surface snapshot and releases all targets", async () => {
    const { backend, token, url } = await createSurfaceServer()
    const headers = {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    }
    const first = await fetch(`${url}/admin/surface/ensure`, {
      method: "POST",
      headers,
      body: JSON.stringify({ sessionId: "session-a" }),
    })
    expect(first.status).toBe(200)
    await expect(first.json()).resolves.toMatchObject({
      sessionId: "session-a",
      activeTargetId: "t1",
      tabs: [{ id: "t1" }],
    })

    const second = await fetch(`${url}/admin/surface/ensure`, {
      method: "POST",
      headers,
      body: JSON.stringify({ sessionId: "session-a" }),
    })
    await expect(second.json()).resolves.toMatchObject({
      activeTargetId: "t1",
      tabs: [{ id: "t1" }],
    })
    expect(backend.pages.size).toBe(1)

    const resized = await fetch(`${url}/admin/surface/action`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        sessionId: "session-a",
        action: "resize",
        width: 320,
        height: 620,
      }),
    })
    expect(resized.status).toBe(200)
    expect(backend.pages.get("t1")?.viewportRequests).toEqual([
      { width: 320, height: 620 },
    ])

    const invalidResize = await fetch(`${url}/admin/surface/action`, {
      method: "POST",
      headers,
      body: JSON.stringify({
        sessionId: "session-a",
        action: "resize",
        width: 0,
        height: 620,
      }),
    })
    expect(invalidResize.status).toBe(400)

    const released = await fetch(`${url}/admin/release-session`, {
      method: "POST",
      headers,
      body: JSON.stringify({ sessionId: "session-a" }),
    })
    expect(released.status).toBe(200)
    expect(backend.pages.size).toBe(0)
  })

  it("allows only one bounded screencast stream per Agent session", async () => {
    const { token, url } = await createSurfaceServer()
    const controller = new AbortController()
    const first = await fetch(
      `${url}/admin/surface/stream?sessionId=session-a`,
      {
        headers: { Authorization: `Bearer ${token}` },
        signal: controller.signal,
      }
    )
    expect(first.status).toBe(200)
    expect(first.headers.get("content-type")).toContain("application/x-ndjson")

    const second = await fetch(
      `${url}/admin/surface/stream?sessionId=session-a`,
      { headers: { Authorization: `Bearer ${token}` } }
    )
    expect(second.status).toBe(409)
    await expect(second.json()).resolves.toEqual({
      error: "surface_already_attached",
    })
    controller.abort()
  })
})
