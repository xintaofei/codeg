/**
 * Tailscale Serve / Funnel command builders.
 *
 * Serve is private to the tailnet (same privacy as Tailscale on both devices).
 * Funnel is public HTTPS. Relays do not decrypt either path.
 * The local target is always loopback.
 */

export class FunnelError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "FunnelError"
  }
}

export function funnelTarget(port: number): string {
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new FunnelError("port must be an integer 1-65535")
  }
  return `http://127.0.0.1:${port}`
}

export function serveEnableArgs(port: number): string[] {
  return ["serve", "--bg", "--yes", funnelTarget(port)]
}

export function serveDisableArgs(): string[] {
  return ["serve", "reset"]
}

export function serveStatusArgs(): string[] {
  return ["serve", "status", "--json"]
}

export function funnelEnableArgs(port: number): string[] {
  return ["funnel", "--bg", "--yes", funnelTarget(port)]
}

export function funnelDisableArgs(): string[] {
  return ["funnel", "reset"]
}

export function funnelStatusArgs(): string[] {
  return ["funnel", "status", "--json"]
}

export function isLoopbackTarget(target: string): boolean {
  try {
    const url = new URL(target)
    return url.hostname === "127.0.0.1" || url.hostname === "localhost"
  } catch {
    return false
  }
}

/** Public Funnel HTTPS first, then local bind addresses. */
export function displayAddresses(
  local: string[],
  funnelUrl?: string | null
): string[] {
  if (funnelUrl && !local.includes(funnelUrl)) {
    return [funnelUrl, ...local]
  }
  return local
}
