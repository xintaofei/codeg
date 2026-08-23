import { mkdir, realpath } from "node:fs/promises"
import { basename, join, resolve } from "node:path"

import { CdpConnection, connectCdp } from "./cdp.js"
import { DEFAULT_TOOL_TIMEOUT_MS } from "./contracts.js"
import { ManagedChromeProcess, discoverBrowserPath } from "./chrome-process.js"
import { BrowserError, asBrowserError } from "./errors.js"
import { CdpPageAutomation } from "./page-automation.js"
import {
  assertControlledPath,
  sanitizeRedirectHeaders,
  validateNavigationUrl,
  type HeaderEntry,
} from "./security.js"
import type {
  BrowserBackend,
  BrowserDoctorResult,
  BrowserDownload,
  BrowserRuntimeState,
  BrowserRuntimeStatus,
  BrowserSurfaceFrame,
  BrowserSurfaceInput,
  BrowserTab,
  PageController,
  PageSnapshot,
} from "./runtime-types.js"

interface ChromeBackendOptions {
  profileDir: string
  downloadDir: string
  browserPath?: string
  headless?: boolean
}

interface TargetInfo {
  targetId: string
  type: string
  title: string
  url: string
}

type BrowserSurfaceKeyInput = Extract<BrowserSurfaceInput, { kind: "key" }>

interface DownloadRecord extends BrowserDownload {
  sessionId: string | null
  suggestedFilename: string
}

const RECOVERY_BACKOFF_MS = [1_000, 2_000, 4_000] as const

export function browserSurfaceKeyEventParams(
  input: BrowserSurfaceKeyInput
): Record<string, unknown> {
  return {
    type: input.event === "down" ? "keyDown" : "keyUp",
    ...keyDetails(input.key),
    key: bounded(input.key, 128),
    ...(input.code ? { code: bounded(input.code, 128) } : {}),
    text:
      input.event === "down" && input.text
        ? bounded(input.text, 4_096)
        : undefined,
    modifiers: input.modifiers ?? 0,
  }
}

export class ChromeBackend implements BrowserBackend {
  private stateValue: BrowserRuntimeState = "stopped"
  private browserCdp: CdpConnection | null = null
  private browserVersion: string | null = null
  private lastErrorCode: string | null = null
  private recoveryAttempt = 0
  private readonly pages = new Map<string, ChromePageController>()
  private readonly targetOwners = new Map<string, string>()
  private readonly frameOwners = new Map<string, string>()
  private readonly downloadRecords = new Map<string, DownloadRecord>()
  private readonly process: ManagedChromeProcess

  constructor(private readonly options: ChromeBackendOptions) {
    this.process = new ManagedChromeProcess(options)
    this.process.onCrash(() => {
      this.browserCdp = null
      this.browserVersion = null
      this.stateValue = "error"
      this.lastErrorCode = "BROWSER_CRASHED"
      for (const page of this.pages.values())
        page.close().catch(() => undefined)
      this.pages.clear()
    })
  }

  get state(): BrowserRuntimeState {
    return this.stateValue
  }

  status(): BrowserRuntimeStatus {
    const info = this.process.info
    return {
      state: this.stateValue,
      browserPid: info?.pid ?? null,
      browserName: info?.name ?? null,
      browserVersion: this.browserVersion,
      sessionCount: new Set(this.targetOwners.values()).size,
      recoveryAttempt: this.recoveryAttempt,
      lastErrorCode: this.lastErrorCode,
    }
  }

  async doctor(): Promise<BrowserDoctorResult> {
    const checks: BrowserDoctorResult["checks"] = []
    try {
      const executable = await discoverBrowserPath(this.options.browserPath)
      checks.push({ name: "browser", ok: true, detail: basename(executable) })
    } catch {
      checks.push({ name: "browser", ok: false, detail: "not_found" })
    }
    for (const [name, directory] of [
      ["profile", this.options.profileDir],
      ["downloads", this.options.downloadDir],
    ] as const) {
      try {
        await mkdir(directory, { recursive: true })
        await realpath(directory)
        checks.push({ name, ok: true, detail: "writable" })
      } catch {
        checks.push({ name, ok: false, detail: "unavailable" })
      }
    }
    checks.push({
      name: "process",
      ok: this.process.info !== null,
      detail: this.process.info ? "running" : "stopped",
    })
    checks.push({
      name: "cdp",
      ok: this.browserCdp !== null && this.stateValue === "ready",
      detail: this.browserCdp ? "connected" : "disconnected",
    })
    return { ok: checks.every((check) => check.ok), checks }
  }

