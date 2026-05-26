import { WS_READY_CHANNEL } from "./constants"
import type { AttachTransportHost } from "./web-event-stream"
import { WebEventStream } from "./web-event-stream"
import type {
  CallOptions,
  EventStream,
  Transport,
  UnsubscribeFn,
} from "./types"
import { buildCodegWebSocketProtocols } from "./ws-auth"
import { getCodegToken, redirectToCodegLogin } from "./web-auth"

// 60s covers the worst-case ACP probe path: some agents (Gemini in
// particular) burn 8–10s on the Initialize handshake before session/new
// can even start. The backend probe timeout in `ConnectionManager::
// probe_agent_options` is 60s, so the transport-level cap must be at
// least that high — otherwise the UI aborts with "Request timed out"
// while the backend is still holding a live probe process.
const WEB_CALL_TIMEOUT_MS = 60_000
// Upper bound on how long `subscribe()` will wait for the server `__ready__`
// frame. Generous enough to cover slow local servers and remote round-trips
// (typical: <100ms local, <1s WAN), but bounded so an older server (no
// `__ready__` support), a hung backend task, or a buffering proxy can't
// permanently lock the UI. On timeout we proceed without confirmation — the
// pre-fix race window reopens, but the UI stays responsive.
const READY_TIMEOUT_MS = 5_000
// Number of consecutive WS reconnect failures before we give up and surface
// the session-expired state. Matches `WS_RECONNECT_FAIL_THRESHOLD` in
// `src-tauri/src/commands/remote_proxy.rs` so behaviour is consistent across
// transports.
const WS_RECONNECT_FAIL_THRESHOLD = 3
// Exponential backoff bounds, in milliseconds: 1s → 2s → 4s → … capped at
// 32s. Cap matches the Rust-side WS_BACKOFF_MAX_SECS.
const WS_BACKOFF_INITIAL_MS = 1_000
const WS_BACKOFF_MAX_MS = 32_000

interface WebEvent {
  channel: string
  payload: unknown
}

const getToken = getCodegToken

