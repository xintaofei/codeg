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

import { RelayCallError, RelayTransport } from "./relay-transport"

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
  it("propagates AbortSignal cancellation to the encrypted desktop session", async () => {
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(new Uint8Array(32).fill(0x21)),
    })
    const sendEncrypted = vi.fn().mockResolvedValue(undefined)
    const internal = transport as unknown as {
      keys: object | null
      sendEncrypted: (payload: unknown) => Promise<void>
      sendPending: () => Promise<void>
    }
    internal.keys = {}
    internal.sendEncrypted = sendEncrypted
    internal.sendPending = vi.fn().mockResolvedValue(undefined)

    const controller = new AbortController()
    const result = transport.call(
      "get_stats",
      {},
      { signal: controller.signal }
    )
    controller.abort()

    await expect(result).rejects.toMatchObject({ name: "AbortError" })
    expect(sendEncrypted).toHaveBeenCalledWith({
      kind: "cancel",
      request_id: expect.stringMatching(/^r_/),
    })
    transport.destroy()
  })

  it("maps structured Relay failures to a standard Error with a code", async () => {
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(new Uint8Array(32).fill(0x22)),
    })
    const internal = transport as unknown as {
      pending: Map<string, unknown>
      sendPending: () => Promise<void>
      finishPending(id: string, success: boolean, value: unknown): void
    }
    internal.sendPending = vi.fn().mockResolvedValue(undefined)
    const result = transport.call("get_stats")
    const requestId = [...internal.pending.keys()][0]
    internal.finishPending(requestId, false, {
      code: "codeg_unreachable",
      message: "Local Codeg is unavailable",
    })

    await expect(result).rejects.toEqual(
      expect.objectContaining<Partial<RelayCallError>>({
        name: "RelayCallError",
        code: "codeg_unreachable",
        message: "Local Codeg is unavailable",
      })
    )
    transport.destroy()
  })

  it("chunks large requests and advances progress only after desktop acknowledgements", async () => {
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(new Uint8Array(32).fill(0x23)),
    })
    const progress = vi.fn()
    const chunks: Array<Record<string, unknown>> = []
    let requestedRestart = false
    const internal = transport as unknown as {
      sessionPromise: Promise<void>
      pending: Map<string, unknown>
      sendEncrypted(payload: Record<string, unknown>): Promise<void>
      dispatchDesktopPayload(payload: Record<string, unknown>): Promise<void>
      finishPending(id: string, success: boolean, value: unknown): void
    }
    internal.sessionPromise = Promise.resolve()
    internal.sendEncrypted = vi.fn(async (payload: Record<string, unknown>) => {
      expect(payload.kind).toBe("chunk")
      chunks.push(payload)
      queueMicrotask(() => {
        const index = Number(payload.index)
        const nextIndex =
          index === 1 && !requestedRestart
            ? ((requestedRestart = true), 0)
            : index + 1
        void internal.dispatchDesktopPayload({
          kind: "chunk_ack",
          chunk_id: payload.chunk_id,
          next_index: nextIndex,
        })
      })
    })

    const result = transport.call<{ uploaded: boolean }>(
      "relay_upload_attachment",
      { dataBase64: "x".repeat(700_000) },
      { onProgress: progress }
    )
    await eventually(() => chunks.length === 5)
    const requestId = chunks[0].request_id as string
    expect(chunks.map((chunk) => chunk.index)).toEqual([0, 1, 0, 1, 2])
    expect(new Set(chunks.map((chunk) => chunk.chunk_id)).size).toBe(1)
    expect(new Set(chunks.map((chunk) => chunk.request_id))).toEqual(
      new Set([requestId])
    )
    expect(progress.mock.calls[0]).toEqual([0, expect.any(Number)])
    const finalProgress = progress.mock.calls[progress.mock.calls.length - 1]
    expect(finalProgress?.[0]).toBe(finalProgress?.[1])

    internal.finishPending(requestId, true, { uploaded: true })
    await expect(result).resolves.toEqual({ uploaded: true })
    transport.destroy()
  })

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

  it("dispatches a long ordered event stream once and closes on replay", async () => {
    const transport = new RelayTransport({
      relayUrl: "wss://relay.example.test/v1/ws",
      desktopId: "d_test",
      deviceId: "m_phone",
      routingToken: "routing-token-at-least-thirty-two-characters",
      pairingRoot: relayBase64UrlEncode(new Uint8Array(32).fill(0x51)),
    })
    const socket = MockWebSocket.instances[0]
    socket.readyState = MockWebSocket.OPEN
    const key = await crypto.subtle.importKey(
      "raw",
      new Uint8Array(32).fill(0x63),
      "AES-GCM",
      false,
      ["encrypt", "decrypt"]
    )
    const eventHandler = vi.fn()
    const connectionId = "c_long_session"
    const internal = transport as unknown as {
      keys: { mobileToDesktop: CryptoKey; desktopToMobile: CryptoKey } | null
      connectionId: string
      handlers: Map<string, Set<(payload: unknown) => void>>
    }
    internal.keys = { mobileToDesktop: key, desktopToMobile: key }
    internal.connectionId = connectionId
    internal.handlers.set("task.updated", new Set([eventHandler]))

    let lastFrame: RelayFrameEnvelope | null = null
    for (let sequence = 1; sequence <= 512; sequence++) {
      const nonce = relayNonce(0x0044324d, BigInt(sequence))
      const frame: RelayFrameEnvelope = {
        v: 1,
        type: "frame",
        desktop_id: "d_test",
        device_id: "m_phone",
        connection_id: connectionId,
        frame_id: `f_event_${sequence}`,
        seq: sequence,
        ack: 0,
        nonce: relayBase64UrlEncode(nonce),
        ciphertext: "",
      }
      frame.ciphertext = relayBase64UrlEncode(
        await sealRelayFrame(
          key,
          nonce,
          relayFrameAad(frame),
          new TextEncoder().encode(
            JSON.stringify({
              kind: "ws_frame",
              frame: {
                channel: "task.updated",
                payload: { sequence },
              },
            })
          )
        )
      )
      lastFrame = frame
      socket.emit(frame)
    }

    await eventually(() => eventHandler.mock.calls.length === 512)
    expect(eventHandler.mock.calls[0]).toEqual([{ sequence: 1 }])
    expect(eventHandler.mock.calls[511]).toEqual([{ sequence: 512 }])

    socket.emit(lastFrame)
    await eventually(() => socket.closeCount === 1)
    expect(eventHandler).toHaveBeenCalledTimes(512)
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