  async start(): Promise<void> {
    if (this.stateValue === "ready") return
    if (this.stateValue === "starting") {
      throw new BrowserError(
        "RUNTIME_NOT_READY",
        "Browser runtime is still starting",
        {
          retryable: true,
          recovery: "retry",
        }
      )
    }
    this.stateValue = "starting"
    this.lastErrorCode = null
    try {
      const info = await this.process.start()
      const version = await fetchChromeJson<{ webSocketDebuggerUrl?: string }>(
        info.port,
        "/json/version"
      )
      if (!version.webSocketDebuggerUrl) {
        throw new BrowserError(
          "RUNTIME_START_FAILED",
          "Chrome did not publish its browser DevTools endpoint"
        )
      }
      const cdp = await connectCdp(version.webSocketDebuggerUrl)
      this.browserCdp = cdp
      const details = await cdp.call<{ product?: string }>("Browser.getVersion")
      this.browserVersion = details.product ?? null
      await cdp.call("Target.setDiscoverTargets", { discover: true })
      await cdp.call("Browser.setDownloadBehavior", {
        behavior: "allow",
        downloadPath: resolve(this.options.downloadDir),
        eventsEnabled: true,
      })
      cdp.on<{
        guid: string
        frameId?: string
        suggestedFilename?: string
      }>("Browser.downloadWillBegin", (event) => this.onDownloadBegin(event))
      cdp.on<{
        guid: string
        state: "inProgress" | "completed" | "canceled"
        receivedBytes?: number
        totalBytes?: number
      }>("Browser.downloadProgress", (event) => this.onDownloadProgress(event))
      this.stateValue = "ready"
      this.recoveryAttempt = 0
    } catch (error) {
      this.lastErrorCode = asBrowserError(error).code
      this.stateValue = "error"
      await this.process.stop()
      throw error
    }
  }

  async stop(): Promise<void> {
    for (const page of this.pages.values())
      await page.close().catch(() => undefined)
    this.pages.clear()
    this.targetOwners.clear()
    this.frameOwners.clear()
    this.downloadRecords.clear()
    this.browserCdp?.close()
    this.browserCdp = null
    this.browserVersion = null
    await this.process.stop()
    this.stateValue = "stopped"
    this.lastErrorCode = null
    this.recoveryAttempt = 0
  }

  async shutdown(): Promise<void> {
    await this.stop()
  }

  async recover(): Promise<void> {
    let lastError: unknown
    this.stateValue = "recovering"
    for (let index = 0; index < RECOVERY_BACKOFF_MS.length; index += 1) {
      this.recoveryAttempt = index + 1
      await this.process.stop()
      this.browserCdp = null
      await sleep(RECOVERY_BACKOFF_MS[index]!)
      try {
        this.stateValue = "stopped"
        await this.start()
        return
      } catch (error) {
        lastError = error
        this.stateValue = "recovering"
      }
    }
    this.stateValue = "error"
    this.lastErrorCode = asBrowserError(lastError).code
    throw new BrowserError(
      "RUNTIME_START_FAILED",
      "Browser recovery exhausted three attempts",
      { cause: lastError, recovery: "check_settings" }
    )
  }

  async sessionSnapshot(): Promise<null> {
    return null
  }

  async listTargets(): Promise<BrowserTab[]> {
    const cdp = this.requireReady()
    const result = await cdp.call<{ targetInfos: TargetInfo[] }>(
      "Target.getTargets"
    )
    return result.targetInfos
      .filter((target) => target.type === "page")
      .map((target) => ({
        id: target.targetId,
        url: target.url,
        title: target.title,
      }))
  }

  async openTarget(sessionId: string, url?: string): Promise<PageController> {
    const cdp = this.requireReady()
    const targetUrl = url
      ? (await validateNavigationUrl(url)).url
      : "about:blank"
    const result = await cdp.call<{ targetId: string }>("Target.createTarget", {
      url: targetUrl,
      newWindow: false,
      background: false,
    })
    return await this.page(sessionId, result.targetId)
  }

