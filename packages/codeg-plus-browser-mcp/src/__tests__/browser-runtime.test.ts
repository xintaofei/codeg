import { describe, expect, it } from "vitest"

import { BrowserRuntime } from "../browser-runtime.js"
import type {
  BrowserBackend,
  BrowserDoctorResult,
  BrowserDownload,
  BrowserRuntimeState,
  BrowserRuntimeStatus,
  BrowserSurfaceFrame,
  BrowserTab,
  PageController,
  PageSnapshot,
} from "../runtime-types.js"

class FakePage implements PageController {
  loading = false
  historyIndex = 0
  history = ["about:blank"]
  screencastStarts = 0
  screencastStops = 0
  readonly viewportRequests: Array<{ width: number; height: number }> = []
  private frameListener: ((frame: BrowserSurfaceFrame) => void) | null = null

  constructor(readonly tab: BrowserTab) {}

  async info(): Promise<BrowserTab> {
    return this.tab
  }

  async navigate(url: string): Promise<BrowserTab> {
    this.tab.url = url
    this.tab.title = "Navigated"
    this.history = this.history.slice(0, this.historyIndex + 1)
    this.history.push(url)
    this.historyIndex = this.history.length - 1
    return this.tab
  }

  async snapshot(): Promise<PageSnapshot> {
    return {
      nodes: [{ ref: "e1", role: "button", name: "Continue" }],
      documentUrl: this.tab.url,
      title: this.tab.title,
    }
  }

  async screenshot() {
    return { data: "AA==", mimeType: "image/png" as const }
  }

  async click(): Promise<void> {}
  async type(): Promise<void> {}
  async press(): Promise<void> {}
  async scroll(): Promise<void> {}
  async wait(): Promise<void> {}
  async surfaceState() {
    return {
      tab: this.tab,
      loading: this.loading,
      canGoBack: this.historyIndex > 0,
      canGoForward: this.historyIndex + 1 < this.history.length,
    }
  }
  async goHistory(delta: -1 | 1): Promise<void> {
    this.historyIndex += delta
    this.tab.url = this.history[this.historyIndex]!
  }
  async reload(): Promise<void> {}
  async stopLoading(): Promise<void> {
    this.loading = false
  }
  async dispatchSurfaceInput(): Promise<void> {}
  async setSurfaceViewport(width: number, height: number): Promise<void> {
    this.viewportRequests.push({ width, height })
  }
  async startScreencast(listener: (frame: BrowserSurfaceFrame) => void) {
    this.screencastStarts += 1
    this.frameListener = listener
    return async () => {
      this.screencastStops += 1
      this.frameListener = null
    }
  }
  emitFrame() {
    this.frameListener?.({
      targetId: this.tab.id,
      data: "AA==",
      mimeType: "image/jpeg",
      deviceWidth: 800,
      deviceHeight: 600,
      pageScaleFactor: 1,
    })
  }
  async close(): Promise<void> {}
}

class FakeBackend implements BrowserBackend {
  state: BrowserRuntimeState = "ready"
  readonly pages = new Map<string, FakePage>()
  readonly owners = new Map<string, string>()
  readonly openTargetUrls: Array<string | undefined> = []
  private nextId = 1

  status(): BrowserRuntimeStatus {
    return {
      state: this.state,
      browserPid: 123,
      browserName: "Fake Chrome",
      browserVersion: "1",
      sessionCount: new Set(this.owners.values()).size,
      recoveryAttempt: 0,
      lastErrorCode: null,
    }
  }

  async doctor(): Promise<BrowserDoctorResult> {
    return { ok: true, checks: [] }
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

  async openTarget(_sessionId: string, url?: string): Promise<PageController> {
    this.openTargetUrls.push(url)
    const id = `t${this.nextId++}`
    const page = new FakePage({
      id,
      url: url ?? "about:blank",
      title: "New Tab",
    })
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
    return {
      guid: "d1",
      state: "completed",
      filename: "file.txt",
      receivedBytes: 1,
      totalBytes: 1,
    }
  }

  assignTarget(targetId: string, sessionId: string): void {
    this.owners.set(targetId, sessionId)
  }

  async releaseSession(sessionId: string): Promise<void> {
    for (const [targetId, owner] of this.owners) {
      if (owner === sessionId) await this.closeTarget(sessionId, targetId)
    }
  }
}

describe("BrowserRuntime", () => {
  it("keeps tabs isolated by Agent session", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)
    const first = await runtime.callTool("session-a", "tabs.open", {
      url: "https://example.com/",
    })
    const second = await runtime.callTool("session-b", "tabs.open", {
      url: "https://example.org/",
    })
    const firstId = first.envelope.tab?.id
    expect(firstId).toBeTruthy()
    expect(second.envelope.tab?.id).not.toBe(firstId)

    const firstList = await runtime.callTool("session-a", "tabs.list", {})
    expect(firstList.envelope.data).toMatchObject({
      tabs: [{ id: firstId }],
    })
    const crossSession = await runtime.callTool("session-b", "tabs.focus", {
      tabId: firstId,
    })
    expect(crossSession.envelope).toMatchObject({
      ok: false,
      error: { code: "TAB_NOT_FOUND" },
    })
  })

