const RELAY_URL_KEY = "codeg_mobile_relay_url"
const RELAY_DESKTOP_ID_KEY = "codeg_mobile_relay_desktop_id"
const RELAY_DEVICE_ID_KEY = "codeg_mobile_relay_device_id"
const RELAY_ROUTING_TOKEN_KEY = "codeg_mobile_relay_routing_token"
const RELAY_PAIRING_ROOT_KEY = "codeg_mobile_relay_pairing_root"

export interface MobileRelayPublicConfig {
  relayUrl: string
  desktopId: string
  deviceId: string
}

export interface MobileRelaySecrets {
  routingToken: string
  pairingRoot: string
}

export interface MobileRelayConfig
  extends MobileRelayPublicConfig, MobileRelaySecrets {}

interface PairingPayload {
  v: number
  relay_url: string
  desktop_id: string
  device_id: string
  routing_token: string
  pairing_root: string
  expires_at?: number
}

let relaySecrets: MobileRelaySecrets = {
  routingToken: "",
  pairingRoot: "",
}

function isMobileTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
  )
}

function normalizeRelayUrl(value: string): string {
  const trimmed = value.trim()
  if (!trimmed) return ""
  const withScheme = /^wss?:\/\//i.test(trimmed) ? trimmed : `wss://${trimmed}`
  return withScheme.replace(/\/+$/, "")
}

async function loadSecret(key: string): Promise<string> {
  if (!isMobileTauri()) return sessionStorage.getItem(key) ?? ""
  const { invoke } = await import("@tauri-apps/api/core")
  const result = await invoke<{ value?: string }>(
    "plugin:secure-vault|load_secret",
    { payload: { key } }
  )
  return result.value ?? ""
}

async function storeSecret(key: string, value: string): Promise<void> {
  if (!isMobileTauri()) {
    sessionStorage.setItem(key, value)
    return
  }
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("plugin:secure-vault|store_secret", {
    payload: { key, value },
  })
}

async function deleteSecret(key: string): Promise<void> {
  sessionStorage.removeItem(key)
  if (!isMobileTauri()) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("plugin:secure-vault|delete_secret", {
    payload: { key },
  })
}

export async function bootstrapMobileRelaySecrets(): Promise<void> {
  const [routingToken, pairingRoot] = await Promise.all([
    loadSecret(RELAY_ROUTING_TOKEN_KEY),
    loadSecret(RELAY_PAIRING_ROOT_KEY),
  ])
  relaySecrets = { routingToken, pairingRoot }
  localStorage.removeItem(RELAY_ROUTING_TOKEN_KEY)
  localStorage.removeItem(RELAY_PAIRING_ROOT_KEY)
}

export function getMobileRelayConfig(): MobileRelayConfig | null {
  if (typeof window === "undefined") return null
  const relayUrl = normalizeRelayUrl(localStorage.getItem(RELAY_URL_KEY) ?? "")
  const desktopId = localStorage.getItem(RELAY_DESKTOP_ID_KEY)?.trim() ?? ""
  const deviceId = localStorage.getItem(RELAY_DEVICE_ID_KEY)?.trim() ?? ""
  if (
    !relayUrl ||
    !desktopId ||
    !deviceId ||
    !relaySecrets.routingToken ||
    !relaySecrets.pairingRoot
  ) {
    return null
  }
  return { relayUrl, desktopId, deviceId, ...relaySecrets }
}

export function getMobileRelayPublicConfig(): MobileRelayPublicConfig {
  if (typeof window === "undefined") {
    return { relayUrl: "", desktopId: "", deviceId: "" }
  }
  return {
    relayUrl: normalizeRelayUrl(localStorage.getItem(RELAY_URL_KEY) ?? ""),
    desktopId: localStorage.getItem(RELAY_DESKTOP_ID_KEY)?.trim() ?? "",
    deviceId: localStorage.getItem(RELAY_DEVICE_ID_KEY)?.trim() ?? "",
  }
}

export async function setMobileRelayConfig(
  config: MobileRelayConfig
): Promise<void> {
  const relayUrl = normalizeRelayUrl(config.relayUrl)
  if (!relayUrl || !config.desktopId.trim() || !config.deviceId.trim()) {
    throw new Error("Relay configuration is incomplete")
  }
  await Promise.all([
    storeSecret(RELAY_ROUTING_TOKEN_KEY, config.routingToken.trim()),
    storeSecret(RELAY_PAIRING_ROOT_KEY, config.pairingRoot.trim()),
  ])
  relaySecrets = {
    routingToken: config.routingToken.trim(),
    pairingRoot: config.pairingRoot.trim(),
  }
  localStorage.setItem(RELAY_URL_KEY, relayUrl)
  localStorage.setItem(RELAY_DESKTOP_ID_KEY, config.desktopId.trim())
  localStorage.setItem(RELAY_DEVICE_ID_KEY, config.deviceId.trim())
}

export async function clearMobileRelayConfig(): Promise<void> {
  relaySecrets = { routingToken: "", pairingRoot: "" }
  localStorage.removeItem(RELAY_URL_KEY)
  localStorage.removeItem(RELAY_DESKTOP_ID_KEY)
  localStorage.removeItem(RELAY_DEVICE_ID_KEY)
  await Promise.all([
    deleteSecret(RELAY_ROUTING_TOKEN_KEY),
    deleteSecret(RELAY_PAIRING_ROOT_KEY),
  ])
}

export function parseMobileRelayPairingPayload(
  value: string
): MobileRelayConfig {
  let payload: PairingPayload
  try {
    payload = JSON.parse(value) as PairingPayload
  } catch {
    throw new Error("配对内容不是有效 JSON")
  }
  if (payload.v !== 1) throw new Error("不支持的 Relay 配对版本")
  if (
    typeof payload.expires_at === "number" &&
    payload.expires_at * 1000 < Date.now()
  ) {
    throw new Error("配对二维码已过期")
  }
  const relayUrl = normalizeRelayUrl(payload.relay_url ?? "")
  const ids = [payload.desktop_id, payload.device_id]
  if (
    !relayUrl ||
    !ids.every(
      (id) =>
        typeof id === "string" &&
        id.length >= 3 &&
        id.length <= 128 &&
        /^[A-Za-z0-9._:-]+$/.test(id)
    ) ||
    typeof payload.routing_token !== "string" ||
    payload.routing_token.length < 32 ||
    typeof payload.pairing_root !== "string"
  ) {
    throw new Error("配对内容缺少有效的 Relay 凭据")
  }
  const padding = "=".repeat((4 - (payload.pairing_root.length % 4)) % 4)
  let root: string
  try {
    root = atob(
      payload.pairing_root.replace(/-/g, "+").replace(/_/g, "/") + padding
    )
  } catch {
    throw new Error("配对根密钥格式无效")
  }
  if (root.length !== 32) throw new Error("配对根密钥长度无效")
  return {
    relayUrl,
    desktopId: payload.desktop_id,
    deviceId: payload.device_id,
    routingToken: payload.routing_token,
    pairingRoot: payload.pairing_root,
  }
}
