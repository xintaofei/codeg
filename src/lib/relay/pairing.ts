import type { MobileRelayConfig, MobileRelayPairingPayload } from "./config"
import {
  deriveMobilePairingMaterial,
  exportRelayPublicKey,
  generateRelayEphemeralKeyPair,
  openMobilePairingAccept,
  relayBase64UrlDecode,
  relayBase64UrlEncode,
} from "./crypto"

const decoder = new TextDecoder()
const POLL_INTERVAL_MS = 750
const COMPLETE_TIMEOUT_MS = 2_000
const COMPLETE_RETRY_DELAYS_MS = [0, 250, 750] as const

interface PairingStatusResponse {
  status: "waiting" | "accepted" | "rejected" | "consumed"
  expires_at: number
  nonce?: string
  ciphertext?: string
}

interface PairingAcceptPayload {
  v: number
  desktop_id: string
  device_id: string
  routing_token: string
  expires_at: number
}

export interface MobileRelayPairingProgress {
  status: "waiting_confirmation"
  sas: string
  deviceId: string
}

function relayHttpEndpoint(
  relayUrl: string,
  pairId: string,
  suffix = ""
): string {
  const url = new URL(relayUrl)
  url.protocol = url.protocol === "wss:" ? "https:" : "http:"
  url.pathname = `/v1/pairings/${encodeURIComponent(pairId)}${suffix}`
  url.search = ""
  url.hash = ""
  return url.toString()
}

function mobileDeviceName(): string {
  if (/Android/i.test(navigator.userAgent)) return "Android device"
  if (/iPhone|iPad|iPod/i.test(navigator.userAgent)) return "iOS device"
  return "Codeg Mobile"
}

async function pairingFetch(
  url: string,
  init?: RequestInit
): Promise<Response> {
  const response = await fetch(url, {
    ...init,
    cache: "no-store",
    headers: {
      "Content-Type": "application/json",
      ...init?.headers,
    },
  })
  if (response.ok) return response
  let code = "pairing_failed"
  try {
    const body = (await response.json()) as { error?: string }
    if (typeof body.error === "string") code = body.error
  } catch {
    // Preserve the stable fallback below when a proxy returns a non-JSON body.
  }
  if (code === "pair_expired") throw new Error("配对二维码已过期")
  if (code === "pair_consumed") throw new Error("此配对二维码已被使用")
  if (code === "pair_device_mismatch") throw new Error("配对设备身份不匹配")
  throw new Error(`Relay 配对失败（${code}，HTTP ${response.status}）`)
}

function wait(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(new DOMException("Pairing canceled", "AbortError"))
      return
    }
    const onAbort = () => {
      globalThis.clearTimeout(timer)
      reject(new DOMException("Pairing canceled", "AbortError"))
    }
    const timer = globalThis.setTimeout(() => {
      signal?.removeEventListener("abort", onAbort)
      resolve()
    }, ms)
    signal?.addEventListener("abort", onAbort, { once: true })
  })
}

async function acknowledgePairingComplete(
  payload: MobileRelayPairingPayload,
  deviceId: string
): Promise<void> {
  for (const retryDelay of COMPLETE_RETRY_DELAYS_MS) {
    if (retryDelay > 0) await wait(retryDelay)
    const controller = new AbortController()
    const timeout = globalThis.setTimeout(
      () => controller.abort(),
      COMPLETE_TIMEOUT_MS
    )
    try {
      await pairingFetch(
        relayHttpEndpoint(payload.relayUrl, payload.pairId, "/complete"),
        {
          method: "POST",
          body: JSON.stringify({ device_id: deviceId }),
          signal: controller.signal,
        }
      )
      return
    } catch {
      // The encrypted, device-bound credential is already valid. Completion only
      // turns the accepted pairing into a short-lived tombstone, so a transient
      // acknowledgement failure must not discard credentials the desktop issued.
    } finally {
      globalThis.clearTimeout(timeout)
    }
  }
}

export async function completeMobileRelayPairing(
  payload: MobileRelayPairingPayload,
  onProgress?: (progress: MobileRelayPairingProgress) => void,
  signal?: AbortSignal
): Promise<MobileRelayConfig> {
  if (payload.expiresAt * 1000 <= Date.now()) {
    throw new Error("配对二维码已过期")
  }
  const deviceId = `m_${crypto.randomUUID().replace(/-/g, "")}`
  const keyPair = await generateRelayEphemeralKeyPair()
  const mobilePublicKey = relayBase64UrlEncode(
    await exportRelayPublicKey(keyPair.publicKey)
  )
  const pairSecret = relayBase64UrlDecode(payload.pairSecret)
  const material = await deriveMobilePairingMaterial(
    keyPair.privateKey,
    relayBase64UrlDecode(payload.desktopPublicKey),
    pairSecret,
    payload.desktopId,
    payload.pairId,
    deviceId
  )
  const pairingUrl = relayHttpEndpoint(payload.relayUrl, payload.pairId)
  await pairingFetch(
    relayHttpEndpoint(payload.relayUrl, payload.pairId, "/request"),
    {
      method: "POST",
      body: JSON.stringify({
        device_id: deviceId,
        device_name: mobileDeviceName(),
        mobile_public_key: mobilePublicKey,
      }),
      signal,
    }
  )
  onProgress?.({
    status: "waiting_confirmation",
    sas: material.sas,
    deviceId,
  })

  while (Date.now() < payload.expiresAt * 1000) {
    const response = await pairingFetch(
      `${pairingUrl}?device_id=${encodeURIComponent(deviceId)}`,
      { signal }
    )
    const status = (await response.json()) as PairingStatusResponse
    if (status.status === "rejected") {
      throw new Error("电脑已拒绝此次配对")
    }
    if (status.status === "consumed") {
      throw new Error("此配对二维码已被使用")
    }
    if (status.status === "accepted") {
      if (
        typeof status.nonce !== "string" ||
        typeof status.ciphertext !== "string"
      ) {
        throw new Error("Relay 返回的配对确认不完整")
      }
      const plaintext = await openMobilePairingAccept(
        material.acceptKey,
        relayBase64UrlDecode(status.nonce),
        relayBase64UrlDecode(status.ciphertext),
        payload.desktopId,
        payload.pairId,
        deviceId
      )
      let accepted: PairingAcceptPayload
      try {
        accepted = JSON.parse(decoder.decode(plaintext)) as PairingAcceptPayload
      } catch {
        throw new Error("电脑配对确认内容无效")
      }
      if (
        accepted.v !== 2 ||
        accepted.desktop_id !== payload.desktopId ||
        accepted.device_id !== deviceId ||
        typeof accepted.routing_token !== "string" ||
        accepted.routing_token.length < 32 ||
        accepted.expires_at !== payload.expiresAt
      ) {
        throw new Error("电脑配对确认与当前设备不匹配")
      }
      await acknowledgePairingComplete(payload, deviceId)
      return {
        relayUrl: payload.relayUrl,
        desktopId: payload.desktopId,
        deviceId,
        routingToken: accepted.routing_token,
        pairingRoot: relayBase64UrlEncode(material.pairingRoot),
      }
    }
    await wait(POLL_INTERVAL_MS, signal)
  }
  throw new Error("配对二维码已过期")
}
