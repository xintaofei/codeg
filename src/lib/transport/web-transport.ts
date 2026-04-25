import type { Transport, UnsubscribeFn } from "./types"

interface WebEvent {
  channel: string
  payload: unknown
}

function createClientId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID()
  }
  return `web-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

function getToken(): string {
  return localStorage.getItem("codeg_token") ?? ""
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
    const res = await fetch(`${this.baseUrl}/api/${command}`, {
      method: "POST",
      headers,
      body: JSON.stringify(args ?? {}),
      keepalive,
    })
    if (res.status === 401) {
      WebTransport.redirectToLogin()
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
      this.wsFailCount++
      if (this.wsFailCount >= 3) {
        WebTransport.redirectToLogin()
        return
      }
      this.reconnectTimer = setTimeout(() => this.connectWs(), 3000)
    }

    this.ws.onerror = () => {
      this.ws?.close()
    }
  }

  destroy() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
    }
    this.ws?.close()
    this.ws = null
    this.handlers.clear()
  }
}
