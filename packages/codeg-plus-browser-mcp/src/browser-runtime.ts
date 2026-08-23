import {
  DEFAULT_TOOL_TIMEOUT_MS,
  MAX_TEXT_RESULT_BYTES,
  parseToolArguments,
} from "./contracts.js"
import { BrowserError, asBrowserError } from "./errors.js"
import type {
  BrowserBackend,
  BrowserBackendSessionSnapshot,
  BrowserRuntimeStatus,
  BrowserSurfaceAction,
  BrowserSurfaceEvent,
  BrowserSurfaceSnapshot,
  BrowserTab,
  BrowserToolOutput,
  PageController,
  ToolResultEnvelope,
} from "./runtime-types.js"

interface BrowserSession {
  readonly targets: Set<string>
  activeTargetId: string | null
  surfaceViewport: { width: number; height: number } | null
  lastSeenAt: number
}

interface SurfaceSubscription {
  emit: (event: BrowserSurfaceEvent) => void
  targetId: string | null
  stopScreencast: (() => Promise<void>) | null
}

export interface BrowserRuntimeOptions {
  sessionTtlMs?: number
  now?: () => number
}

const DEFAULT_SESSION_TTL_MS = 30 * 60 * 1_000

export class BrowserRuntime {
  private readonly sessions = new Map<string, BrowserSession>()
  private readonly surfaceSubscriptions = new Map<string, SurfaceSubscription>()
  private readonly sessionTtlMs: number
  private readonly now: () => number

  constructor(
    private readonly backend: BrowserBackend,
    options: BrowserRuntimeOptions = {}
  ) {
    this.sessionTtlMs = options.sessionTtlMs ?? DEFAULT_SESSION_TTL_MS
    this.now = options.now ?? Date.now
  }

  status(): BrowserRuntimeStatus {
    return {
      ...this.backend.status(),
      sessionCount: this.sessions.size,
    }
  }

  async callTool(
    sessionId: string,
    name: string,
    rawArguments: unknown
  ): Promise<BrowserToolOutput> {
    const parsed = parseToolArguments(name, rawArguments)
    if (!parsed.ok) {
      const message =
        parsed.errorCode === "UNKNOWN_TOOL"
          ? "The requested browser tool is not available"
          : "Browser tool arguments are invalid"
      return this.failure(
        sessionId,
        name,
        new BrowserError(parsed.errorCode, message)
      )
    }

    try {
      await this.cleanupStaleSessions()
      const output = await this.dispatch(sessionId, name, parsed.value)
      if (output.envelope.ok && this.surfaceSubscriptions.has(sessionId)) {
        try {
          await this.refreshSurface(sessionId)
        } catch (error) {
          await this.releaseSession(sessionId)
          throw error
        }
      }
      ensureBoundedEnvelope(output.envelope)
      return output
    } catch (error) {
      return this.failure(sessionId, name, asBrowserError(error))
    }
  }

  async releaseSession(sessionId: string): Promise<void> {
    await this.detachSurface(sessionId)
    this.sessions.delete(sessionId)
    await this.backend.releaseSession(sessionId)
  }

  async shutdown(): Promise<void> {
    for (const sessionId of [...this.surfaceSubscriptions.keys()]) {
      await this.detachSurface(sessionId)
    }
    this.sessions.clear()
    await this.backend.shutdown()
  }

  async ensureSurface(sessionId: string): Promise<BrowserSurfaceSnapshot> {
    await this.cleanupStaleSessions()
    this.requireReady()
    const session = this.session(sessionId)
    if (!session.activeTargetId) await this.activePage(sessionId, session)
    return await this.surfaceSnapshot(sessionId, session)
  }

  async surfaceAction(
    sessionId: string,
    command: BrowserSurfaceAction
  ): Promise<BrowserSurfaceSnapshot> {
    await this.cleanupStaleSessions()
    this.requireReady()
    const session = this.session(sessionId)
    await this.reconcileBackendSession(sessionId, session, false)

    if (command.action === "open") {
      const page = await this.backend.openTarget(sessionId)
      let tab = await page.info()
      this.ownTarget(sessionId, session, tab.id)
      if (command.url) {
        tab = await page.navigate(command.url, DEFAULT_TOOL_TIMEOUT_MS)
      }
    } else if (command.action === "focus") {
      this.assertOwned(session, command.targetId)
      await this.backend.focusTarget(sessionId, command.targetId)
      session.activeTargetId = command.targetId
    } else if (command.action === "close") {
      this.assertOwned(session, command.targetId)
      await this.backend.closeTarget(sessionId, command.targetId)
      session.targets.delete(command.targetId)
      if (session.activeTargetId === command.targetId) {
        session.activeTargetId = session.targets.values().next().value ?? null
      }
    } else {
      const page = await this.activePage(sessionId, session)
      if (command.action === "navigate") {
        await page.navigate(command.url, DEFAULT_TOOL_TIMEOUT_MS)
      } else if (command.action === "back") {
        await page.goHistory(-1, DEFAULT_TOOL_TIMEOUT_MS)
      } else if (command.action === "forward") {
        await page.goHistory(1, DEFAULT_TOOL_TIMEOUT_MS)
      } else if (command.action === "reload") {
        await page.reload(DEFAULT_TOOL_TIMEOUT_MS)
      } else if (command.action === "stop") {
        await page.stopLoading(DEFAULT_TOOL_TIMEOUT_MS)
      } else if (command.action === "resize") {
        session.surfaceViewport = {
          width: command.width,
          height: command.height,
        }
        await this.applySurfaceViewport(session, page)
      } else if (command.action === "input") {
        await page.dispatchSurfaceInput(command.input, DEFAULT_TOOL_TIMEOUT_MS)
      }
    }

    await this.refreshSurface(sessionId)
    return await this.surfaceSnapshot(sessionId, session)
  }

