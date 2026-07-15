import { describe, expect, it } from "vitest"

import { relayBase64UrlEncode } from "./crypto"
import { parseMobileRelayPairingPayload } from "./config"

describe("mobile Relay pairing payload", () => {
  it("parses a current v1 payload", () => {
    const pairingRoot = relayBase64UrlEncode(new Uint8Array(32).fill(3))
    expect(
      parseMobileRelayPairingPayload(
        JSON.stringify({
          v: 1,
          relay_url: "relay.example.test/v1/ws",
          desktop_id: "d_test",
          device_id: "m_phone",
          routing_token: "routing-token-at-least-thirty-two-characters",
          pairing_root: pairingRoot,
          expires_at: Math.floor(Date.now() / 1000) + 300,
        })
      )
    ).toEqual({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot,
    })
  })

  it("rejects expired and malformed payloads", () => {
    const root = relayBase64UrlEncode(new Uint8Array(32).fill(4))
    expect(() =>
      parseMobileRelayPairingPayload(
        JSON.stringify({
          v: 1,
          relay_url: "wss://relay.example.test/v1/ws",
          desktop_id: "d_test",
          device_id: "m_phone",
          routing_token: "routing-token-at-least-thirty-two-characters",
          pairing_root: root,
          expires_at: 1,
        })
      )
    ).toThrow(/过期/)
    expect(() => parseMobileRelayPairingPayload("{}")).toThrow()
  })
})
