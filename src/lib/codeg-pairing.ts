/**
 * Desktop Web Service QR payload. Keep in lock-step with
 * CodegiOS/Networking/CodegPairing.swift.
 */

export type PairingMode = "local" | "private" | "public"

export type PairingPayload = {
  url: string
  token: string
  mode: PairingMode
  name: string
}

export function pairingQrValue(input: {
  url: string
  token: string
  mode: PairingMode
  name?: string
}): string {
  const token = input.token.trim()
  if (!token) return input.url
  const payload: PairingPayload = {
    url: input.url,
    token,
    mode: input.mode,
    name: input.name ?? "Codeg",
  }
  return JSON.stringify(payload)
}

export function pairingMode(input: {
  serveEnabled?: boolean
  funnelEnabled?: boolean
}): PairingMode {
  if (input.serveEnabled) return "private"
  if (input.funnelEnabled) return "public"
  return "local"
}
