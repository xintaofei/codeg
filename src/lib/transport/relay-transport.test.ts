import { webcrypto } from "node:crypto"
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest"

import {
  createRelayHandshakeProof,
  deriveRelayDirectionalKeys,
  deriveRelaySharedSecret,
  exportRelayPublicKey,
  generateRelayEphemeralKeyPair,
  openRelayFrame,
  relayBase64UrlDecode,
  relayBase64UrlEncode,
  relayNonce,
  sealRelayFrame,
  verifyRelayHandshakeProof,
} from "@/lib/relay/crypto"
import { relayFrameAad, type RelayFrameEnvelope } from "@/lib/relay/protocol"

import { RelayTransport } from "./relay-transport"

class MockWebSocket {
  static readonly OPEN = 1
  static instances: MockWebSocket[] = []
  readyState = 0
  closeCount = 0
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null

  constructor(readonly url: string) {
    MockWebSocket.instances.push(this)
  }

  open(): void {
    this.readyState = MockWebSocket.OPEN
    this.onopen?.()
  }

  send(value: string): void {
    this.sent.push(value)
  }

  emit(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) })
  }

  close(): void {
    this.closeCount += 1
    this.readyState = 3
  }
}

beforeAll(() => {
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: webcrypto,
  })
})

beforeEach(() => {
  MockWebSocket.instances = []
  vi.stubGlobal("WebSocket", MockWebSocket)
})

async function eventually(check: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (check()) return
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
  throw new Error("condition was not met")
}