  async subscribeSurface(
    sessionId: string,
    emit: (event: BrowserSurfaceEvent) => void
  ): Promise<() => Promise<void>> {
    if (this.surfaceSubscriptions.has(sessionId)) {
      throw new BrowserError(
        "SURFACE_ALREADY_ATTACHED",
        "The Browser surface is already attached for this Agent session"
      )
    }
    const subscription: SurfaceSubscription = {
      emit,
      targetId: null,
      stopScreencast: null,
    }
    this.surfaceSubscriptions.set(sessionId, subscription)
    try {
      await this.ensureSurface(sessionId)
      await this.refreshSurface(sessionId)
    } catch (error) {
      this.surfaceSubscriptions.delete(sessionId)
      await subscription.stopScreencast?.().catch(() => undefined)
      throw error
    }
    let detached = false
    return async () => {
      if (detached) return
      detached = true
      if (this.surfaceSubscriptions.get(sessionId) === subscription) {
        await this.detachSurface(sessionId)
      }
    }
  }

  private async dispatch(
    sessionId: string,
    name: string,
    args: Record<string, unknown>
  ): Promise<BrowserToolOutput> {
    if (name === "runtime.status") {
      return this.success(sessionId, name, {
        ...this.status(),
      })
    }
    if (name === "runtime.doctor") {
      return this.success(sessionId, name, await this.backend.doctor())
    }
    if (name === "runtime.start") {
      await this.backend.start()
      return this.success(sessionId, name, this.backend.status())
    }
    if (name === "runtime.stop") {
      for (const sessionId of [...this.surfaceSubscriptions.keys()]) {
        await this.detachSurface(sessionId)
      }
      this.sessions.clear()
      await this.backend.stop()
      return this.success(sessionId, name, this.backend.status())
    }
    if (name === "runtime.recover") {
      for (const sessionId of [...this.surfaceSubscriptions.keys()]) {
        await this.detachSurface(sessionId)
      }
      this.sessions.clear()
      await this.backend.recover()
      return this.success(sessionId, name, this.backend.status())
    }

    this.requireReady()
    const session = this.session(sessionId)
    await this.reconcileBackendSession(sessionId, session, false)
    if (name === "tabs.list") {
      const tabs = await this.syncSessionTargets(sessionId, session)
      return this.success(sessionId, name, {
        tabs,
        activeTabId: session.activeTargetId,
      })
    }
    if (name === "tabs.open") {
      // Always create about:blank first, attach the page controller (which
      // enables request interception), and establish session ownership before
      // navigating. Creating the target directly at an external URL would let
      // its first wave of subresource requests race the SSRF interceptor.
      const page = await this.backend.openTarget(sessionId)
      let tab = await page.info()
      this.ownTarget(sessionId, session, tab.id)
      const requestedUrl = optionalString(args.url)
      if (requestedUrl) {
        tab = await page.navigate(requestedUrl, DEFAULT_TOOL_TIMEOUT_MS)
      }
      return this.success(sessionId, name, { opened: true }, tab)
    }
    if (name === "tabs.focus") {
      const targetId = requiredString(args.tabId)
      this.assertOwned(session, targetId)
      await this.backend.focusTarget(sessionId, targetId)
      session.activeTargetId = targetId
      const tab = await (await this.backend.page(sessionId, targetId)).info()
      return this.success(sessionId, name, { focused: true }, tab)
    }
    if (name === "tabs.close") {
      const targetId = requiredString(args.tabId)
      this.assertOwned(session, targetId)
      await this.backend.closeTarget(sessionId, targetId)
      session.targets.delete(targetId)
      if (session.activeTargetId === targetId) {
        session.activeTargetId = session.targets.values().next().value ?? null
      }
      return this.success(sessionId, name, {
        closedTabId: targetId,
        activeTabId: session.activeTargetId,
      })
    }

    if (name === "download.list") {
      return this.success(sessionId, name, {
        downloads: await this.backend.downloads(sessionId),
      })
    }
    if (name === "download.wait") {
      const download = await this.backend.waitForDownload(
        sessionId,
        optionalString(args.guid),
        optionalNumber(args.timeoutMs) ?? DEFAULT_TOOL_TIMEOUT_MS
      )
      return this.success(sessionId, name, { download })
    }

    const page = await this.activePage(sessionId, session)
    const timeoutMs = optionalNumber(args.timeoutMs) ?? DEFAULT_TOOL_TIMEOUT_MS
    if (name === "page.navigate") {
      const tab = await page.navigate(requiredString(args.url), timeoutMs)
      return this.success(sessionId, name, { navigated: true }, tab)
    }
    if (name === "page.snapshot") {
      const snapshot = await page.snapshot({
        interactive: args.interactive === true,
        limit: optionalNumber(args.limit) ?? 500,
        timeoutMs,
      })
      const tab = await page.info()
      return this.success(sessionId, name, snapshot, tab)
    }
    if (name === "page.screenshot") {
      const format = args.format === "jpeg" ? "jpeg" : "png"
      const image = await page.screenshot({
        format,
        fullPage: args.fullPage === true,
        quality: optionalNumber(args.quality),
        timeoutMs,
      })
      const tab = await page.info()
      return {
        ...this.success(sessionId, name, { captured: true }, tab),
        image,
      }
    }
    if (name === "action.click") {
      await page.click(requiredString(args.ref), timeoutMs)
      return this.actionSuccess(sessionId, name, page)
    }
    if (name === "action.type" || name === "action.fill") {
      await page.type(
        requiredString(args.ref),
        requiredString(args.text),
        name === "action.fill",
        timeoutMs
      )
      return this.actionSuccess(sessionId, name, page)
    }
    if (name === "action.press") {
      await page.press(requiredString(args.key), timeoutMs)
      return this.actionSuccess(sessionId, name, page)
    }
    if (name === "action.scroll") {
      await page.scroll(
        optionalString(args.ref),
        optionalNumber(args.deltaX) ?? 0,
        optionalNumber(args.deltaY) ?? 600,
        timeoutMs
      )
      return this.actionSuccess(sessionId, name, page)
    }
    if (name === "action.wait") {
      await page.wait({
        milliseconds: optionalNumber(args.milliseconds),
        text: optionalString(args.text),
        urlIncludes: optionalString(args.urlIncludes),
        timeoutMs,
      })
      return this.actionSuccess(sessionId, name, page)
    }
    throw new BrowserError(
      "UNKNOWN_TOOL",
      "The requested browser tool is not available"
    )
  }

