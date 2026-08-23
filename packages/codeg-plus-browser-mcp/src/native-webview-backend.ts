import type { CdpCallOptions } from "./cdp.js"
import { DEFAULT_TOOL_TIMEOUT_MS } from "./contracts.js"
import { BrowserError, asBrowserError } from "./errors.js"
import { CdpPageAutomation, type CdpCallTransport } from "./page-automation.js"
import { validateNavigationUrl } from "./security.js"
import type {
  BrowserBackend,
  BrowserBackendSessionSnapshot,
  BrowserDoctorResult,
  BrowserDownload,
  BrowserRuntimeState,
  BrowserRuntimeStatus,
  BrowserTab,
  PageController,
  PageSnapshot,
} from "./runtime-types.js"

export interface NativeWebViewBackendOptions {
  endpoint: string
  token: string
}

interface NativeBridgeHealth {
  ok: true
  backend: "embedded_webview2"
  protocolVersion: "1"
}

interface NativeBrowserTab extends BrowserTab {
  generation: number
  loading: boolean
  canGoBack: boolean
  canGoForward: boolean
  state: "ready" | "loading" | "error"
  errorCode: string | null
}

interface NativeBrowserSnapshot {
  connectionId: string
  tabs: NativeBrowserTab[]
  activeTabId: string | null
}

interface NativeTabIdentity {
  connectionId: string
  tabId: string
  generation: number
}

interface NativeBridgeFailure {
  code: string
  message: string
  retryable: boolean
}

const NATIVE_BRIDGE_PROTOCOL_VERSION = "1"
const NATIVE_BRIDGE_MAX_RESPONSE_BYTES = 1024 * 1024
const NATIVE_BRIDGE_TIMEOUT_MS = 20_000

export class NativeWebViewBackend implements BrowserBackend {
  private readonly bridge: NativeBridgeClient
  private stateValue: BrowserRuntimeState = "stopped"
  private lastErrorCode: string | null = null
  private recoveryAttempt = 0
  private readonly sessions = new Set<string>()
  private readonly pages = new Map<string, NativePageController>()

  constructor(options: NativeWebViewBackendOptions) {
    this.bridge = new NativeBridgeClient(options)
  }

  get state(): BrowserRuntimeState {
    return this.stateValue
  }

  status(): BrowserRuntimeStatus {
    return {
      state: this.stateValue,
      browserPid: null,
      browserName: "Microsoft Edge WebView2",
      browserVersion: null,
      sessionCount: this.sessions.size,
      recoveryAttempt: this.recoveryAttempt,
      lastErrorCode: this.lastErrorCode,
    }
  }

  async doctor(): Promise<BrowserDoctorResult> {
    try {
      await this.bridge.health()
      return {
        ok: true,
        checks: [
          { name: "browser", ok: true, detail: "webview2" },
          { name: "profile", ok: true, detail: "rust_owned" },
          { name: "downloads", ok: true, detail: "rust_owned" },
          { name: "cdp", ok: true, detail: "native_bridge" },
          { name: "process", ok: true, detail: "host_owned" },
        ],
      }
    } catch (error) {
      const browserError = asBrowserError(error)
      return {
        ok: false,
        checks: [
          { name: "browser", ok: false, detail: browserError.code },
          { name: "profile", ok: true, detail: "rust_owned" },
          { name: "downloads", ok: true, detail: "rust_owned" },
          { name: "cdp", ok: false, detail: browserError.code },
          { name: "process", ok: false, detail: "bridge_unavailable" },
        ],
      }
    }
  }

  async start(): Promise<void> {
    if (this.stateValue === "ready") return
    this.stateValue = "starting"
    this.lastErrorCode = null
    try {
      const health = await this.bridge.health()
      if (
        health.backend !== "embedded_webview2" ||
        health.protocolVersion !== NATIVE_BRIDGE_PROTOCOL_VERSION
      ) {
        throw new BrowserError(
          "RUNTIME_START_FAILED",
          "The native Browser bridge protocol is incompatible",
          { recovery: "check_settings" }
        )
      }
      this.stateValue = "ready"
      this.recoveryAttempt = 0
    } catch (error) {
      const browserError = asBrowserError(error)
      this.stateValue = "error"
      this.lastErrorCode = browserError.code
      throw browserError
    }
  }