describe("RelayTransport", () => {
  it("authenticates a session and resolves an encrypted command response", async () => {
    const pairingRoot = new Uint8Array(32).fill(0x39)
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(pairingRoot),
    })
    const socket = MockWebSocket.instances[0]
    socket.open()
    await eventually(() => socket.sent.length >= 2)

    const routingHello = JSON.parse(socket.sent[0])
    expect(routingHello.token).toBe(
      "routing-token-at-least-thirty-two-characters"
    )
    const mobileHello = JSON.parse(socket.sent[1])
    await expect(
      verifyRelayHandshakeProof(
        pairingRoot,
        mobileHello.proof,
        "mobile",
        "d_test",
        "m_phone",
        mobileHello.connection_id,
        mobileHello.public_key
      )
    ).resolves.toBe(true)

    const desktopKeys = await generateRelayEphemeralKeyPair()
    const desktopPublic = relayBase64UrlEncode(
      await exportRelayPublicKey(desktopKeys.publicKey)
    )
    const desktopProof = await createRelayHandshakeProof(
      pairingRoot,
      "desktop",
      "d_test",
      "m_phone",
      mobileHello.connection_id,
      desktopPublic
    )
    const shared = await deriveRelaySharedSecret(
      desktopKeys.privateKey,
      relayBase64UrlDecode(mobileHello.public_key)
    )
    const keys = await deriveRelayDirectionalKeys(
      shared,
      pairingRoot,
      mobileHello.connection_id
    )
    socket.emit({
      v: 1,
      type: "pair",
      phase: "desktop_hello",
      desktop_id: "d_test",
      device_id: "m_phone",
      connection_id: mobileHello.connection_id,
      public_key: desktopPublic,
      proof: desktopProof,
    })

    const resultPromise = transport.call<{ answer: number }>("get_stats", {
      scope: "mobile",
    })
    await eventually(() => socket.sent.length >= 3)
    const requestFrame = JSON.parse(socket.sent[2]) as RelayFrameEnvelope
    const requestPlaintext = await openRelayFrame(
      keys.mobileToDesktop,
      relayNonce(0x004d3244, 1n),
      relayFrameAad(requestFrame),
      relayBase64UrlDecode(requestFrame.ciphertext)
    )
    const request = JSON.parse(new TextDecoder().decode(requestPlaintext))
    expect(request).toMatchObject({
      kind: "request",
      command: "get_stats",
      args: { scope: "mobile" },
    })

    const responseNonce = relayNonce(0x0044324d, 1n)
    const responseFrame: RelayFrameEnvelope = {
      v: 1,
      type: "frame",
      desktop_id: "d_test",
      device_id: "m_phone",
      connection_id: mobileHello.connection_id,
      frame_id: "f_desktop",
      seq: 1,
      ack: 1,
      nonce: relayBase64UrlEncode(responseNonce),
      ciphertext: "",
    }
    responseFrame.ciphertext = relayBase64UrlEncode(
      await sealRelayFrame(
        keys.desktopToMobile,
        responseNonce,
        relayFrameAad(responseFrame),
        new TextEncoder().encode(
          JSON.stringify({
            kind: "response",
            request_id: request.request_id,
            ok: true,
            result: { answer: 42 },
          })
        )
      )
    )
    socket.emit(responseFrame)

    await expect(resultPromise).resolves.toEqual({ answer: 42 })
    transport.destroy()
  })

  it("serializes desktop hello and the immediately following ready frame", async () => {
    const pairingRoot = new Uint8Array(32).fill(0x41)
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(pairingRoot),
    })
    const socket = MockWebSocket.instances[0]
    socket.open()
    await eventually(() => socket.sent.length >= 2)

    const mobileHello = JSON.parse(socket.sent[1])
    const desktopKeys = await generateRelayEphemeralKeyPair()
    const desktopPublic = relayBase64UrlEncode(
      await exportRelayPublicKey(desktopKeys.publicKey)
    )
    const desktopProof = await createRelayHandshakeProof(
      pairingRoot,
      "desktop",
      "d_test",
      "m_phone",
      mobileHello.connection_id,
      desktopPublic
    )
    const shared = await deriveRelaySharedSecret(
      desktopKeys.privateKey,
      relayBase64UrlDecode(mobileHello.public_key)
    )
    const keys = await deriveRelayDirectionalKeys(
      shared,
      pairingRoot,
      mobileHello.connection_id
    )
    const readyNonce = relayNonce(0x0044324d, 1n)
    const readyFrame: RelayFrameEnvelope = {
      v: 1,
      type: "frame",
      desktop_id: "d_test",
      device_id: "m_phone",
      connection_id: mobileHello.connection_id,
      frame_id: "f_ready",
      seq: 1,
      ack: 0,
      nonce: relayBase64UrlEncode(readyNonce),
      ciphertext: "",
    }
    readyFrame.ciphertext = relayBase64UrlEncode(
      await sealRelayFrame(
        keys.desktopToMobile,
        readyNonce,
        relayFrameAad(readyFrame),
        new TextEncoder().encode(JSON.stringify({ kind: "ready" }))
      )
    )

    // Deliver both messages in the same turn, matching the production
    // bridge. The transport must finish the handshake before decrypting.
    socket.emit({
      v: 1,
      type: "pair",
      phase: "desktop_hello",
      desktop_id: "d_test",
      device_id: "m_phone",
      connection_id: mobileHello.connection_id,
      public_key: desktopPublic,
      proof: desktopProof,
    })
    socket.emit(readyFrame)

    await eventually(() => transport.getConnectionSnapshot() === "connected")
    await transport.waitForReady()
    expect(socket.closeCount).toBe(0)
    transport.destroy()
  })

  it("reassembles and verifies chunked encrypted payloads", async () => {
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(new Uint8Array(32).fill(0x52)),
    })
    const payload = new TextEncoder().encode(
      JSON.stringify({
        kind: "response",
        request_id: "r_large",
        ok: true,
        result: { value: "large payload" },
      })
    )
    const checksum = relayBase64UrlEncode(
      new Uint8Array(await crypto.subtle.digest("SHA-256", payload))
    )
    const midpoint = Math.ceil(payload.length / 2)
    const acceptChunk = (
      transport as unknown as {
        acceptChunk(chunk: Record<string, unknown>): Promise<unknown>
      }
    ).acceptChunk.bind(transport)
    const base = {
      kind: "chunk",
      chunk_id: "ch_test",
      total: 2,
      total_bytes: payload.length,
      sha256: checksum,
    }

    await expect(
      acceptChunk({
        ...base,
        index: 0,
        data: relayBase64UrlEncode(payload.slice(0, midpoint)),
      })
    ).resolves.toBeNull()
    await expect(
      acceptChunk({
        ...base,
        index: 1,
        data: relayBase64UrlEncode(payload.slice(midpoint)),
      })
    ).resolves.toEqual({
      kind: "response",
      request_id: "r_large",
      ok: true,
      result: { value: "large payload" },
    })

    transport.destroy()
  })
})
