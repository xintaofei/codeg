import type { Transport, UnsubscribeFn } from "./types"

interface WebEvent {
  channel: string
  payload: unknown
}

function createClientId(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID()
  }
  return `web-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

function getToken(): string {
  return localStorage.getItem("codeg_token") ?? ""
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export class WebTransport implements Transport {
  private ws: WebSocket | null = null
  private handlers = new Map<string, Set<(payload: unknown) => void>>()
  private baseUrl: string
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private wsFailCount = 0
  private clientId = createClientId()

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl
  }

  async call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const token = getToken()
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      "X-Codeg-Client-Id": this.clientId,
    }
    if (token) {
      headers.Authorization = `Bearer ${token}`
    }
    const keepalive =
      command === "acp_disconnect" || command === "terminal_kill"

    // Idempotent read commands may be retried once on transient network
    // failure / 5xx. Non-idempotent commands (acp_prompt, terminal_create,
    // anything that mutates) MUST NOT be retried, because the server may
    // already have processed the first attempt.
    const isIdempotent =
      command.startsWith("list_") ||
      command.startsWith("get_") ||
      command.startsWith("acp_get_") ||
      command.startsWith("acp_list_")
    const maxAttempts = isIdempotent ? 2 : 1

    let lastError: unknown
    for (let attempt = 1; attempt <= maxAttempts; attempt++) {
      // 15s hard timeout per attempt — protects against hung connections.
      const ctrl = new AbortController()
      const timeoutId = setTimeout(() => ctrl.abort(), 15_000)
      try {
        const res = await fetch(`${this.baseUrl}/api/${command}`, {
          method: "POST",
          headers,
          body: JSON.stringify(args ?? {}),
          keepalive,
          signal: ctrl.signal,
        })
        clearTimeout(timeoutId)

        if (res.status === 401) {
          WebTransport.redirectToLogin()
          throw new Error("Unauthorized")
        }
        if (!res.ok) {
          // 5xx is retriable for idempotent calls; 4xx is a real error.
          const isTransient = res.status >= 500
          const error = await res.json().catch(() => ({
            code: "network_error",
            message: `HTTP ${res.status}`,
          }))
          if (isTransient && attempt < maxAttempts) {
            lastError = error
            await sleep(250 * attempt)
            continue
          }
          throw error
        }
        return res.json()
      } catch (err) {
        clearTimeout(timeoutId)
        // AbortError / network refused are retriable for idempotent reads.
        const isAbort = err instanceof DOMException && err.name === "AbortError"
        const isNetwork = err instanceof TypeError
        if ((isAbort || isNetwork) && attempt < maxAttempts) {
          lastError = err
          await sleep(250 * attempt)
          continue
        }
        throw err
      }
    }
    throw lastError ?? new Error("network failure")
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

    if (!this.ws) {
      this.connectWs()
    }

    return () => {
      this.handlers.get(event)?.delete(wrappedHandler)
    }
  }

  isDesktop(): boolean {
    return false
  }

  private static redirectToLogin() {
    if (window.location.pathname.startsWith("/login")) return
    localStorage.removeItem("codeg_token")
    window.location.href = "/login"
  }

  private connectWs() {
    const token = getToken()
    const wsUrl = new URL("/ws/events", this.baseUrl)
    if (token) {
      wsUrl.searchParams.set("token", token)
    }
    wsUrl.searchParams.set("clientId", this.clientId)
    this.ws = new WebSocket(wsUrl)

    this.ws.onopen = () => {
      this.wsFailCount = 0
    }

    this.ws.onmessage = (msg) => {
      try {
        const event = JSON.parse(msg.data) as WebEvent
        // Server emits `__resync` when a slow socket overflows the
        // broadcast buffer and skipped events. If nothing is subscribed
        // we at least log it so devs see why the UI may be stale.
        if (event.channel === "__resync" && !this.handlers.has("__resync")) {
          console.warn("[ws] resync requested but no handler", event.payload)
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

    this.ws.onclose = (ev) => {
      this.ws = null
      this.wsFailCount++

      // Only force the user back to /login on explicit auth failures —
      // either an HTTP 401 surfaced as WS close code 4401, or any 4xx.
      // Transient network drops (sleep/wake, flaky wifi) used to log the
      // user out after 3 attempts, which was extremely user-hostile.
      const isAuthFailure =
        ev.code === 4401 || (ev.code >= 4400 && ev.code < 4500)
      if (isAuthFailure) {
        WebTransport.redirectToLogin()
        return
      }

      // Exponential backoff with ±20% jitter, capped at 30s.
      // 1s → 2s → 4s → 8s → 16s → 30s → 30s …
      const baseMs = Math.min(
        30_000,
        1000 * 2 ** Math.min(this.wsFailCount - 1, 5)
      )
      const jitter = baseMs * (Math.random() * 0.4 - 0.2)
      const delay = Math.max(500, Math.round(baseMs + jitter))
      this.reconnectTimer = setTimeout(() => this.connectWs(), delay)
    }

    this.ws.onerror = () => {
      this.ws?.close()
    }
  }

  destroy() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
    this.ws?.close()
    this.ws = null
    this.wsFailCount = 0
    this.handlers.clear()
  }
}
