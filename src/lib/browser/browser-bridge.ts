// Client of the web-mode port bridge: when the workbench runs in a browser,
// a dev server on the codeg host (`http://localhost:3000` as an agent printed
// it) is shown through a bridge listener the server binds next to its own
// port. The workbench asks the API for a grant, then loads the listener's
// entry URL in an iframe; the entry sets the listener's cookie and redirects
// to the page. No React, no DOM beyond `fetch` for the reachability probe.

import { getTransport, isDesktop } from "@/lib/transport"

import { isLoopbackHost } from "./browser-url"

export interface BridgeStatus {
  enabled: boolean
  /** Ports a listener may take (`0` = any free port). */
  ports: number[]
  /** Hostname to use for the bridge instead of the page's own. */
  publicHost: string | null
}

export interface BridgeGrant {
  targetPort: number
  bridgePort: number
  /** Path on the bridge origin that sets the cookie and redirects. */
  entryPath: string
  publicHost: string | null
  /** Path and query of the requested address, for the entry redirect. */
  path: string
}

const OFF: BridgeStatus = { enabled: false, ports: [], publicHost: null }

let statusPromise: Promise<BridgeStatus> | null = null
let resolvedStatus: BridgeStatus | null = null

/**
 * Whether this server bridges dev-server ports. Cached for the session; the
 * desktop never needs it (its browser tabs reach the host directly).
 */
export function bridgeStatus(): Promise<BridgeStatus> {
  if (isDesktop()) {
    resolvedStatus = OFF
    return Promise.resolve(OFF)
  }
  if (!statusPromise) {
    statusPromise = getTransport()
      .call<BridgeStatus>("browser_bridge_status", {})
      .then((status) => {
        resolvedStatus = status
        return status
      })
      .catch(() => {
        statusPromise = null
        return OFF
      })
  }
  return statusPromise
}

/** Synchronous view for decisions inside a click; null until asked once. */
export function bridgeStatusSnapshot(): BridgeStatus | null {
  return resolvedStatus
}

/** Tests only. */
export function resetBridgeStatusForTests(): void {
  statusPromise = null
  resolvedStatus = null
}

export function bridgeOpen(url: string, tabId: string): Promise<BridgeGrant> {
  return getTransport().call<BridgeGrant>("browser_bridge_open", { url, tabId })
}

export function bridgeClose(tabId: string): Promise<void> {
  return getTransport()
    .call<unknown>("browser_bridge_close", { tabId })
    .then(() => undefined)
}

/**
 * An address the bridge can carry: plain http to the host's own loopback.
 * A private-network or public host is not the server's to forward, and the
 * bridge speaks to its target without TLS.
 */
export function isBridgeableUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    return parsed.protocol === "http:" && isLoopbackHost(parsed.hostname)
  } catch {
    return false
  }
}

export interface PageLocation {
  /** `https:` / `http:` — the bridge follows the page's scheme. */
  protocol: string
  hostname: string
}

/**
 * Origin of the grant's listener as this browser should reach it: the
 * server's public host when it named one, else the host the workbench was
 * loaded from — the same host, a different port, which keeps the bridge's
 * cookie same-site with the workbench.
 */
export function bridgeOrigin(grant: BridgeGrant, page: PageLocation): string {
  const host = grant.publicHost ?? page.hostname
  const authority =
    host.includes(":") && !host.startsWith("[") ? `[${host}]` : host
  return `${page.protocol}//${authority}:${grant.bridgePort}`
}

/** The URL the frame loads first: the entry that sets the cookie. */
export function bridgeEntryUrl(grant: BridgeGrant, page: PageLocation): string {
  const to = grant.path.startsWith("/") ? grant.path : `/${grant.path}`
  return `${bridgeOrigin(grant, page)}${grant.entryPath}?to=${encodeURIComponent(to)}`
}

/**
 * Whether the listener answers from where this browser sits. Unmapped ports
 * (Docker without the range published, a firewall) are the likely failure,
 * and a blank frame would say nothing about it.
 */
export async function probeBridge(
  origin: string,
  timeoutMs = 4000
): Promise<boolean> {
  const controller = new AbortController()
  const timer = window.setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(`${origin}/__codeg_bridge/ping`, {
      mode: "cors",
      credentials: "omit",
      cache: "no-store",
      signal: controller.signal,
    })
    return response.ok
  } catch {
    return false
  } finally {
    window.clearTimeout(timer)
  }
}