  async page(_sessionId: string, targetId: string): Promise<PageController> {
    this.requireReady()
    const cached = this.pages.get(targetId)
    if (cached) return cached
    const port = this.process.info?.port
    if (!port) throw runtimeNotReady()
    const page = new ChromePageController({
      targetId,
      port,
      getTargetInfo: async () => await this.targetInfo(targetId),
      getOwner: () => this.targetOwners.get(targetId),
      onFrame: (frameId, sessionId) => this.frameOwners.set(frameId, sessionId),
    })
    this.pages.set(targetId, page)
    return page
  }

  async focusTarget(_sessionId: string, targetId: string): Promise<void> {
    await this.requireReady().call("Target.activateTarget", { targetId })
  }

  async closeTarget(_sessionId: string, targetId: string): Promise<void> {
    const page = this.pages.get(targetId)
    await page?.close().catch(() => undefined)
    this.pages.delete(targetId)
    this.targetOwners.delete(targetId)
    const result = await this.requireReady().call<{ success: boolean }>(
      "Target.closeTarget",
      { targetId }
    )
    if (!result.success) {
      throw new BrowserError(
        "TAB_NOT_FOUND",
        "The browser tab could not be closed"
      )
    }
  }

  async downloads(sessionId: string): Promise<BrowserDownload[]> {
    return [...this.downloadRecords.values()]
      .filter((download) => download.sessionId === sessionId)
      .slice(-100)
      .map(withoutSessionId)
  }

  async waitForDownload(
    sessionId: string,
    guid: string | undefined,
    timeoutMs: number
  ): Promise<BrowserDownload> {
    const deadline = Date.now() + timeoutMs
    while (Date.now() <= deadline) {
      const candidates = [...this.downloadRecords.values()].filter(
        (download) => download.sessionId === sessionId
      )
      const download = guid
        ? candidates.find((candidate) => candidate.guid === guid)
        : candidates[candidates.length - 1]
      if (download && download.state !== "inProgress") {
        return withoutSessionId(download)
      }
      await sleep(100)
    }
    throw new BrowserError("TIMEOUT", "Timed out waiting for the download", {
      retryable: true,
      recovery: "retry",
    })
  }

  assignTarget(targetId: string, sessionId: string): void {
    this.targetOwners.set(targetId, sessionId)
  }

  async releaseSession(sessionId: string): Promise<void> {
    const targets = [...this.targetOwners.entries()]
      .filter(([, owner]) => owner === sessionId)
      .map(([targetId]) => targetId)
    for (const targetId of targets) {
      await this.closeTarget(sessionId, targetId).catch(() => undefined)
    }
    for (const [frameId, owner] of this.frameOwners) {
      if (owner === sessionId) this.frameOwners.delete(frameId)
    }
    for (const [guid, download] of this.downloadRecords) {
      if (download.sessionId === sessionId) this.downloadRecords.delete(guid)
    }
  }

  private requireReady(): CdpConnection {
    if (this.stateValue !== "ready" || !this.browserCdp) throw runtimeNotReady()
    return this.browserCdp
  }

  private async targetInfo(targetId: string): Promise<BrowserTab> {
    const result = await this.requireReady().call<{ targetInfo: TargetInfo }>(
      "Target.getTargetInfo",
      { targetId }
    )
    return {
      id: result.targetInfo.targetId,
      url: result.targetInfo.url,
      title: result.targetInfo.title,
    }
  }

  private onDownloadBegin(event: {
    guid: string
    frameId?: string
    suggestedFilename?: string
  }): void {
    const suggestedFilename = basename(event.suggestedFilename ?? "download")
    this.downloadRecords.set(event.guid, {
      guid: event.guid,
      state: "inProgress",
      filename: suggestedFilename,
      suggestedFilename,
      receivedBytes: 0,
      totalBytes: 0,
      sessionId: event.frameId
        ? (this.frameOwners.get(event.frameId) ?? null)
        : null,
    })
  }

  private onDownloadProgress(event: {
    guid: string
    state: "inProgress" | "completed" | "canceled"
    receivedBytes?: number
    totalBytes?: number
  }): void {
    const record = this.downloadRecords.get(event.guid)
    if (!record) return
    record.state = event.state
    record.receivedBytes = event.receivedBytes ?? record.receivedBytes
    record.totalBytes = event.totalBytes ?? record.totalBytes
    if (event.state === "canceled") record.errorCode = "DOWNLOAD_FAILED"
    if (event.state === "completed") {
      const candidate = join(this.options.downloadDir, record.suggestedFilename)
      void assertControlledPath(this.options.downloadDir, candidate)
        .then((path) => {
          record.path = path
        })
        .catch(() => {
          record.state = "canceled"
          record.errorCode = "PATH_ESCAPE"
        })
    }
  }
}

