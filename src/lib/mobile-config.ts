const MOBILE_SERVER_URL_KEY = "codeg_mobile_server_url"
const MOBILE_CONNECTION_MODE_KEY = "codeg_mobile_connection_mode"

export type MobileConnectionMode = "direct" | "relay"

export function normalizeServerUrl(value: string): string {
  const trimmed = value.trim()
  if (!trimmed) return ""
  const withScheme = /^https?:\/\//i.test(trimmed)
    ? trimmed
    : `https://${trimmed}`
  return withScheme.replace(/\/+$/, "")
}

export function getMobileServerUrl(): string {
  if (typeof window === "undefined") return ""
  return normalizeServerUrl(localStorage.getItem(MOBILE_SERVER_URL_KEY) ?? "")
}

export function setMobileServerUrl(value: string): string {
  const normalized = normalizeServerUrl(value)
  if (!normalized) {
    localStorage.removeItem(MOBILE_SERVER_URL_KEY)
    return ""
  }
  localStorage.setItem(MOBILE_SERVER_URL_KEY, normalized)
  return normalized
}

export function clearMobileServer(): void {
  localStorage.removeItem(MOBILE_SERVER_URL_KEY)
  localStorage.removeItem(MOBILE_CONNECTION_MODE_KEY)
  localStorage.removeItem("codeg_token")
}

export function getMobileConnectionMode(): MobileConnectionMode {
  if (typeof window === "undefined") return "direct"
  return localStorage.getItem(MOBILE_CONNECTION_MODE_KEY) === "relay"
    ? "relay"
    : "direct"
}

export function setMobileConnectionMode(mode: MobileConnectionMode): void {
  localStorage.setItem(MOBILE_CONNECTION_MODE_KEY, mode)
}