  async stop(): Promise<void> {
    for (const sessionId of [...this.sessions]) {
      await this.bridge
        .command({ command: "release", connectionId: sessionId })
        .catch(() => undefined)
    }
    await this.shutdown()
  }

  async shutdown(): Promise<void> {
    for (const page of this.pages.values()) await page.close()
    this.pages.clear()
    this.sessions.clear()
    this.stateValue = "stopped"
    this.lastErrorCode = null
    this.recoveryAttempt = 0
  }

  async recover(): Promise<void> {
    this.stateValue = "recovering"
    this.recoveryAttempt = 1
    try {
      await this.bridge.health()
      for (const sessionId of [...this.sessions]) {
        let snapshot: NativeBrowserSnapshot
        try {
          const value = await this.bridge.command({
            command: "snapshot",
            connectionId: sessionId,
          })
          snapshot = readSnapshotValue(value)
        } catch (error) {
          if (
            error instanceof NativeBridgeRequestError &&
            error.nativeCode === "NATIVE_SESSION_NOT_FOUND"
          ) {
            continue
          }
          throw error
        }
        for (const tab of snapshot.tabs.filter(
          (candidate) => candidate.state === "error"
        )) {
          const value = await this.bridge.command({
            command: "recover",
            connectionId: sessionId,
            tabId: tab.id,
            generation: tab.generation,
          })
          snapshot = readSnapshotValue(value)
        }
        this.acceptSnapshot(sessionId, snapshot)
      }
      for (const page of this.pages.values()) await page.close()
      this.pages.clear()
      this.sessions.clear()
      this.stateValue = "ready"
      this.lastErrorCode = null
      this.recoveryAttempt = 0
    } catch (error) {
      const browserError = asBrowserError(error)
      this.stateValue = "error"
      this.lastErrorCode = browserError.code
      throw browserError
    }
  }

  async sessionSnapshot(
    sessionId: string,
    ensure: boolean
  ): Promise<BrowserBackendSessionSnapshot> {
    this.requireReady()
    try {
      const snapshot = await this.fetchSnapshot(sessionId, ensure)
      return {
        tabs: snapshot.tabs.map(publicTab),
        activeTargetId: snapshot.activeTabId,
      }
    } catch (error) {
      if (
        !ensure &&
        error instanceof NativeBridgeRequestError &&
        error.nativeCode === "NATIVE_SESSION_NOT_FOUND"
      ) {
        this.clearSession(sessionId)
        return { tabs: [], activeTargetId: null }
      }
      throw error
    }
  }

  async listTargets(sessionId: string): Promise<BrowserTab[]> {
    return (await this.sessionSnapshot(sessionId, false)).tabs
  }

  async openTarget(sessionId: string, url?: string): Promise<PageController> {
    this.requireReady()
    const safeUrl = url ? (await validateNavigationUrl(url)).url : undefined
    const value = await this.bridge.command({
      command: "create",
      connectionId: sessionId,
      ...(safeUrl ? { url: safeUrl } : {}),
    })
    const snapshot = this.acceptSnapshot(sessionId, readSnapshotValue(value))
    const tab = snapshot.tabs.find(
      (candidate) => candidate.id === snapshot.activeTabId
    )
    if (!tab) throw tabNotFound()
    return this.controller(sessionId, tab)
  }

  async page(sessionId: string, targetId: string): Promise<PageController> {
    this.requireReady()
    const snapshot = await this.fetchSnapshot(sessionId, false)
    const tab = snapshot.tabs.find((candidate) => candidate.id === targetId)
    if (!tab) throw tabNotFound()
    return this.controller(sessionId, tab)
  }

  async focusTarget(sessionId: string, targetId: string): Promise<void> {
    const identity = await this.identity(sessionId, targetId)
    await this.commandSnapshot(identity, { command: "focus" })
  }

  async closeTarget(sessionId: string, targetId: string): Promise<void> {
    const identity = await this.identity(sessionId, targetId)
    await this.commandSnapshot(identity, { command: "close" })
  }

