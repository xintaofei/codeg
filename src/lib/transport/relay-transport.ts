import type { MobileRelayConfig } from "@/lib/relay/config"
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
  type RelayDirectionalKeys,
} from "@/lib/relay/crypto"
import {
  RELAY_PROTOCOL_VERSION,
  isRelayFrameEnvelope,
  relayFrameAad,
  type RelayFrameEnvelope,
} from "@/lib/relay/protocol"

import type { AttachTransportHost } from "./web-event-stream"
import { WebEventStream } from "./web-event-stream"
import type { WebConnState } from "./web-transport"
import type {
  CallOptions,
  EventStream,
  Transport,
  UnsubscribeFn,
} from "./types"

const DEFAULT_CALL_TIMEOUT_MS = 60_000
const READY_TIMEOUT_MS = 5_000
const RECONNECT_MAX_MS = 4_000
const MOBILE_TO_DESKTOP_NONCE_TAG = 0x004d3244
const DESKTOP_TO_MOBILE_NONCE_TAG = 0x0044324d

interface PairEnvelope {
  v: 1
  type: "pair"
  phase: "mobile_hello" | "desktop_hello"
  desktop_id: string
  device_id: string
  connection_id: string
  public_key: string
  proof: string
}

interface PendingCall {
  requestId: string
  command: string
  args: Record<string, unknown>
  idempotencyKey: string
  timeoutMs: number
  resolve(value: unknown): void
  reject(reason: unknown): void
  timer: ReturnType<typeof setTimeout>
}

interface RelayResponse {
  kind: "response"
  request_id: string
  ok: boolean
  result?: unknown
  error?: unknown
}

interface RelayWsFrame {
  kind: "ws_frame"
  frame: unknown
}

interface RelayReady {
  kind: "ready"
}

type DesktopPayload = RelayResponse | RelayWsFrame | RelayReady

