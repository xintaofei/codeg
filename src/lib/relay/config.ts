import { relayBase64UrlDecode } from "./crypto"

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
  pair_id: string
  pair_secret: string
  desktop_public_key: string
  expires_at: number
}

export interface MobileRelayPairingPayload {
  relayUrl: string
  desktopId: string
  pairId: string
  pairSecret: string
  desktopPublicKey: string
  expiresAt: number
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

export function normalizeRelayUrl(value: string): string {
  const trimmed = value.trim()
  if (!trimmed) return ""
  const withScheme = /^wss?:\/\//i.test(trimmed) ? trimmed : `wss://${trimmed}`
  return withScheme.replace(/\/+$/, "")
}

function isLoopbackRelayHost(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, "")
  return (
    normalized === "localhost" ||
    normalized === "127.0.0.1" ||
    normalized === "::1"
  )
}

function requireSecureRelayUrl(value: string): string {
  const normalized = normalizeRelayUrl(value)
  let url: URL
  try {
    url = new URL(normalized)
  } catch {
    throw new Error("Relay 地址无效")
  }
  const secure = url.protocol === "wss:"
  const loopbackDevelopment =
    url.protocol === "ws:" && isLoopbackRelayHost(url.hostname)
  if (
    (!secure && !loopbackDevelopment) ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  ) {
    throw new Error("Relay 必须使用 wss://；ws:// 仅允许本机开发地址")
  }
  return url.toString().replace(/\/$/, "")
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
  let relayUrl = ""
  try {
    relayUrl = requireSecureRelayUrl(localStorage.getItem(RELAY_URL_KEY) ?? "")
  } catch {
    return null
  }
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
  let relayUrl = ""
  try {
    relayUrl = requireSecureRelayUrl(localStorage.getItem(RELAY_URL_KEY) ?? "")
  } catch {
    // Corrupt or legacy insecure settings should send the user back through
    // pairing instead of reconnecting with a weaker transport.
  }
  return {
    relayUrl,
    desktopId: localStorage.getItem(RELAY_DESKTOP_ID_KEY)?.trim() ?? "",
    deviceId: localStorage.getItem(RELAY_DEVICE_ID_KEY)?.trim() ?? "",
  }
}

export async function setMobileRelayConfig(
  config: MobileRelayConfig
): Promise<void> {
  const relayUrl = requireSecureRelayUrl(config.relayUrl)
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
): MobileRelayPairingPayload {
  let payload: PairingPayload
  try {
    payload = JSON.parse(value) as PairingPayload
  } catch {
    throw new Error("配对内容不是有效 JSON")
  }
  if (payload.v !== 2) throw new Error("不支持或不安全的 Relay 配对版本")
  if (!Number.isSafeInteger(payload.expires_at)) {
    throw new Error("配对内容缺少有效期限")
  }
  if (payload.expires_at * 1000 < Date.now()) {
    throw new Error("配对二维码已过期")
  }
  let relayUrl = ""
  try {
    relayUrl = requireSecureRelayUrl(payload.relay_url ?? "")
  } catch {
    throw new Error("配对内容中的 Relay 地址不安全")
  }
  const ids = [payload.desktop_id, payload.pair_id]
  if (
    !relayUrl ||
    !ids.every(
      (id) =>
        typeof id === "string" &&
        id.length >= 3 &&
        id.length <= 128 &&
        /^[A-Za-z0-9._:-]+$/.test(id)
    ) ||
    typeof payload.pair_secret !== "string" ||
    typeof payload.desktop_public_key !== "string"
  ) {
    throw new Error("配对内容缺少有效的一次性凭据")
  }
  let pairSecret: Uint8Array
  let desktopPublicKey: Uint8Array
  try {
    pairSecret = relayBase64UrlDecode(payload.pair_secret)
    desktopPublicKey = relayBase64UrlDecode(payload.desktop_public_key)
  } catch {
    throw new Error("配对密钥格式无效")
  }
  if (pairSecret.length !== 32) throw new Error("配对密钥长度无效")
  if (desktopPublicKey.length !== 65 || desktopPublicKey[0] !== 4) {
    throw new Error("电脑配对公钥无效")
  }
  return {
    relayUrl,
    desktopId: payload.desktop_id,
    pairId: payload.pair_id,
    pairSecret: payload.pair_secret,
    desktopPublicKey: payload.desktop_public_key,
    expiresAt: payload.expires_at,
  }
}