  async downloads(sessionId: string): Promise<BrowserDownload[]> {
    this.requireReady()
    const value = await this.bridge.command({
      command: "downloads",
      connectionId: sessionId,
    })
    const downloads = asRecord(value).downloads
    if (!Array.isArray(downloads)) throw invalidBridgeResponse()
    return downloads.map(readDownload)
  }

  async waitForDownload(
    sessionId: string,
    guid: string | undefined,
    timeoutMs: number
  ): Promise<BrowserDownload> {
    const deadline = Date.now() + timeoutMs
    while (Date.now() <= deadline) {
      const downloads = await this.downloads(sessionId)
      const download = guid
        ? downloads.find((candidate) => candidate.guid === guid)
        : downloads[downloads.length - 1]
      if (download && download.state !== "inProgress") return download
      await sleep(100)
    }
    throw new BrowserError("TIMEOUT", "Timed out waiting for the download", {
      retryable: true,
      recovery: "retry",
    })
  }

  assignTarget(_targetId: string, sessionId: string): void {
    this.sessions.add(sessionId)
  }

  async releaseSession(sessionId: string): Promise<void> {
    try {
      await this.bridge.command({ command: "release", connectionId: sessionId })
    } catch (error) {
      if (
        !(error instanceof NativeBridgeRequestError) ||
        error.nativeCode !== "NATIVE_SESSION_NOT_FOUND"
      ) {
        throw error
      }
    } finally {
      this.clearSession(sessionId)
    }
  }

  async tabInfo(identity: NativeTabIdentity): Promise<NativeBrowserTab> {
    const snapshot = await this.fetchSnapshot(identity.connectionId, false)
    const tab = snapshot.tabs.find(
      (candidate) => candidate.id === identity.tabId
    )
    if (!tab) throw tabNotFound()
    if (tab.generation !== identity.generation) throw staleGeneration()
    if (tab.state === "error") throw crashedTab(tab.errorCode)
    return tab
  }

  async commandSnapshot(
    identity: NativeTabIdentity,
    command:
      | { command: "focus" | "close" | "reload" | "stop" }
      | { command: "navigate"; url: string }
      | { command: "history"; direction: "back" | "forward" }
  ): Promise<NativeBrowserSnapshot> {
    this.requireReady()
    const value = await this.bridge.command({
      ...command,
      connectionId: identity.connectionId,
      tabId: identity.tabId,
      generation: identity.generation,
    })
    return this.acceptSnapshot(identity.connectionId, readSnapshotValue(value))
  }

  async waitForTabReady(
    identity: NativeTabIdentity,
    timeoutMs: number
  ): Promise<NativeBrowserTab> {
    const deadline = Date.now() + timeoutMs
    while (Date.now() <= deadline) {
      const tab = await this.tabInfo(identity)
      if (!tab.loading && tab.state === "ready") return tab
      await sleep(100)
    }
    throw new BrowserError("TIMEOUT", "Page navigation timed out", {
      retryable: true,
      recovery: "retry",
    })
  }

  async callCdp<T>(
    identity: NativeTabIdentity,
    method: string,
    params: Record<string, unknown>,
    options: CdpCallOptions
  ): Promise<T> {
    this.requireReady()
    if (options.signal?.aborted) {
      throw new BrowserError("ABORTED", "Browser operation was aborted", {
        retryable: true,
        recovery: "retry",
      })
    }
    const value = await this.bridge.command(
      {
        command: "cdp",
        connectionId: identity.connectionId,
        tabId: identity.tabId,
        generation: identity.generation,
        method,
        params,
      },
      options.timeoutMs
    )
    const response = asRecord(value)
    if (!("result" in response)) throw invalidBridgeResponse()
    return response.result as T
  }

  private async identity(
    sessionId: string,
    targetId: string
  ): Promise<NativeTabIdentity> {
    const snapshot = await this.fetchSnapshot(sessionId, false)
    const tab = snapshot.tabs.find((candidate) => candidate.id === targetId)
    if (!tab) throw tabNotFound()
    return {
      connectionId: sessionId,
      tabId: targetId,
      generation: tab.generation,
    }
  }