  private async actionSuccess(
    sessionId: string,
    action: string,
    page: PageController
  ): Promise<BrowserToolOutput> {
    return this.success(
      sessionId,
      action,
      { completed: true },
      await page.info()
    )
  }

  private session(sessionId: string): BrowserSession {
    let session = this.sessions.get(sessionId)
    if (!session) {
      session = {
        targets: new Set(),
        activeTargetId: null,
        surfaceViewport: null,
        lastSeenAt: this.now(),
      }
      this.sessions.set(sessionId, session)
    }
    session.lastSeenAt = this.now()
    return session
  }

  private async activePage(
    sessionId: string,
    session: BrowserSession
  ): Promise<PageController> {
    await this.reconcileBackendSession(sessionId, session, true)
    if (!session.activeTargetId) {
      const page = await this.backend.openTarget(sessionId)
      const tab = await page.info()
      this.ownTarget(sessionId, session, tab.id)
      await this.applySurfaceViewport(session, page)
      return page
    }
    this.assertOwned(session, session.activeTargetId)
    return await this.backend.page(sessionId, session.activeTargetId)
  }

  private async surfaceSnapshot(
    sessionId: string,
    session: BrowserSession
  ): Promise<BrowserSurfaceSnapshot> {
    const tabs = await this.syncSessionTargets(sessionId, session)
    const activeTargetId = session.activeTargetId
    const active = activeTargetId
      ? await (
          await this.backend.page(sessionId, activeTargetId)
        ).surfaceState()
      : null
    return { sessionId, tabs, activeTargetId, active }
  }

  private async syncSessionTargets(
    sessionId: string,
    session: BrowserSession
  ): Promise<BrowserTab[]> {
    const authoritative = await this.reconcileBackendSession(
      sessionId,
      session,
      false
    )
    const all =
      authoritative?.tabs ?? (await this.backend.listTargets(sessionId))
    const tabs = all.filter((tab) => session.targets.has(tab.id))
    const live = new Set(tabs.map((tab) => tab.id))
    for (const targetId of [...session.targets]) {
      if (!live.has(targetId)) session.targets.delete(targetId)
    }
    if (
      session.activeTargetId !== null &&
      !session.targets.has(session.activeTargetId)
    ) {
      session.activeTargetId = tabs[tabs.length - 1]?.id ?? null
    }
    return tabs
  }