interface ChromePageOptions {
  targetId: string
  port: number
  getTargetInfo(): Promise<BrowserTab>
  getOwner(): string | undefined
  onFrame(frameId: string, sessionId: string): void
}

class ChromePageController implements PageController {
  private cdp: CdpConnection | null = null
  private initialized = false
  private readonly automation: CdpPageAutomation
  private readonly requestUrls = new Map<string, string>()
  private loading = false
  private screencastActive = false

  constructor(private readonly options: ChromePageOptions) {
    this.automation = new CdpPageAutomation(
      async () => await this.connection(),
      async () => await this.info()
    )
  }

  async info(): Promise<BrowserTab> {
    return await this.options.getTargetInfo()
  }

  async navigate(url: string, timeoutMs: number): Promise<BrowserTab> {
    const safeUrl = (await validateNavigationUrl(url)).url
    const cdp = await this.connection()
    const result = await cdp.call<{ errorText?: string }>(
      "Page.navigate",
      { url: safeUrl },
      { timeoutMs }
    )
    if (result.errorText) {
      throw new BrowserError(
        "INTERNAL_ERROR",
        "The browser rejected the navigation",
        {
          retryable: true,
          recovery: "retry",
        }
      )
    }
    await this.waitForReadyState(timeoutMs)
    return await this.info()
  }

  async snapshot(options: {
    interactive: boolean
    limit: number
    timeoutMs: number
  }): Promise<PageSnapshot> {
    return await this.automation.snapshot(options)
  }

  async screenshot(options: {
    format: "png" | "jpeg"
    fullPage: boolean
    quality?: number
    timeoutMs: number
  }): Promise<{ data: string; mimeType: "image/png" | "image/jpeg" }> {
    return await this.automation.screenshot(options)
  }

  async click(ref: string, timeoutMs: number): Promise<void> {
    await this.automation.click(ref, timeoutMs)
  }

  async type(
    ref: string,
    text: string,
    clear: boolean,
    timeoutMs: number
  ): Promise<void> {
    await this.automation.type(ref, text, clear, timeoutMs)
  }

  async press(key: string, timeoutMs: number): Promise<void> {
    await this.automation.press(key, timeoutMs)
  }

  async scroll(
    ref: string | undefined,
    deltaX: number,
    deltaY: number,
    timeoutMs: number
  ): Promise<void> {
    await this.automation.scroll(ref, deltaX, deltaY, timeoutMs)
  }

  async wait(options: {
    milliseconds?: number
    text?: string
    urlIncludes?: string
    timeoutMs: number
  }): Promise<void> {
    await this.automation.wait(options)
  }

  async surfaceState() {
    const cdp = await this.connection()
    const history = await cdp.call<{
      currentIndex: number
      entries: Array<{ id: number }>
    }>("Page.getNavigationHistory")
    return {
      tab: await this.info(),
      loading: this.loading,
      canGoBack: history.currentIndex > 0,
      canGoForward: history.currentIndex + 1 < history.entries.length,
    }
  }

  async goHistory(delta: -1 | 1, timeoutMs: number): Promise<void> {
    const cdp = await this.connection()
    const history = await cdp.call<{
      currentIndex: number
      entries: Array<{ id: number }>
    }>("Page.getNavigationHistory", {}, { timeoutMs })
    const entry = history.entries[history.currentIndex + delta]
    if (!entry) return
    await cdp.call(
      "Page.navigateToHistoryEntry",
      { entryId: entry.id },
      { timeoutMs }
    )
    await this.waitForReadyState(timeoutMs)
  }

  async reload(timeoutMs: number): Promise<void> {
    await (
      await this.connection()
    ).call("Page.reload", { ignoreCache: false }, { timeoutMs })
    await this.waitForReadyState(timeoutMs)
  }

  async stopLoading(timeoutMs: number): Promise<void> {
    await (await this.connection()).call("Page.stopLoading", {}, { timeoutMs })
    this.loading = false
  }