  private async fetchSnapshot(
    sessionId: string,
    ensure: boolean
  ): Promise<NativeBrowserSnapshot> {
    const value = await this.bridge.command({
      command: ensure ? "ensure" : "snapshot",
      connectionId: sessionId,
    })
    return this.acceptSnapshot(sessionId, readSnapshotValue(value))
  }

  private acceptSnapshot(
    sessionId: string,
    snapshot: NativeBrowserSnapshot
  ): NativeBrowserSnapshot {
    if (snapshot.connectionId !== sessionId) throw invalidBridgeResponse()
    this.sessions.add(sessionId)
    const live = new Set(
      snapshot.tabs.map((tab) => pageKey(sessionId, tab.id, tab.generation))
    )
    for (const [key, page] of this.pages) {
      if (key.startsWith(`${sessionId}\0`) && !live.has(key)) {
        void page.close()
        this.pages.delete(key)
      }
    }
    return snapshot
  }

  private controller(
    sessionId: string,
    tab: NativeBrowserTab
  ): NativePageController {
    const key = pageKey(sessionId, tab.id, tab.generation)
    let page = this.pages.get(key)
    if (!page) {
      page = new NativePageController(this, {
        connectionId: sessionId,
        tabId: tab.id,
        generation: tab.generation,
      })
      this.pages.set(key, page)
    }
    return page
  }

  private clearSession(sessionId: string): void {
    this.sessions.delete(sessionId)
    for (const [key, page] of this.pages) {
      if (key.startsWith(`${sessionId}\0`)) {
        void page.close()
        this.pages.delete(key)
      }
    }
  }

  private requireReady(): void {
    if (this.stateValue !== "ready") throw runtimeNotReady()
  }
}

class NativePageController implements PageController {
  private readonly automation: CdpPageAutomation
  private readonly cdp: CdpCallTransport

  constructor(
    private readonly backend: NativeWebViewBackend,
    private readonly identity: NativeTabIdentity
  ) {
    this.cdp = {
      call: async <T = unknown>(
        method: string,
        params: Record<string, unknown> = {},
        options: CdpCallOptions = {}
      ): Promise<T> =>
        await this.backend.callCdp<T>(this.identity, method, params, options),
    }
    this.automation = new CdpPageAutomation(
      async () => this.cdp,
      async () => await this.info()
    )
  }

  async info(): Promise<BrowserTab> {
    return publicTab(await this.backend.tabInfo(this.identity))
  }

  async navigate(url: string, timeoutMs: number): Promise<BrowserTab> {
    const safeUrl = (await validateNavigationUrl(url)).url
    const started = Date.now()
    await this.backend.commandSnapshot(this.identity, {
      command: "navigate",
      url: safeUrl,
    })
    const tab = await this.backend.waitForTabReady(
      this.identity,
      Math.max(100, timeoutMs - (Date.now() - started))
    )
    await this.automation.waitForReadyState(
      Math.max(100, timeoutMs - (Date.now() - started))
    )
    return publicTab(tab)
  }

  async snapshot(options: {
    interactive: boolean
    limit: number
    timeoutMs: number
  }): Promise<PageSnapshot> {
    await this.backend.tabInfo(this.identity)
    return await this.automation.snapshot(options)
  }

  async screenshot(options: {
    format: "png" | "jpeg"
    fullPage: boolean
    quality?: number
    timeoutMs: number
  }): Promise<{ data: string; mimeType: "image/png" | "image/jpeg" }> {
    await this.backend.tabInfo(this.identity)
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
    const tab = await this.backend.tabInfo(this.identity)
    return {
      tab: publicTab(tab),
      loading: tab.loading,
      canGoBack: tab.canGoBack,
      canGoForward: tab.canGoForward,
    }
  }

  async goHistory(delta: -1 | 1, timeoutMs: number): Promise<void> {
    await this.backend.commandSnapshot(this.identity, {
      command: "history",
      direction: delta < 0 ? "back" : "forward",
    })
    await this.backend.waitForTabReady(this.identity, timeoutMs)
  }

