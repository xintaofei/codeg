const encoder = new TextEncoder()

export function relayBase64UrlEncode(bytes: Uint8Array): string {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/g, "")
}

export function relayBase64UrlDecode(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(value)) {
    throw new Error("Invalid base64url value")
  }
  const padding = "=".repeat((4 - (value.length % 4)) % 4)
  const binary = atob(value.replace(/-/g, "+").replace(/_/g, "/") + padding)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

function relayHandshakeCanonical(
  role: "mobile" | "desktop",
  desktopId: string,
  deviceId: string,
  connectionId: string,
  publicKey: string
): Uint8Array {
  return encoder.encode(
    `codeg-relay-v1|${role}|${desktopId}|${deviceId}|${connectionId}|${publicKey}`
  )
}

async function importRelayHmacKey(pairingRoot: Uint8Array): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    pairingRoot,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"]
  )
}

export async function createRelayHandshakeProof(
  pairingRoot: Uint8Array,
  role: "mobile" | "desktop",
  desktopId: string,
  deviceId: string,
  connectionId: string,
  publicKey: string
): Promise<string> {
  const key = await importRelayHmacKey(pairingRoot)
  const signature = await crypto.subtle.sign(
    "HMAC",
    key,
    relayHandshakeCanonical(role, desktopId, deviceId, connectionId, publicKey)
  )
  return relayBase64UrlEncode(new Uint8Array(signature))
}

export async function verifyRelayHandshakeProof(
  pairingRoot: Uint8Array,
  proof: string,
  role: "mobile" | "desktop",
  desktopId: string,
  deviceId: string,
  connectionId: string,
  publicKey: string
): Promise<boolean> {
  const key = await importRelayHmacKey(pairingRoot)
  return crypto.subtle.verify(
    "HMAC",
    key,
    relayBase64UrlDecode(proof),
    relayHandshakeCanonical(role, desktopId, deviceId, connectionId, publicKey)
  )
}

export interface RelayDirectionalKeys {
  mobileToDesktop: CryptoKey
  desktopToMobile: CryptoKey
}

export async function generateRelayEphemeralKeyPair(): Promise<CryptoKeyPair> {
  return crypto.subtle.generateKey(
    { name: "ECDH", namedCurve: "P-256" },
    true,
    ["deriveBits"]
  )
}

export async function exportRelayPublicKey(
  publicKey: CryptoKey
): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.exportKey("raw", publicKey))
}

export async function deriveRelaySharedSecret(
  privateKey: CryptoKey,
  peerPublicKey: Uint8Array
): Promise<Uint8Array> {
  const peer = await crypto.subtle.importKey(
    "raw",
    peerPublicKey,
    { name: "ECDH", namedCurve: "P-256" },
    false,
    []
  )
  return new Uint8Array(
    await crypto.subtle.deriveBits(
      { name: "ECDH", public: peer },
      privateKey,
      256
    )
  )
}

export interface MobilePairingMaterial {
  pairingRoot: Uint8Array
  sas: string
  acceptKey: CryptoKey
}

export async function deriveMobilePairingMaterial(
  privateKey: CryptoKey,
  desktopPublicKey: Uint8Array,
  pairSecret: Uint8Array,
  desktopId: string,
  pairId: string,
  deviceId: string
): Promise<MobilePairingMaterial> {
  if (pairSecret.length !== 32) {
    throw new Error("Pairing secret must contain 32 bytes")
  }
  const sharedSecret = await deriveRelaySharedSecret(
    privateKey,
    desktopPublicKey
  )
  const context = `codeg-relay-pair-v2|${desktopId}|${pairId}|${deviceId}`
  const keyMaterial = await crypto.subtle.importKey(
    "raw",
    sharedSecret,
    "HKDF",
    false,
    ["deriveBits", "deriveKey"]
  )
  const pairingRoot = new Uint8Array(
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
    ["decrypt"]
  )
  const sasKey = await importRelayHmacKey(pairingRoot)
  const sasDigest = new Uint8Array(
    await crypto.subtle.sign("HMAC", sasKey, encoder.encode(`${context}|sas`))
  )
  const sasNumber =
    new DataView(sasDigest.buffer, sasDigest.byteOffset, 4).getUint32(
      0,
      false
    ) % 1_000_000
  return {
    pairingRoot,
    sas: sasNumber.toString().padStart(6, "0"),
    acceptKey,
  }
}

export async function openMobilePairingAccept(
  acceptKey: CryptoKey,
  nonce: Uint8Array,
  ciphertext: Uint8Array,
  desktopId: string,
  pairId: string,
  deviceId: string
): Promise<Uint8Array> {
  if (nonce.length !== 12)
    throw new Error("Pairing nonce must contain 12 bytes")
  const aad = encoder.encode(
    `codeg-relay-pair-v2|accept|${desktopId}|${pairId}|${deviceId}`
  )
  return new Uint8Array(
    await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
      acceptKey,
      ciphertext
    )
  )
}

async function deriveAesKey(
  sharedSecret: Uint8Array,
  pairingRoot: Uint8Array,
  connectionId: string,
  direction: string
): Promise<CryptoKey> {
  const material = await crypto.subtle.importKey(
    "raw",
    sharedSecret,
    "HKDF",
    false,
    ["deriveKey"]
  )
  return crypto.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: pairingRoot,
      info: encoder.encode(`codeg-relay-v1|${connectionId}|${direction}`),
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"]
  )
}

export async function deriveRelayDirectionalKeys(
  sharedSecret: Uint8Array,
  pairingRoot: Uint8Array,
  connectionId: string
): Promise<RelayDirectionalKeys> {
  const [mobileToDesktop, desktopToMobile] = await Promise.all([
    deriveAesKey(sharedSecret, pairingRoot, connectionId, "mobile-to-desktop"),
    deriveAesKey(sharedSecret, pairingRoot, connectionId, "desktop-to-mobile"),
  ])
  return { mobileToDesktop, desktopToMobile }
}

export function relayNonce(directionTag: number, sequence: bigint): Uint8Array {
  if (!Number.isSafeInteger(directionTag) || directionTag < 0)
    throw new RangeError("direction tag must be a non-negative integer")
  if (sequence <= 0n || sequence > 0xffff_ffff_ffff_ffffn)
    throw new RangeError("relay sequence is out of range")
  const nonce = new Uint8Array(12)
  const view = new DataView(nonce.buffer)
  view.setUint32(0, directionTag, false)
  view.setBigUint64(4, sequence, false)
  return nonce
}

export async function sealRelayFrame(
  key: CryptoKey,
  nonce: Uint8Array,
  aad: Uint8Array,
  plaintext: Uint8Array
): Promise<Uint8Array> {
  return new Uint8Array(
    await crypto.subtle.encrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
      key,
      plaintext
    )
  )
}

export async function openRelayFrame(
  key: CryptoKey,
  nonce: Uint8Array,
  aad: Uint8Array,
  ciphertext: Uint8Array
): Promise<Uint8Array> {
  return new Uint8Array(
    await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
      key,
      ciphertext
    )
  )
}
