const encoder = new TextEncoder()

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