  async reload(timeoutMs: number): Promise<void> {
    await this.backend.commandSnapshot(this.identity, { command: "reload" })
    await this.backend.waitForTabReady(this.identity, timeoutMs)
  }

  async stopLoading(): Promise<void> {
    await this.backend.commandSnapshot(this.identity, { command: "stop" })
  }

  async dispatchSurfaceInput(): Promise<void> {
    throw nativeSurfaceOwnedByHost()
  }

  async setSurfaceViewport(): Promise<void> {
    throw nativeSurfaceOwnedByHost()
  }

  async startScreencast(): Promise<() => Promise<void>> {
    throw nativeSurfaceOwnedByHost()
  }

  async close(): Promise<void> {
    this.automation.clear()
  }
}

class NativeBridgeClient {
  private readonly endpoint: string
  private readonly token: string

  constructor(options: NativeWebViewBackendOptions) {
    this.endpoint = validateBridgeEndpoint(options.endpoint)
    if (Buffer.byteLength(options.token, "utf8") < 32) {
      throw new BrowserError(
        "RUNTIME_START_FAILED",
        "The native Browser bridge token is invalid",
        { recovery: "check_settings" }
      )
    }
    this.token = options.token
  }

  async health(): Promise<NativeBridgeHealth> {
    const value = await this.request("GET", "/v1/health")
    const record = asRecord(value)
    if (
      record.ok !== true ||
      record.backend !== "embedded_webview2" ||
      record.protocolVersion !== NATIVE_BRIDGE_PROTOCOL_VERSION
    ) {
      throw invalidBridgeResponse()
    }
    return record as unknown as NativeBridgeHealth
  }

  async command(
    command: Record<string, unknown>,
    timeoutMs = DEFAULT_TOOL_TIMEOUT_MS
  ): Promise<unknown> {
    const value = await this.request("POST", "/v1/command", command, timeoutMs)
    const record = asRecord(value)
    if (record.ok !== true || !("value" in record)) {
      throw invalidBridgeResponse()
    }
    return record.value
  }

  private async request(
    method: "GET" | "POST",
    path: string,
    body?: Record<string, unknown>,
    timeoutMs = DEFAULT_TOOL_TIMEOUT_MS
  ): Promise<unknown> {
    const controller = new AbortController()
    const timer = setTimeout(
      () => controller.abort(),
      Math.min(Math.max(100, timeoutMs), NATIVE_BRIDGE_TIMEOUT_MS)
    )
    try {
      let response: Response
      try {
        response = await fetch(`${this.endpoint}${path}`, {
          method,
          headers: {
            Accept: "application/json",
            Authorization: `Bearer ${this.token}`,
            ...(body ? { "Content-Type": "application/json" } : {}),
          },
          ...(body ? { body: JSON.stringify(body) } : {}),
          signal: controller.signal,
        })
      } catch (error) {
        if (controller.signal.aborted) {
          throw new BrowserError("TIMEOUT", "Native Browser bridge timed out", {
            cause: error,
            retryable: true,
            recovery: "retry",
          })
        }
        throw new BrowserError(
          "RUNTIME_NOT_READY",
          "Native Browser bridge is disconnected",
          { cause: error, retryable: true, recovery: "recover_runtime" }
        )
      }
      const declaredLength = Number(response.headers.get("content-length"))
      if (
        Number.isFinite(declaredLength) &&
        declaredLength > NATIVE_BRIDGE_MAX_RESPONSE_BYTES
      ) {
        throw invalidBridgeResponse()
      }
      const text = await response.text()
      if (Buffer.byteLength(text, "utf8") > NATIVE_BRIDGE_MAX_RESPONSE_BYTES) {
        throw invalidBridgeResponse()
      }
      let value: unknown
      try {
        value = JSON.parse(text) as unknown
      } catch {
        throw invalidBridgeResponse()
      }
      if (!response.ok) {
        const error = readBridgeFailure(value)
        throw mapBridgeFailure(error)
      }
      return value
    } finally {
      clearTimeout(timer)
    }
  }
}

