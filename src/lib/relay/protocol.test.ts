import { describe, expect, it } from "vitest"

import { isRelayFrameEnvelope, relayFrameAad } from "./protocol"

describe("Codeg Relay v1 envelope", () => {
  const valid = {
    v: 1 as const,
    type: "frame" as const,
    desktop_id: "d_desktop",
    device_id: "m_phone",
    connection_id: "c_connection",
    frame_id: "f_019f",
    seq: 4,
    ack: 3,
    nonce: "AAAAAAAAAAAAAAAA",
    ciphertext: "encrypted",
  }

  it("accepts the bounded v1 routing envelope", () => {
    expect(isRelayFrameEnvelope(valid)).toBe(true)
    expect(new TextDecoder().decode(relayFrameAad(valid))).toBe(
      "1|d_desktop|m_phone|c_connection|f_019f|4|3"
    )
  })

  it("rejects downgrade, invalid sequence and unsafe identifiers", () => {
    expect(isRelayFrameEnvelope({ ...valid, v: 0 })).toBe(false)
    expect(isRelayFrameEnvelope({ ...valid, seq: 0 })).toBe(false)
    expect(isRelayFrameEnvelope({ ...valid, device_id: "../phone" })).toBe(
      false
    )
  })
})
