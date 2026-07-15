export const RELAY_PROTOCOL_VERSION = 1 as const
export const RELAY_MAX_FRAME_BYTES = 1024 * 1024
export const RELAY_UPLOAD_CHUNK_BYTES = 256 * 1024
export const RELAY_REPLAY_WINDOW = 4096
export const RELAY_MAX_UNACKNOWLEDGED_FRAMES = 256

export type RelayEnvelopeType =
  | "hello"
  | "pair"
  | "frame"
  | "ack"
  | "ping"
  | "pong"
  | "revoke"
  | "error"

export interface RelayFrameEnvelope {
  v: typeof RELAY_PROTOCOL_VERSION
  type: "frame"
  desktop_id: string
  device_id: string
  connection_id: string
  frame_id: string
  seq: number
  ack: number
  nonce: string
  ciphertext: string
}

function validId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length >= 3 &&
    value.length <= 128 &&
    /^[A-Za-z0-9._:-]+$/.test(value)
  )
}

export function isRelayFrameEnvelope(
  value: unknown
): value is RelayFrameEnvelope {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false
  const frame = value as Partial<RelayFrameEnvelope>
  return (
    frame.v === RELAY_PROTOCOL_VERSION &&
    frame.type === "frame" &&
    validId(frame.desktop_id) &&
    validId(frame.device_id) &&
    validId(frame.connection_id) &&
    validId(frame.frame_id) &&
    Number.isSafeInteger(frame.seq) &&
    Number(frame.seq) > 0 &&
    Number.isSafeInteger(frame.ack) &&
    Number(frame.ack) >= 0 &&
    typeof frame.nonce === "string" &&
    frame.nonce.length === 16 &&
    typeof frame.ciphertext === "string" &&
    frame.ciphertext.length > 0 &&
    frame.ciphertext.length <= Math.ceil((RELAY_MAX_FRAME_BYTES * 4) / 3) + 4
  )
}

export function relayFrameAad(frame: RelayFrameEnvelope): Uint8Array {
  return new TextEncoder().encode(
    [
      frame.v,
      frame.desktop_id,
      frame.device_id,
      frame.connection_id,
      frame.frame_id,
      frame.seq,
      frame.ack,
    ].join("|")
  )
}