export class WebTransport implements Transport {
  private ws: WebSocket | null = null
  private handlers = new Map<string, Set<(payload: unknown) => void>>()
  private baseUrl: string
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private wsFailCount = 0
  private readyPromise!: Promise<void>
  private readyResolve!: () => void
  // Tracks whether `__ready__` has ever arrived on this transport instance.
  // The first arrival is the initial connect; subsequent arrivals are
  // reconnects (after `onclose` reset the promise). Reconnect callbacks
  // run only on subsequent arrivals so consumers can refresh state that
  // may have desynced during the disconnect window.
  private hasReadiedOnce = false
  private reconnectCallbacks = new Set<() => void>()
  // Latched in `destroy()`. The async `onclose` fired by `ws.close()` inside
  // `destroy()` would otherwise increment `wsFailCount` and schedule a new
  // reconnect — and, worse, can trip `wsFailCount >= 3` after repeated
  // teardowns and call `redirectToLogin()` from a transport the caller
  // already let go of. Guard `onclose` to short-circuit when destroyed.
  private destroyed = false
  // Attach-protocol plumbing. The EventStream is created lazily on first
  // call to `eventStream()`; it lives for the entire transport lifetime and
  // re-attaches its subscriptions on every WS-ready transition.
  private wsOpen = false
  private wsReadyCallbacks = new Set<() => void>()
  private eventStreamInstance: WebEventStream | null = null

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl
    this.resetReady()
  }

  private resetReady() {
    this.readyPromise = new Promise<void>((resolve) => {
      this.readyResolve = resolve
    })
  }

  // Bounded wait on `readyPromise`. If `__ready__` does not arrive within
  // `READY_TIMEOUT_MS`, log a warning and fall through — degrades to the
  // pre-handshake behavior instead of hanging the UI forever.
  //
  // Public so callers (e.g. `acp_connect` in the ACP context) can gate
  // HTTP commands on WS readiness, not just the initial `subscribe()`.
  // The `subscribe()`-only gate covers the initial-connect race but
  // leaves a window where `acp_connect` fired during a mid-session
  // reconnect would still race the broadcaster's `receiver_count == 0`
  // guard. Awaiting this directly before such HTTP calls closes that gap.
  async waitForReady(): Promise<void> {
    let timeoutId: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<"timeout">((resolve) => {
      timeoutId = setTimeout(() => resolve("timeout"), READY_TIMEOUT_MS)
    })
    const result = await Promise.race([
      this.readyPromise.then(() => "ready" as const),
      timeoutPromise,
    ])
    if (timeoutId !== undefined) clearTimeout(timeoutId)
    if (result === "timeout") {
      console.warn(
        `[WebTransport] WS __ready__ frame did not arrive within ${READY_TIMEOUT_MS}ms; ` +
          "proceeding without server-side subscribe confirmation (initial-connect race may reopen)."
      )
    }
  }

  async call<T>(
    command: string,
    args?: Record<string, unknown>,
    options?: CallOptions
  ): Promise<T> {
    const token = getToken()
    const controller = new AbortController()
    // Per-call override beats the transport-wide default. Used for
    // commands whose backend handler has its own long deadline that
    // would otherwise race with `WEB_CALL_TIMEOUT_MS` and surface
    // "Request timed out" before the backend can return a structured
    // error. See `describeAgentOptions` in `lib/api.ts`.
    const effectiveTimeoutMs = options?.timeoutMs ?? WEB_CALL_TIMEOUT_MS
    const timeout = window.setTimeout(
      () => controller.abort(),
      effectiveTimeoutMs
    )
    let res: Response
    try {
      res = await fetch(`${this.baseUrl}/api/${command}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(args ?? {}),
        signal: controller.signal,
      })
    } catch (err) {
      if (err instanceof DOMException && err.name === "AbortError") {
        throw new Error("Request timed out")
      }
      throw err
    } finally {
      window.clearTimeout(timeout)
    }
    if (res.status === 401) {
      redirectToCodegLogin()
      throw new Error("Unauthorized")
    }
    if (!res.ok) {
      const error = await res.json().catch(() => ({
        code: "network_error",
        message: `HTTP ${res.status}`,
      }))
      throw error
    }
    return res.json()
  }

  async subscribe<T>(
    event: string,
    handler: (payload: T) => void
  ): Promise<UnsubscribeFn> {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, new Set())
    }
    const wrappedHandler = handler as (payload: unknown) => void
    this.handlers.get(event)!.add(wrappedHandler)

    // If WS is not connected but we now have a token, connect
    if (!this.ws && getToken()) {
      this.connectWs()
    }

    // Gate on the server-side broadcaster receiver actually being subscribed.
    // The backend WS handler sends a `__ready__` frame after subscribing, so
    // any event emitted past this await is guaranteed to reach a receiver.
    // Without this, events fired before the server-side subscribe (e.g. the
    // ACP `Connected` event after a fast Initialize) are silently dropped
    // because the broadcaster skips `send` when receiver_count == 0, leaving
    // the UI permanently stuck on "正在连接".
    if (getToken()) {
      await this.waitForReady()
    }

    return () => {
      this.handlers.get(event)?.delete(wrappedHandler)
    }
  }

  isDesktop(): boolean {
    return false
  }

  onReconnect(callback: () => void): UnsubscribeFn {
    this.reconnectCallbacks.add(callback)
    return () => {
      this.reconnectCallbacks.delete(callback)
    }
  }

  eventStream(): EventStream {
    if (!this.eventStreamInstance) {
      const host: AttachTransportHost = {
        isWsOpen: () => this.wsOpen,
        sendFrame: (frame) => this.sendWsFrame(frame),
        onWsReady: (callback) => {
          this.wsReadyCallbacks.add(callback)
          return () => {
            this.wsReadyCallbacks.delete(callback)
          }
        },
      }
      this.eventStreamInstance = new WebEventStream(host)
      // If a token is present but the WS hasn't been opened yet (no legacy
      // subscribe has run), trigger the connect now so attach frames sent
      // by the consumer have a place to land.
      if (!this.ws && getToken()) {
        this.connectWs()
      }
    }
    return this.eventStreamInstance
  }

  private sendWsFrame(frame: object): boolean {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return false
    try {
      this.ws.send(JSON.stringify(frame))
      return true
    } catch (err) {
      console.warn("[WebTransport] sendWsFrame failed:", err)
      return false
    }
  }

  private connectWs() {
    const token = getToken()
    if (!token) return

    const wsUrl = this.baseUrl.replace(/^http/, "ws") + "/ws/events"
    this.ws = new WebSocket(wsUrl, buildCodegWebSocketProtocols(token))

    this.ws.onopen = () => {
      this.wsFailCount = 0
      this.wsOpen = true
      // Notify the EventStream so it can re-issue attach frames for any
      // active subscriptions. Fires on initial connect AND every reconnect;
      // the EventStream uses each sub's running `lastAppliedSeq` so the
      // server can pick replay vs. snapshot. Errors in callbacks must not
      // break sibling callbacks.
      for (const cb of this.wsReadyCallbacks) {
        try {
          cb()
        } catch (err) {
          console.error("[WebTransport] wsReady callback threw:", err)
        }
      }
    }

    this.ws.onmessage = (msg) => {
      try {
        const parsed = JSON.parse(msg.data) as unknown
        // Attach-protocol frames carry a `type` discriminator; legacy
        // global-broadcast frames carry a `channel` discriminator. Routing
        // by which field is present lets the two coexist on the same WS.
        if (
          parsed &&
          typeof parsed === "object" &&
          "type" in (parsed as object)
        ) {
          this.eventStreamInstance?.handleServerFrame(parsed)
          return
        }
        const event = parsed as WebEvent
        if (event.channel === WS_READY_CHANNEL) {
          this.readyResolve()
          if (this.hasReadiedOnce) {
            // Reconnect path: server-side receiver_count was 0 during the
            // disconnect window, so any event fired in that gap was dropped.
            // Notify consumers to recover state (e.g. refetch snapshots).
            // Errors in user callbacks must not break sibling callbacks.
            for (const cb of this.reconnectCallbacks) {
              try {
                cb()
              } catch (err) {
                console.error("[WebTransport] reconnect callback threw:", err)
              }
            }
          } else {
            this.hasReadiedOnce = true
          }
          return
        }
        const handlers = this.handlers.get(event.channel)
        if (handlers) {
          for (const h of handlers) {
            h(event.payload)
          }
        }
      } catch {
        // ignore malformed messages
      }
    }

    this.ws.onclose = () => {
      this.ws = null
      this.wsOpen = false
      // New subscribers (and any concurrent subscribe() calls in flight)
      // must wait for the next connection's `__ready__` before resolving.
      this.resetReady()
      if (this.destroyed) return
      this.wsFailCount++
      if (this.wsFailCount >= WS_RECONNECT_FAIL_THRESHOLD) {
        redirectToCodegLogin()
        return
      }
      // Exponential backoff: 1s, 2s, 4s, … capped at WS_BACKOFF_MAX_MS.
      // Kept in lockstep with the Rust-side WS task in
      // `src-tauri/src/commands/remote_proxy.rs` so the user experiences
      // the same reconnect cadence whether they're on the web client or
      // a desktop remote workspace.
      const shift = Math.min(this.wsFailCount - 1, 8)
      const delay = Math.min(WS_BACKOFF_INITIAL_MS << shift, WS_BACKOFF_MAX_MS)
      this.reconnectTimer = setTimeout(() => this.connectWs(), delay)
    }

    this.ws.onerror = () => {
      this.ws?.close()
    }
  }

  destroy() {
    this.destroyed = true
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    if (this.ws) {
      // Detach handlers BEFORE close() so the async `onclose` fired by the
      // browser doesn't see this instance at all and can't trip the
      // wsFailCount-based redirectToLogin from a transport the caller
      // already discarded. The `destroyed` guard above covers any handlers
      // that may have already been dispatched before this detach.
      this.ws.onopen = null
      this.ws.onmessage = null
      this.ws.onclose = null
      this.ws.onerror = null
      this.ws.close()
      this.ws = null
    }
    this.wsOpen = false
    this.handlers.clear()
    this.reconnectCallbacks.clear()
    this.wsReadyCallbacks.clear()
    this.eventStreamInstance?.destroy()
    this.eventStreamInstance = null
    // Settle any in-flight `subscribe()` awaiters so their promises don't
    // leak alongside the destroyed transport. Safe to call multiple times —
    // resolving an already-settled Promise is a no-op.
    this.readyResolve?.()
  }
}
