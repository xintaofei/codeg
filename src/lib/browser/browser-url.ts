// URL helpers for the built-in browser. Pure: no React, no transport, no DOM —
// every function takes a string and answers a question about it, so the link
// decision function and the Rust-side policy can be tested against the same
// tables.

const IPV4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/

function parseIpv4(host: string): [number, number, number, number] | null {
  const m = host.match(IPV4)
  if (!m) return null
  const parts = m.slice(1, 5).map(Number) as [number, number, number, number]
  return parts.every((n) => n >= 0 && n <= 255) ? parts : null
}

/** `new URL(...).hostname` keeps the brackets around an IPv6 literal. */
function stripBrackets(host: string): string {
  return host.startsWith("[") && host.endsWith("]") ? host.slice(1, -1) : host
}

/** Lower-cased hostname without IPv6 brackets, or null when `url` won't parse. */
export function hostnameOf(url: string): string | null {
  try {
    const hostname = new URL(url).hostname
    return hostname ? stripBrackets(hostname).toLowerCase() : null
  } catch {
    return null
  }
}

/**
 * True for addresses that resolve to THIS machine: `localhost` and any
 * `*.localhost` name, the whole `127/8` block, IPv6 `::1`, and the unspecified
 * addresses `0.0.0.0` / `::` (dev servers print `http://0.0.0.0:3000` and a
 * browser connects to that as local). IPv4-mapped IPv6 (`::ffff:127.0.0.1`) is
 * unwrapped first.
 */
export function isLoopbackHost(hostname: string): boolean {
  const host = stripBrackets(hostname.trim().toLowerCase())
  if (!host) return false
  if (host === "localhost" || host.endsWith(".localhost")) return true
  if (host === "::1" || host === "::" || host === "0.0.0.0") return true
  const v4 = parseIpv4(host.startsWith("::ffff:") ? host.slice(7) : host)
  return v4 !== null && v4[0] === 127
}

/**
 * True for RFC 1918 / link-local / unique-local ranges and mDNS `*.local`
 * names — hosts that only mean something on the network the machine is on.
 * Loopback is NOT included; use `isLoopbackOrPrivateHost` for the union.
 */
export function isPrivateNetworkHost(hostname: string): boolean {
  const host = stripBrackets(hostname.trim().toLowerCase())
  if (!host) return false
  if (host.endsWith(".local")) return true
  const v4 = parseIpv4(host.startsWith("::ffff:") ? host.slice(7) : host)
  if (v4) {
    const [a, b] = v4
    return (
      a === 10 ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 169 && b === 254)
    )
  }
  if (host.includes(":")) {
    // fc00::/7 (unique local) and fe80::/10 (link local).
    const first = host.split(":")[0]
    if (first.length === 4) {
      const n = Number.parseInt(first, 16)
      if (Number.isNaN(n)) return false
      return (n & 0xfe00) === 0xfc00 || (n & 0xffc0) === 0xfe80
    }
  }
  return false
}

export function isLoopbackOrPrivateHost(hostname: string): boolean {
  return isLoopbackHost(hostname) || isPrivateNetworkHost(hostname)
}

export function isLoopbackOrPrivateUrl(url: string): boolean {
  const hostname = hostnameOf(url)
  return hostname !== null && isLoopbackOrPrivateHost(hostname)
}

/** The URL's origin, or null when it is opaque (`about:blank`, `blob:` …). */
export function originOf(url: string): string | null {
  try {
    const origin = new URL(url).origin
    return origin && origin !== "null" ? origin : null
  } catch {
    return null
  }
}

/** Effective port: the explicit one, else the scheme default; null if unknown. */
export function portOf(url: string): number | null {
  try {
    const parsed = new URL(url)
    if (parsed.port) return Number(parsed.port)
    if (parsed.protocol === "http:" || parsed.protocol === "ws:") return 80
    if (parsed.protocol === "https:" || parsed.protocol === "wss:") return 443
    return null
  } catch {
    return null
  }
}

/**
 * Canonical form used to answer "is this page already open in a tab?": the
 * WHATWG-serialized href with the fragment dropped (a different `#hash` is the
 * same document). Null when the string is not a URL at all.
 */
export function normalizeUrlForDedupe(url: string): string | null {
  try {
    const parsed = new URL(url)
    parsed.hash = ""
    return parsed.href
  } catch {
    return null
  }
}

/** `host:port` as a user would type it — for the "this address lives on the
 *  remote host" hint; the default port is omitted. */
export function displayHostPort(url: string): string | null {
  try {
    const parsed = new URL(url)
    return parsed.host || null
  } catch {
    return null
  }
}