  async dispatchSurfaceInput(
    input: BrowserSurfaceInput,
    timeoutMs: number
  ): Promise<void> {
    const cdp = await this.connection()
    if (input.kind === "text") {
      await cdp.call("Input.insertText", { text: input.text }, { timeoutMs })
      return
    }
    if (input.kind === "key") {
      await cdp.call(
        "Input.dispatchKeyEvent",
        browserSurfaceKeyEventParams(input),
        { timeoutMs }
      )
      return
    }
    const typeByEvent = {
      pressed: "mousePressed",
      released: "mouseReleased",
      moved: "mouseMoved",
      wheel: "mouseWheel",
    } as const
    await cdp.call(
      "Input.dispatchMouseEvent",
      {
        type: typeByEvent[input.event],
        x: finiteCoordinate(input.x),
        y: finiteCoordinate(input.y),
        button: input.button ?? (input.event === "wheel" ? "none" : "left"),
        clickCount:
          input.event === "pressed" || input.event === "released" ? 1 : 0,
        deltaX: input.event === "wheel" ? (input.deltaX ?? 0) : undefined,
        deltaY: input.event === "wheel" ? (input.deltaY ?? 0) : undefined,
        modifiers: input.modifiers ?? 0,
      },
      { timeoutMs }
    )
  }

  async setSurfaceViewport(
    width: number,
    height: number,
    timeoutMs: number
  ): Promise<void> {
    await (
      await this.connection()
    ).call(
      "Emulation.setDeviceMetricsOverride",
      {
        width,
        height,
        deviceScaleFactor: 1,
        mobile: false,
        screenWidth: width,
        screenHeight: height,
      },
      { timeoutMs }
    )
  }

  async startScreencast(
    listener: (frame: BrowserSurfaceFrame) => void
  ): Promise<() => Promise<void>> {
    if (this.screencastActive) {
      throw new BrowserError(
        "SURFACE_ALREADY_ATTACHED",
        "This browser target already has a screencast subscriber"
      )
    }
    const cdp = await this.connection()
    this.screencastActive = true
    const unsubscribe = cdp.on<{
      data: string
      sessionId: number
      metadata?: {
        deviceWidth?: number
        deviceHeight?: number
        pageScaleFactor?: number
      }
    }>("Page.screencastFrame", (event) => {
      void cdp
        .call("Page.screencastFrameAck", { sessionId: event.sessionId })
        .then(() => {
          if (!this.screencastActive) return
          listener({
            targetId: this.options.targetId,
            data: event.data,
            mimeType: "image/jpeg",
            deviceWidth: Math.max(1, event.metadata?.deviceWidth ?? 1),
            deviceHeight: Math.max(1, event.metadata?.deviceHeight ?? 1),
            pageScaleFactor: event.metadata?.pageScaleFactor ?? 1,
          })
        })
        .catch(() => undefined)
    })
    try {
      await cdp.call("Page.startScreencast", {
        format: "jpeg",
        quality: 70,
        maxWidth: 1440,
        maxHeight: 900,
        everyNthFrame: 1,
      })
    } catch (error) {
      unsubscribe()
      this.screencastActive = false
      throw error
    }
    let stopped = false
    return async () => {
      if (stopped) return
      stopped = true
      unsubscribe()
      if (!this.screencastActive) return
      this.screencastActive = false
      await cdp.call("Page.stopScreencast").catch(() => undefined)
    }
  }

  async close(): Promise<void> {
    this.screencastActive = false
    this.cdp?.close()
    this.cdp = null
    this.initialized = false
    this.automation.clear()
    this.requestUrls.clear()
  }

  private async connection(): Promise<CdpConnection> {
    if (!this.cdp) {
      this.cdp = await connectCdp(
        `ws://127.0.0.1:${this.options.port}/devtools/page/${encodeURIComponent(this.options.targetId)}`
      )
    }
    if (!this.initialized) {
      await Promise.all([
        this.cdp.call("Page.enable"),
        this.cdp.call("DOM.enable"),
        this.cdp.call("Runtime.enable"),
        this.cdp.call("Accessibility.enable"),
      ])
      this.cdp.on<{
        requestId: string
        networkId?: string
        request: { url: string; headers?: Record<string, string> }
      }>("Fetch.requestPaused", (event) => {
        void this.handlePausedRequest(event)
      })
      this.cdp.on<{ frame?: { id?: string } }>(
        "Page.frameNavigated",
        (event) => {
          const owner = this.options.getOwner()
          if (owner && event.frame?.id)
            this.options.onFrame(event.frame.id, owner)
        }
      )
      this.cdp.on("Page.frameStartedLoading", () => {
        this.loading = true
      })
      this.cdp.on("Page.frameStoppedLoading", () => {
        this.loading = false
      })
      await this.cdp.call("Fetch.enable", {
        patterns: [
          { urlPattern: "http://*/*", requestStage: "Request" },
          { urlPattern: "https://*/*", requestStage: "Request" },
        ],
        handleAuthRequests: false,
      })
      const frameTree = await this.cdp.call<{
        frameTree?: { frame?: { id?: string } }
      }>("Page.getFrameTree")
      const owner = this.options.getOwner()
      if (owner && frameTree.frameTree?.frame?.id) {
        this.options.onFrame(frameTree.frameTree.frame.id, owner)
      }
      this.initialized = true
    }
    return this.cdp
  }