class NativeBridgeRequestError extends BrowserError {
  constructor(
    readonly nativeCode: string,
    code: ConstructorParameters<typeof BrowserError>[0],
    message: string,
    options: ConstructorParameters<typeof BrowserError>[2]
  ) {
    super(code, message, options)
  }
}

function mapBridgeFailure(error: NativeBridgeFailure): BrowserError {
  if (error.code === "NATIVE_GENERATION_MISMATCH") {
    return new NativeBridgeRequestError(
      error.code,
      "SESSION_STALE",
      "The native Browser tab generation changed; refresh the tab snapshot and retry",
      { retryable: true, recovery: "retry" }
    )
  }
  if (
    error.code === "NATIVE_SESSION_NOT_FOUND" ||
    error.code === "NATIVE_TAB_NOT_FOUND"
  ) {
    return new NativeBridgeRequestError(
      error.code,
      "TAB_NOT_FOUND",
      "The native Browser tab is not owned by this Agent session",
      { retryable: false, recovery: "retry" }
    )
  }
  if (error.code === "NATIVE_CONTROLLER_FAILED") {
    return new NativeBridgeRequestError(
      error.code,
      "BROWSER_CRASHED",
      "The native WebView2 controller is unavailable",
      { retryable: true, recovery: "recover_runtime" }
    )
  }
  if (
    error.code === "NATIVE_BRIDGE_FAILED" ||
    error.code === "NATIVE_RUNTIME_NOT_INITIALIZED"
  ) {
    return new NativeBridgeRequestError(
      error.code,
      "RUNTIME_NOT_READY",
      "The native Browser bridge is unavailable",
      { retryable: true, recovery: "recover_runtime" }
    )
  }
  if (error.code === "NAVIGATION_BLOCKED") {
    return new NativeBridgeRequestError(
      error.code,
      "URL_INVALID",
      "The native Browser blocked the navigation",
      { retryable: false, recovery: "retry" }
    )
  }
  return new NativeBridgeRequestError(
    error.code,
    "INTERNAL_ERROR",
    "The native Browser bridge rejected the operation",
    {
      retryable: error.retryable,
      recovery: error.retryable ? "recover_runtime" : "check_settings",
    }
  )
}

function readBridgeFailure(value: unknown): NativeBridgeFailure {
  const error = asRecord(asRecord(value).error)
  if (
    typeof error.code !== "string" ||
    typeof error.message !== "string" ||
    typeof error.retryable !== "boolean"
  ) {
    throw invalidBridgeResponse()
  }
  return {
    code: error.code,
    message: error.message,
    retryable: error.retryable,
  }
}

function readSnapshotValue(value: unknown): NativeBrowserSnapshot {
  return readSnapshot(asRecord(value).snapshot)
}

function readSnapshot(value: unknown): NativeBrowserSnapshot {
  const record = asRecord(value)
  if (
    typeof record.connectionId !== "string" ||
    !Array.isArray(record.tabs) ||
    (record.activeTabId !== null && typeof record.activeTabId !== "string")
  ) {
    throw invalidBridgeResponse()
  }
  const tabs = record.tabs.map(readTab)
  if (new Set(tabs.map((tab) => tab.id)).size !== tabs.length) {
    throw invalidBridgeResponse()
  }
  if (
    record.activeTabId !== null &&
    !tabs.some((tab) => tab.id === record.activeTabId)
  ) {
    throw invalidBridgeResponse()
  }
  return {
    connectionId: record.connectionId,
    tabs,
    activeTabId: record.activeTabId,
  }
}

function readTab(value: unknown): NativeBrowserTab {
  const record = asRecord(value)
  if (
    typeof record.id !== "string" ||
    !Number.isSafeInteger(record.generation) ||
    (record.generation as number) < 1 ||
    typeof record.url !== "string" ||
    typeof record.title !== "string" ||
    typeof record.loading !== "boolean" ||
    typeof record.canGoBack !== "boolean" ||
    typeof record.canGoForward !== "boolean" ||
    (record.state !== "ready" &&
      record.state !== "loading" &&
      record.state !== "error") ||
    (record.errorCode !== null &&
      record.errorCode !== undefined &&
      typeof record.errorCode !== "string")
  ) {
    throw invalidBridgeResponse()
  }
  return {
    id: record.id,
    generation: record.generation as number,
    url: record.url,
    title: record.title,
    loading: record.loading,
    canGoBack: record.canGoBack,
    canGoForward: record.canGoForward,
    state: record.state,
    errorCode: typeof record.errorCode === "string" ? record.errorCode : null,
  }
}