function relayId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID().replace(/-/g, "")}`
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) return false
  let difference = 0
  for (let index = 0; index < left.length; index++) {
    difference |= left[index] ^ right[index]
  }
  return difference === 0
}

export class RelayTransport implements Transport {
  private ws: WebSocket | null = null
  private destroyed = false
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private reconnectDelay = 1_000
  private keys: RelayDirectionalKeys | null = null
  private connectionId = ""
  private ephemeralKeyPair: CryptoKeyPair | null = null
  private sendSeq = 0
  private receivedSeq = 0
  private sessionPromise!: Promise<void>
  private sessionResolve!: () => void
  private readyPromise!: Promise<void>
  private readyResolve!: () => void
  private hasReadiedOnce = false
  private handlers = new Map<string, Set<(payload: unknown) => void>>()
  private pending = new Map<string, PendingCall>()
  private reconnectCallbacks = new Set<() => void>()
  private wsReadyCallbacks = new Set<() => void>()
  private connectionListeners = new Set<() => void>()
  private connState: WebConnState = "reconnecting"
  private eventStreamInstance: WebEventStream | null = null
  private readonly pairingRoot: Uint8Array

  constructor(private readonly config: MobileRelayConfig) {
    this.pairingRoot = relayBase64UrlDecode(config.pairingRoot)
    if (this.pairingRoot.length !== 32) {
      throw new Error("Relay pairing root must contain 32 bytes")
    }
    this.resetSession()
    this.resetReady()
    this.connect()
  }

  async call<T>(
    command: string,
    args: Record<string, unknown> = {},
    options?: CallOptions
  ): Promise<T> {
    const requestId = relayId("r")
    const idempotencyKey = relayId("i")
    const timeoutMs = options?.timeoutMs ?? DEFAULT_CALL_TIMEOUT_MS
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(requestId)
        reject(new Error("Request timed out"))
      }, timeoutMs)
      const pending: PendingCall = {
        requestId,
        command,
        args,
        idempotencyKey,
        timeoutMs,
        resolve: (value) => resolve(value as T),
        reject,
        timer,
      }
      this.pending.set(requestId, pending)
      void this.sendPending(pending).catch((error) => {
        if (this.destroyed) {
          this.finishPending(requestId, false, error)
        }
      })
    })
  }

  async subscribe<T>(
    event: string,
    handler: (payload: T) => void
  ): Promise<UnsubscribeFn> {
    const wrapped = handler as (payload: unknown) => void
    if (!this.handlers.has(event)) this.handlers.set(event, new Set())
    this.handlers.get(event)!.add(wrapped)
    await this.waitForReady()
    return () => this.handlers.get(event)?.delete(wrapped)
  }

  isDesktop(): boolean {
    return false
  }

  onReconnect(callback: () => void): UnsubscribeFn {
    this.reconnectCallbacks.add(callback)
    return () => this.reconnectCallbacks.delete(callback)
  }

  async waitForReady(): Promise<void> {
    let timer: ReturnType<typeof setTimeout> | undefined
    await Promise.race([
      this.readyPromise,
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, READY_TIMEOUT_MS)
      }),
    ])
    if (timer) clearTimeout(timer)
  }

  eventStream(): EventStream {
    if (!this.eventStreamInstance) {
      const host: AttachTransportHost = {
        isWsOpen: () => Boolean(this.keys),
        sendFrame: (frame) => {
          if (!this.keys) return false
          void this.sendEncrypted({ kind: "ws_frame", frame }).catch(() => {})
          return true
        },
        onWsReady: (callback) => {
          this.wsReadyCallbacks.add(callback)
          return () => this.wsReadyCallbacks.delete(callback)
        },
      }
      this.eventStreamInstance = new WebEventStream(host)
    }
    return this.eventStreamInstance
  }

  getConnectionSnapshot(): WebConnState {
    return this.connState
  }

  subscribeConnection(callback: () => void): UnsubscribeFn {
    this.connectionListeners.add(callback)
    return () => this.connectionListeners.delete(callback)
  }

  reconnectNow(): void {
    if (this.destroyed) return
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.reconnectDelay = 1_000
    this.ws?.close()
    this.ws = null
    this.resetSession()
    this.resetReady()
    this.setConnState("reconnecting")
    this.connect()
  }

  markUnauthorized(): void {
    if (this.destroyed) return
    this.setConnState("unauthorized")
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.ws?.close()
    this.ws = null
  }

  destroy(): void {
    this.destroyed = true
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.ws?.close()
    this.ws = null
    this.eventStreamInstance?.destroy()
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer)
      pending.reject(new Error("Relay transport was closed"))
    }
    this.pending.clear()
    this.handlers.clear()
    this.reconnectCallbacks.clear()
    this.wsReadyCallbacks.clear()
    this.connectionListeners.clear()
  }

  private resetSession(): void {
    this.keys = null
    this.connectionId = ""
    this.ephemeralKeyPair = null
    this.sendSeq = 0
    this.receivedSeq = 0
    this.sessionPromise = new Promise<void>((resolve) => {
      this.sessionResolve = resolve
    })
  }

  private resetReady(): void {
    this.readyPromise = new Promise<void>((resolve) => {
      this.readyResolve = resolve
    })
  }

  private connect(): void {
    if (this.destroyed || this.ws) return
    const socket = new WebSocket(this.config.relayUrl)
    this.ws = socket
    socket.onopen = () => {
      if (this.ws !== socket) return
      socket.send(
        JSON.stringify({
          v: RELAY_PROTOCOL_VERSION,
          type: "hello",
          role: "mobile",
          desktop_id: this.config.desktopId,
          device_id: this.config.deviceId,
          token: this.config.routingToken,
        })
      )
      void this.beginHandshake(socket).catch(() => socket.close())
    }
    socket.onmessage = (event) => {
      if (this.ws !== socket || typeof event.data !== "string") return
      void this.handleMessage(event.data).catch(() => socket.close())
    }
    socket.onerror = () => {}
    socket.onclose = () => {
      if (this.ws !== socket || this.destroyed) return
      this.ws = null
      this.resetSession()
      this.resetReady()
      if (this.connState !== "unauthorized") {
        this.setConnState("reconnecting")
        this.scheduleReconnect()
      }
    }
  }

  private async beginHandshake(socket: WebSocket): Promise<void> {
    const keyPair = await generateRelayEphemeralKeyPair()
    if (this.ws !== socket || socket.readyState !== WebSocket.OPEN) return
    const connectionId = relayId("c")
    const publicKey = relayBase64UrlEncode(
      await exportRelayPublicKey(keyPair.publicKey)
    )
    const proof = await createRelayHandshakeProof(
      this.pairingRoot,
      "mobile",
      this.config.desktopId,
      this.config.deviceId,
      connectionId,
      publicKey
    )
    this.ephemeralKeyPair = keyPair
    this.connectionId = connectionId
    const hello: PairEnvelope = {
      v: RELAY_PROTOCOL_VERSION,
      type: "pair",
      phase: "mobile_hello",
      desktop_id: this.config.desktopId,
      device_id: this.config.deviceId,
      connection_id: connectionId,
      public_key: publicKey,
      proof,
    }
    socket.send(JSON.stringify(hello))
  }

  private async handleMessage(text: string): Promise<void> {
    const message = JSON.parse(text) as Record<string, unknown>
    if (message.type === "error") {
      if (
        message.code === "unauthorized" ||
        message.code === "device_revoked"
      ) {
        this.markUnauthorized()
      }
      return
    }
    if (message.type === "pair") {
      await this.completeHandshake(message as unknown as PairEnvelope)
      return
    }
    if (isRelayFrameEnvelope(message)) {
      await this.openDesktopFrame(message)
    }
  }

  private async completeHandshake(message: PairEnvelope): Promise<void> {
    if (
      message.v !== RELAY_PROTOCOL_VERSION ||
      message.phase !== "desktop_hello" ||
      message.desktop_id !== this.config.desktopId ||
      message.device_id !== this.config.deviceId ||
      message.connection_id !== this.connectionId ||
      !this.ephemeralKeyPair
    ) {
      throw new Error("Relay desktop handshake metadata is invalid")
    }
    const valid = await verifyRelayHandshakeProof(
      this.pairingRoot,
      message.proof,
      "desktop",
      this.config.desktopId,
      this.config.deviceId,
      this.connectionId,
      message.public_key
    )
    if (!valid) throw new Error("Relay desktop handshake proof is invalid")
    const shared = await deriveRelaySharedSecret(
      this.ephemeralKeyPair.privateKey,
      relayBase64UrlDecode(message.public_key)
    )
    this.keys = await deriveRelayDirectionalKeys(
      shared,
      this.pairingRoot,
      this.connectionId
    )
    this.ephemeralKeyPair = null
    this.reconnectDelay = 1_000
    this.setConnState("connected")
    this.sessionResolve()
    for (const pending of this.pending.values()) {
      void this.sendPending(pending).catch(() => {})
    }
  }

  private async sendPending(pending: PendingCall): Promise<void> {
    await this.sessionPromise
    if (!this.pending.has(pending.requestId)) return
    await this.sendEncrypted({
      kind: "request",
      request_id: pending.requestId,
      command: pending.command,
      args: pending.args,
      idempotency_key: pending.idempotencyKey,
      timeout_ms: pending.timeoutMs,
    })
  }

  private async sendEncrypted(payload: unknown): Promise<void> {
    const keys = this.keys
    const socket = this.ws
    if (!keys || !socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("Relay encrypted session is not ready")
    }
    const seq = ++this.sendSeq
    const nonce = relayNonce(MOBILE_TO_DESKTOP_NONCE_TAG, BigInt(seq))
    const frame: RelayFrameEnvelope = {
      v: RELAY_PROTOCOL_VERSION,
      type: "frame",
      desktop_id: this.config.desktopId,
      device_id: this.config.deviceId,
      connection_id: this.connectionId,
      frame_id: relayId("f"),
      seq,
      ack: this.receivedSeq,
      nonce: relayBase64UrlEncode(nonce),
      ciphertext: "",
    }
    const plaintext = new TextEncoder().encode(JSON.stringify(payload))
    frame.ciphertext = relayBase64UrlEncode(
      await sealRelayFrame(
        keys.mobileToDesktop,
        nonce,
        relayFrameAad(frame),
        plaintext
      )
    )
    socket.send(JSON.stringify(frame))
  }

  private async openDesktopFrame(frame: RelayFrameEnvelope): Promise<void> {
    const keys = this.keys
    if (
      !keys ||
      frame.desktop_id !== this.config.desktopId ||
      frame.device_id !== this.config.deviceId ||
      frame.connection_id !== this.connectionId ||
      frame.seq !== this.receivedSeq + 1
    ) {
      throw new Error("Unexpected or replayed Relay frame")
    }
    const expectedNonce = relayNonce(
      DESKTOP_TO_MOBILE_NONCE_TAG,
      BigInt(frame.seq)
    )
    if (!sameBytes(expectedNonce, relayBase64UrlDecode(frame.nonce))) {
      throw new Error("Relay frame nonce does not match sequence")
    }
    const plaintext = await openRelayFrame(
      keys.desktopToMobile,
      expectedNonce,
      relayFrameAad(frame),
      relayBase64UrlDecode(frame.ciphertext)
    )
    this.receivedSeq = frame.seq
    const payload = JSON.parse(
      new TextDecoder().decode(plaintext)
    ) as DesktopPayload
    this.dispatchDesktopPayload(payload)
  }

  private dispatchDesktopPayload(payload: DesktopPayload): void {
    if (payload.kind === "response") {
      this.finishPending(
        payload.request_id,
        payload.ok,
        payload.ok ? payload.result : payload.error
      )
      return
    }
    if (payload.kind === "ready") {
      const reconnect = this.hasReadiedOnce
      this.hasReadiedOnce = true
      this.readyResolve()
      for (const callback of this.wsReadyCallbacks) callback()
      if (reconnect) {
        for (const callback of this.reconnectCallbacks) callback()
      }
      return
    }
    if (payload.kind === "ws_frame") {
      const frame = payload.frame as {
        channel?: unknown
        payload?: unknown
      }
      this.eventStreamInstance?.handleServerFrame(payload.frame)
      if (typeof frame.channel === "string") {
        for (const handler of this.handlers.get(frame.channel) ?? []) {
          try {
            handler(frame.payload)
          } catch (error) {
            console.error("[RelayTransport] event handler failed", error)
          }
        }
      }
    }
  }

  private finishPending(
    requestId: string,
    success: boolean,
    value: unknown
  ): void {
    const pending = this.pending.get(requestId)
    if (!pending) return
    this.pending.delete(requestId)
    clearTimeout(pending.timer)
    if (success) pending.resolve(value)
    else pending.reject(value)
  }

  private scheduleReconnect(): void {
    if (this.destroyed || this.reconnectTimer) return
    const delay = this.reconnectDelay
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS)
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, delay)
  }

  private setConnState(next: WebConnState): void {
    if (this.connState === next) return
    this.connState = next
    for (const callback of this.connectionListeners) callback()
  }
}