  private async handlePausedRequest(event: {
    requestId: string
    networkId?: string
    request: { url: string; headers?: Record<string, string> }
  }): Promise<void> {
    const cdp = this.cdp
    if (!cdp) return
    try {
      const safe = await validateNavigationUrl(event.request.url)
      const previous = event.networkId
        ? this.requestUrls.get(event.networkId)
        : undefined
      const originalHeaders: HeaderEntry[] = Object.entries(
        event.request.headers ?? {}
      ).map(([name, value]) => ({ name, value }))
      const headers = previous
        ? sanitizeRedirectHeaders(previous, safe.url, originalHeaders)
        : originalHeaders
      if (event.networkId) this.requestUrls.set(event.networkId, safe.url)
      await cdp.call("Fetch.continueRequest", {
        requestId: event.requestId,
        headers,
      })
    } catch {
      await cdp
        .call("Fetch.failRequest", {
          requestId: event.requestId,
          errorReason: "Aborted",
        })
        .catch(() => undefined)
    }
  }

  private async waitForReadyState(timeoutMs: number): Promise<void> {
    await this.automation.waitForReadyState(timeoutMs)
  }
}

function finiteCoordinate(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.min(100_000, value)) : 0
}

function bounded(value: string, maximum: number): string {
  return value.length <= maximum ? value : value.slice(0, maximum)
}

function keyDetails(key: string): Record<string, unknown> {
  const named: Record<string, { code: string; keyCode: number }> = {
    Control: { code: "ControlLeft", keyCode: 17 },
    Shift: { code: "ShiftLeft", keyCode: 16 },
    Alt: { code: "AltLeft", keyCode: 18 },
    Meta: { code: "MetaLeft", keyCode: 91 },
    Enter: { code: "Enter", keyCode: 13 },
    Tab: { code: "Tab", keyCode: 9 },
    Escape: { code: "Escape", keyCode: 27 },
    Backspace: { code: "Backspace", keyCode: 8 },
    Delete: { code: "Delete", keyCode: 46 },
    ArrowUp: { code: "ArrowUp", keyCode: 38 },
    ArrowDown: { code: "ArrowDown", keyCode: 40 },
    ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
    ArrowRight: { code: "ArrowRight", keyCode: 39 },
    Home: { code: "Home", keyCode: 36 },
    End: { code: "End", keyCode: 35 },
    PageUp: { code: "PageUp", keyCode: 33 },
    PageDown: { code: "PageDown", keyCode: 34 },
  }
  const match = named[key]
  if (match) {
    return {
      key,
      code: match.code,
      windowsVirtualKeyCode: match.keyCode,
      nativeVirtualKeyCode: match.keyCode,
    }
  }
  const character = [...key][0] ?? ""
  return {
    key: character,
    code: character.length === 1 ? `Key${character.toUpperCase()}` : character,
    text: character,
    windowsVirtualKeyCode: character.toUpperCase().charCodeAt(0),
    nativeVirtualKeyCode: character.toUpperCase().charCodeAt(0),
  }
}

function withoutSessionId(download: DownloadRecord): BrowserDownload {
  const { sessionId, suggestedFilename, ...result } = download
  void sessionId
  void suggestedFilename
  return { ...result }
}

function runtimeNotReady(): BrowserError {
  return new BrowserError("RUNTIME_NOT_READY", "Browser runtime is not ready", {
    retryable: true,
    recovery: "recover_runtime",
  })
}

async function fetchChromeJson<T>(port: number, path: string): Promise<T> {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), DEFAULT_TOOL_TIMEOUT_MS)
  try {
    const response = await fetch(`http://127.0.0.1:${port}${path}`, {
      signal: controller.signal,
      headers: { Accept: "application/json" },
    })
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    return (await response.json()) as T
  } finally {
    clearTimeout(timer)
  }
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolveDone) => setTimeout(resolveDone, milliseconds))
}