function readDownload(value: unknown): BrowserDownload {
  const record = asRecord(value)
  if (
    typeof record.guid !== "string" ||
    (record.state !== "inProgress" &&
      record.state !== "completed" &&
      record.state !== "canceled") ||
    typeof record.filename !== "string" ||
    typeof record.receivedBytes !== "number" ||
    typeof record.totalBytes !== "number" ||
    (record.path !== undefined &&
      record.path !== null &&
      typeof record.path !== "string") ||
    (record.errorCode !== undefined &&
      record.errorCode !== null &&
      typeof record.errorCode !== "string")
  ) {
    throw invalidBridgeResponse()
  }
  return {
    guid: record.guid,
    state: record.state,
    filename: record.filename,
    receivedBytes: record.receivedBytes,
    totalBytes: record.totalBytes,
    ...(typeof record.path === "string" ? { path: record.path } : {}),
    ...(typeof record.errorCode === "string"
      ? { errorCode: record.errorCode }
      : {}),
  }
}

function publicTab(tab: NativeBrowserTab): BrowserTab {
  return { id: tab.id, url: tab.url, title: tab.title }
}

function validateBridgeEndpoint(value: string): string {
  let endpoint: URL
  try {
    endpoint = new URL(value)
  } catch {
    throw new BrowserError(
      "RUNTIME_START_FAILED",
      "The native Browser bridge endpoint is invalid",
      { recovery: "check_settings" }
    )
  }
  if (
    endpoint.protocol !== "http:" ||
    endpoint.hostname !== "127.0.0.1" ||
    !endpoint.port ||
    endpoint.username ||
    endpoint.password ||
    (endpoint.pathname !== "/" && endpoint.pathname !== "") ||
    endpoint.search ||
    endpoint.hash
  ) {
    throw new BrowserError(
      "RUNTIME_START_FAILED",
      "The native Browser bridge endpoint must be an IPv4 loopback origin",
      { recovery: "check_settings" }
    )
  }
  return endpoint.origin
}

function asRecord(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw invalidBridgeResponse()
  }
  return value as Record<string, unknown>
}

function pageKey(sessionId: string, tabId: string, generation: number): string {
  return `${sessionId}\0${tabId}\0${generation}`
}

function tabNotFound(): BrowserError {
  return new BrowserError(
    "TAB_NOT_FOUND",
    "The native Browser tab is not owned by this Agent session",
    { recovery: "retry" }
  )
}

function staleGeneration(): BrowserError {
  return new BrowserError(
    "SESSION_STALE",
    "The native Browser tab generation changed; refresh the tab snapshot and retry",
    { retryable: true, recovery: "retry" }
  )
}

function crashedTab(errorCode: string | null): BrowserError {
  return new BrowserError(
    "BROWSER_CRASHED",
    errorCode
      ? `The native WebView2 controller failed (${errorCode})`
      : "The native WebView2 controller failed",
    { retryable: true, recovery: "recover_runtime" }
  )
}

function invalidBridgeResponse(): BrowserError {
  return new BrowserError(
    "RUNTIME_NOT_READY",
    "The native Browser bridge returned an invalid response",
    { retryable: true, recovery: "recover_runtime" }
  )
}

function runtimeNotReady(): BrowserError {
  return new BrowserError("RUNTIME_NOT_READY", "Browser runtime is not ready", {
    retryable: true,
    recovery: "recover_runtime",
  })
}

function nativeSurfaceOwnedByHost(): BrowserError {
  return new BrowserError(
    "RUNTIME_NOT_READY",
    "The native Browser surface is controlled directly by the desktop host",
    { retryable: false, recovery: "check_settings" }
  )
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}
