import { webcrypto } from "node:crypto"
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest"

import type { MobileRelayPairingPayload } from "./config"
import {
  deriveRelaySharedSecret,
  exportRelayPublicKey,
  generateRelayEphemeralKeyPair,
  relayBase64UrlDecode,
  relayBase64UrlEncode,
} from "./crypto"
import { completeMobileRelayPairing } from "./pairing"

const encoder = new TextEncoder()

beforeAll(() => {
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: webcrypto,
  })
})

describe("mobile Relay v2 pairing", () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it("keeps a valid device-bound credential when completion acknowledgement is offline", async () => {
    const desktop = await generateRelayEphemeralKeyPair()
    const pairSecret = crypto.getRandomValues(new Uint8Array(32))
    const desktopId = "d_desktop"
    const pairId = "p_one_time"
    const expiresAt = Math.floor(Date.now() / 1000) + 300
    const payload: MobileRelayPairingPayload = {
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId,
      pairId,
      pairSecret: relayBase64UrlEncode(pairSecret),
      desktopPublicKey: relayBase64UrlEncode(
        await exportRelayPublicKey(desktop.publicKey)
      ),
      expiresAt,
    }
    const routingToken = "r_abcdefghijklmnopqrstuvwxyz0123456789"
    let requestedDeviceId = ""
    let requestedMobilePublicKey = ""
    let expectedSas = ""
    let expectedPairingRoot = new Uint8Array()
    let completeAttempts = 0

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
        const url = new URL(
          typeof input === "string" || input instanceof URL
            ? input.toString()
            : input.url
        )
        if (url.pathname.endsWith("/request")) {
          const request = JSON.parse(String(init?.body)) as {
            device_id: string
            mobile_public_key: string
          }
          requestedDeviceId = request.device_id
          requestedMobilePublicKey = request.mobile_public_key
          return new Response(null, { status: 202 })
        }
        if (url.pathname.endsWith("/complete")) {
          completeAttempts += 1
          throw new TypeError("Relay temporarily offline")
        }

        const context = `codeg-relay-pair-v2|${desktopId}|${pairId}|${requestedDeviceId}`
        const sharedSecret = await deriveRelaySharedSecret(
          desktop.privateKey,
          relayBase64UrlDecode(requestedMobilePublicKey)
        )
        const keyMaterial = await crypto.subtle.importKey(
          "raw",
          sharedSecret,
          "HKDF",
          false,
          ["deriveBits", "deriveKey"]
        )
        expectedPairingRoot = new Uint8Array(
          await crypto.subtle.deriveBits(
            {
              name: "HKDF",
              hash: "SHA-256",
              salt: pairSecret,
              info: encoder.encode(`${context}|pairing-root`),
            },
            keyMaterial,
            256
          )
        )
        const sasKey = await crypto.subtle.importKey(
          "raw",
          expectedPairingRoot,
          { name: "HMAC", hash: "SHA-256" },
          false,
          ["sign"]
        )
        const sasDigest = new Uint8Array(
          await crypto.subtle.sign(
            "HMAC",
            sasKey,
            encoder.encode(`${context}|sas`)
          )
        )
        expectedSas = (
          new DataView(
            sasDigest.buffer,
            sasDigest.byteOffset,
            sasDigest.byteLength
          ).getUint32(0, false) % 1_000_000
        )
          .toString()
          .padStart(6, "0")
        const acceptKey = await crypto.subtle.deriveKey(
          {
            name: "HKDF",
            hash: "SHA-256",
            salt: pairSecret,
            info: encoder.encode(`${context}|accept-key`),
          },
          keyMaterial,
          { name: "AES-GCM", length: 256 },
          false,
          ["encrypt"]
        )
        const nonce = crypto.getRandomValues(new Uint8Array(12))
        const plaintext = encoder.encode(
          JSON.stringify({
            v: 2,
            desktop_id: desktopId,
            device_id: requestedDeviceId,
            routing_token: routingToken,
            expires_at: expiresAt,
          })
        )
        const ciphertext = new Uint8Array(
          await crypto.subtle.encrypt(
            {
              name: "AES-GCM",
              iv: nonce,
              additionalData: encoder.encode(
                `codeg-relay-pair-v2|accept|${desktopId}|${pairId}|${requestedDeviceId}`
              ),
              tagLength: 128,
            },
            acceptKey,
            plaintext
          )
        )
        return Response.json({
          status: "accepted",
          expires_at: expiresAt,
          nonce: relayBase64UrlEncode(nonce),
          ciphertext: relayBase64UrlEncode(ciphertext),
        })
      })
    )

    const progress = vi.fn()
    const config = await completeMobileRelayPairing(payload, progress)

    expect(progress).toHaveBeenCalledWith({
      status: "waiting_confirmation",
      sas: expectedSas,
      deviceId: requestedDeviceId,
    })
    expect(config).toEqual({
      relayUrl: payload.relayUrl,
      desktopId,
      deviceId: requestedDeviceId,
      routingToken,
      pairingRoot: relayBase64UrlEncode(expectedPairingRoot),
    })
    expect(completeAttempts).toBe(3)
  })
})
