import { describe, expect, it } from "vitest"

import { relayBase64UrlEncode } from "./crypto"
import { parseMobileRelayPairingPayload } from "./config"

describe("mobile Relay pairing payload", () => {
  it("parses a one-time v2 payload without final routing credentials", () => {
    const pairSecret = relayBase64UrlEncode(new Uint8Array(32).fill(3))
    const desktopPublicKey = new Uint8Array(65).fill(7)
    desktopPublicKey[0] = 4
    const expiresAt = Math.floor(Date.now() / 1000) + 300
    expect(
      parseMobileRelayPairingPayload(
        JSON.stringify({
          v: 2,
          relay_url: "relay.example.test/v1/ws",
          desktop_id: "d_test",
          pair_id: "p_one_time",
          pair_secret: pairSecret,
          desktop_public_key: relayBase64UrlEncode(desktopPublicKey),
          expires_at: expiresAt,
        })
      )
    ).toEqual({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      pairId: "p_one_time",
      pairSecret,
      desktopPublicKey: relayBase64UrlEncode(desktopPublicKey),
      expiresAt,
    })
  })

  it("rejects expired and malformed payloads", () => {
    const secret = relayBase64UrlEncode(new Uint8Array(32).fill(4))
    const publicKey = new Uint8Array(65).fill(5)
    publicKey[0] = 4
    expect(() =>
      parseMobileRelayPairingPayload(
        JSON.stringify({
          v: 2,
          relay_url: "wss://relay.example.test/v1/ws",
          desktop_id: "d_test",
          pair_id: "p_expired",
          pair_secret: secret,
          desktop_public_key: relayBase64UrlEncode(publicKey),
          expires_at: 1,
        })
      )
    ).toThrow(/过期/)
    expect(() => parseMobileRelayPairingPayload("{}")).toThrow()
  })

  it("rejects legacy QR payloads that contain reusable credentials", () => {
    expect(() =>
      parseMobileRelayPairingPayload(
        JSON.stringify({
          v: 1,
          relay_url: "wss://relay.example.test/v1/ws",
          desktop_id: "d_test",
          device_id: "m_phone",
          routing_token: "reusable-token",
          pairing_root: "reusable-root",
          expires_at: Math.floor(Date.now() / 1000) + 300,
        })
      )
    ).toThrow(/不安全/)
  })
})