  private async refreshSurface(sessionId: string): Promise<void> {
    const subscription = this.surfaceSubscriptions.get(sessionId)
    const session = this.sessions.get(sessionId)
    if (!subscription || !session) return
    const snapshot = await this.surfaceSnapshot(sessionId, session)
    const targetId = snapshot.activeTargetId
    if (subscription.targetId !== targetId) {
      await subscription.stopScreencast?.().catch(() => undefined)
      subscription.stopScreencast = null
      subscription.targetId = targetId
      if (targetId) {
        const page = await this.backend.page(sessionId, targetId)
        await this.applySurfaceViewport(session, page)
        subscription.stopScreencast = await page.startScreencast((frame) => {
          if (this.surfaceSubscriptions.get(sessionId) === subscription) {
            subscription.emit({ type: "frame", frame })
          }
        })
      }
    }
    subscription.emit({ type: "snapshot", snapshot })
  }

  private async detachSurface(sessionId: string): Promise<void> {
    const subscription = this.surfaceSubscriptions.get(sessionId)
    if (!subscription) return
    this.surfaceSubscriptions.delete(sessionId)
    await subscription.stopScreencast?.().catch(() => undefined)
  }

  private ownTarget(
    sessionId: string,
    session: BrowserSession,
    targetId: string
  ): void {
    session.targets.add(targetId)
    session.activeTargetId = targetId
    this.backend.assignTarget(targetId, sessionId)
  }

  private async reconcileBackendSession(
    sessionId: string,
    session: BrowserSession,
    ensure: boolean
  ): Promise<BrowserBackendSessionSnapshot | null> {
    const snapshot = await this.backend.sessionSnapshot(sessionId, ensure)
    if (!snapshot) return null
    session.targets.clear()
    for (const tab of snapshot.tabs) session.targets.add(tab.id)
    session.activeTargetId =
      snapshot.activeTargetId && session.targets.has(snapshot.activeTargetId)
        ? snapshot.activeTargetId
        : (snapshot.tabs[snapshot.tabs.length - 1]?.id ?? null)
    return snapshot
  }

  private async applySurfaceViewport(
    session: BrowserSession,
    page: PageController
  ): Promise<void> {
    if (!session.surfaceViewport) return
    await page.setSurfaceViewport(
      session.surfaceViewport.width,
      session.surfaceViewport.height,
      DEFAULT_TOOL_TIMEOUT_MS
    )
  }

  private assertOwned(session: BrowserSession, targetId: string): void {
    if (!session.targets.has(targetId)) {
      throw new BrowserError(
        "TAB_NOT_FOUND",
        "The tab does not exist in the current Agent session"
      )
    }
  }

  private requireReady(): void {
    if (this.backend.state !== "ready") {
      throw new BrowserError(
        "RUNTIME_NOT_READY",
        "Browser runtime is not ready",
        {
          retryable: true,
          recovery: "recover_runtime",
        }
      )
    }
  }

  private async cleanupStaleSessions(): Promise<void> {
    const cutoff = this.now() - this.sessionTtlMs
    const stale = [...this.sessions.entries()]
      .filter(([, session]) => session.lastSeenAt < cutoff)
      .map(([sessionId]) => sessionId)
    for (const sessionId of stale) await this.releaseSession(sessionId)
  }

  private success(
    sessionId: string,
    action: string,
    data: unknown,
    tab?: BrowserTab
  ): BrowserToolOutput {
    const envelope: ToolResultEnvelope = {
      ok: true,
      action,
      sessionId,
      data,
    }
    if (tab) envelope.tab = tab
    return { envelope }
  }

  private failure(
    sessionId: string,
    action: string,
    error: BrowserError
  ): BrowserToolOutput {
    return {
      envelope: {
        ok: false,
        action,
        sessionId,
        error: {
          code: error.code,
          message: error.message,
          retryable: error.retryable,
          ...(error.recovery ? { recovery: error.recovery } : {}),
        },
      },
    }
  }
}

function requiredString(value: unknown): string {
  if (typeof value !== "string") {
    throw new BrowserError(
      "INVALID_ARGS",
      "A required string argument is missing"
    )
  }
  return value
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined
}

function ensureBoundedEnvelope(envelope: ToolResultEnvelope): void {
  if (
    Buffer.byteLength(JSON.stringify(envelope), "utf8") > MAX_TEXT_RESULT_BYTES
  ) {
    throw new BrowserError(
      "RESULT_TOO_LARGE",
      "Browser text result exceeds the 200 kilobyte limit"
    )
  }
}