  it("attaches interception on a blank target before navigating an opened URL", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)
    const result = await runtime.callTool("session-a", "tabs.open", {
      url: "https://example.com/",
    })

    expect(backend.openTargetUrls).toEqual([undefined])
    expect(result.envelope.tab?.url).toBe("https://example.com/")
    expect(backend.owners.get(result.envelope.tab?.id ?? "")).toBe("session-a")
  })

  it("releases only the selected session's owned targets", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)
    await runtime.callTool("session-a", "tabs.open", {})
    await runtime.callTool("session-b", "tabs.open", {})

    await runtime.releaseSession("session-a")
    expect([...backend.owners.values()]).toEqual(["session-b"])
  })

  it("fails closed while the managed runtime is stopped", async () => {
    const backend = new FakeBackend()
    backend.state = "stopped"
    const runtime = new BrowserRuntime(backend)
    const result = await runtime.callTool("session-a", "page.snapshot", {})
    expect(result.envelope).toMatchObject({
      ok: false,
      error: { code: "RUNTIME_NOT_READY", recovery: "recover_runtime" },
    })
  })

  it("does not expose capabilities outside the frozen tool catalog", async () => {
    const runtime = new BrowserRuntime(new FakeBackend())
    const result = await runtime.callTool("session-a", "page.evaluate", {
      expression: "document.cookie",
    })
    expect(result.envelope).toMatchObject({
      ok: false,
      error: { code: "UNKNOWN_TOOL" },
    })
  })

  it("evicts stale sessions and their tabs", async () => {
    let now = 1_000
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend, {
      sessionTtlMs: 100,
      now: () => now,
    })
    await runtime.callTool("session-a", "tabs.open", {})
    now += 101
    await runtime.callTool("session-b", "runtime.status", {})
    expect([...backend.owners.values()]).toEqual([])
  })

  it("ensures and reuses one surface target for an Agent session", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)

    const first = await runtime.ensureSurface("session-a")
    const second = await runtime.ensureSurface("session-a")

    expect(first.activeTargetId).toBe("t1")
    expect(second.activeTargetId).toBe("t1")
    expect(second.tabs).toHaveLength(1)
    expect(backend.openTargetUrls).toEqual([undefined])
  })

  it("streams only the active target and publishes complete snapshots", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)
    const events: Array<{ type: string; [key: string]: unknown }> = []
    const detach = await runtime.subscribeSurface("session-a", (event) => {
      events.push(event)
    })
    const first = backend.pages.get("t1")!
    first.emitFrame()

    await runtime.surfaceAction("session-a", { action: "open" })
    const second = backend.pages.get("t2")!
    second.emitFrame()

    expect(first.screencastStops).toBe(1)
    expect(second.screencastStarts).toBe(1)
    expect(events.filter((event) => event.type === "frame")).toHaveLength(2)
    expect(events[events.length - 2]).toMatchObject({
      type: "snapshot",
      snapshot: { activeTargetId: "t2", tabs: [{ id: "t1" }, { id: "t2" }] },
    })

    await detach()
    expect(second.screencastStops).toBe(1)
  })

  it("resizes the real page viewport and reuses it for newly active targets", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)

    await runtime.surfaceAction("session-a", {
      action: "resize",
      width: 320,
      height: 620,
    })
    expect(backend.pages.get("t1")?.viewportRequests).toEqual([
      { width: 320, height: 620 },
    ])

    const detach = await runtime.subscribeSurface("session-a", () => undefined)
    await runtime.surfaceAction("session-a", { action: "open" })
    expect(backend.pages.get("t2")?.viewportRequests).toEqual([
      { width: 320, height: 620 },
    ])
    await detach()
  })

  it("rejects a second screencast subscriber for the same session", async () => {
    const runtime = new BrowserRuntime(new FakeBackend())
    const detach = await runtime.subscribeSurface("session-a", () => undefined)

    await expect(
      runtime.subscribeSurface("session-a", () => undefined)
    ).rejects.toMatchObject({ code: "SURFACE_ALREADY_ATTACHED" })

    await detach()
  })

  it("explicit release detaches the surface and closes all owned targets", async () => {
    const backend = new FakeBackend()
    const runtime = new BrowserRuntime(backend)
    await runtime.subscribeSurface("session-a", () => undefined)
    await runtime.surfaceAction("session-a", { action: "open" })

    await runtime.releaseSession("session-a")

    expect([...backend.owners.values()]).toEqual([])
    expect(backend.pages.size).toBe(0)
  })
})
