import { webcrypto } from "node:crypto"
import { beforeAll, describe, expect, it } from "vitest"

import {
  createRelayHandshakeProof,
  deriveMobilePairingMaterial,
  deriveRelayDirectionalKeys,
  deriveRelaySharedSecret,
  exportRelayPublicKey,
  generateRelayEphemeralKeyPair,
  openMobilePairingAccept,
  openRelayFrame,
  relayBase64UrlDecode,
  relayBase64UrlEncode,
  relayNonce,
  sealRelayFrame,
  verifyRelayHandshakeProof,
} from "./crypto"

beforeAll(() => {
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: webcrypto,
  })
})

describe("Codeg Relay v1 crypto", () => {
  it("round-trips unpadded base64url", () => {
    const bytes = Uint8Array.from([0, 1, 2, 250, 251, 252, 253, 254, 255])
    const encoded = relayBase64UrlEncode(bytes)
    expect(encoded).not.toMatch(/[+/=]/)
    expect(relayBase64UrlDecode(encoded)).toEqual(bytes)
  })

  it("authenticates ephemeral handshake metadata", async () => {
    const root = crypto.getRandomValues(new Uint8Array(32))
    const proof = await createRelayHandshakeProof(
      root,
      "mobile",
      "d_test",
      "m_phone",
      "c_test",
      "public-key"
    )
    await expect(
      verifyRelayHandshakeProof(
        root,
        proof,
        "mobile",
        "d_test",
        "m_phone",
        "c_test",
        "public-key"
      )
    ).resolves.toBe(true)
    await expect(
      verifyRelayHandshakeProof(
        root,
        proof,
        "mobile",
        "d_test",
        "m_other",
        "c_test",
        "public-key"
      )
    ).resolves.toBe(false)
  })

  it("derives matching directional keys and decrypts an authenticated frame", async () => {
    const mobile = await generateRelayEphemeralKeyPair()
    const desktop = await generateRelayEphemeralKeyPair()
    const [mobilePublic, desktopPublic] = await Promise.all([
      exportRelayPublicKey(mobile.publicKey),
      exportRelayPublicKey(desktop.publicKey),
    ])
    const [mobileSecret, desktopSecret] = await Promise.all([
      deriveRelaySharedSecret(mobile.privateKey, desktopPublic),
      deriveRelaySharedSecret(desktop.privateKey, mobilePublic),
    ])
    expect(mobileSecret).toEqual(desktopSecret)

    const pairingRoot = new Uint8Array(32).fill(0x5a)
    const [mobileKeys, desktopKeys] = await Promise.all([
      deriveRelayDirectionalKeys(mobileSecret, pairingRoot, "connection-1"),
      deriveRelayDirectionalKeys(desktopSecret, pairingRoot, "connection-1"),
    ])
    const nonce = relayNonce(0x4d3244, 1n)
    const aad = new TextEncoder().encode("authenticated-routing-header")
    const plaintext = new TextEncoder().encode("private Codeg payload")
    const ciphertext = await sealRelayFrame(
      mobileKeys.mobileToDesktop,
      nonce,
      aad,
      plaintext
    )
    const opened = await openRelayFrame(
      desktopKeys.mobileToDesktop,
      nonce,
      aad,
      ciphertext
    )
    expect(Array.from(opened)).toEqual(Array.from(plaintext))
  })

  it("rejects modified routing metadata", async () => {
    const mobile = await generateRelayEphemeralKeyPair()
    const desktop = await generateRelayEphemeralKeyPair()
    const secret = await deriveRelaySharedSecret(
      mobile.privateKey,
      await exportRelayPublicKey(desktop.publicKey)
    )
    const keys = await deriveRelayDirectionalKeys(
      secret,
      new Uint8Array(32).fill(7),
      "connection-2"
    )
    const nonce = relayNonce(1, 9n)
    const ciphertext = await sealRelayFrame(
      keys.mobileToDesktop,
      nonce,
      new TextEncoder().encode("device-a"),
      new Uint8Array([1, 2, 3])
    )
    await expect(
      openRelayFrame(
        keys.mobileToDesktop,
        nonce,
        new TextEncoder().encode("device-b"),
        ciphertext
      )
    ).rejects.toBeDefined()
  })

  it("constructs unique deterministic nonces from direction and sequence", () => {
    expect(relayNonce(1, 1n)).not.toEqual(relayNonce(1, 2n))
    expect(relayNonce(1, 1n)).not.toEqual(relayNonce(2, 1n))
    expect(() => relayNonce(1, 0n)).toThrow(/sequence/)
  })

  it("derives and opens a device-bound one-time pairing acceptance", async () => {
    const desktop = await generateRelayEphemeralKeyPair()
    const mobile = await generateRelayEphemeralKeyPair()
    const pairSecret = crypto.getRandomValues(new Uint8Array(32))
    const desktopId = "d_desktop"
    const pairId = "p_one_time"
    const deviceId = "m_phone"
    const context = `codeg-relay-pair-v2|${desktopId}|${pairId}|${deviceId}`

    const [desktopPublic, mobilePublic] = await Promise.all([
      exportRelayPublicKey(desktop.publicKey),
      exportRelayPublicKey(mobile.publicKey),
    ])
    const mobileMaterial = await deriveMobilePairingMaterial(
      mobile.privateKey,
      desktopPublic,
      pairSecret,
      desktopId,
      pairId,
      deviceId
    )
    expect(mobileMaterial.pairingRoot).toHaveLength(32)
    expect(mobileMaterial.sas).toMatch(/^\d{6}$/)

    const desktopSharedSecret = await deriveRelaySharedSecret(
      desktop.privateKey,
      mobilePublic
    )
    const desktopKeyMaterial = await crypto.subtle.importKey(
      "raw",
      desktopSharedSecret,
      "HKDF",
      false,
      ["deriveKey"]
    )
    const desktopAcceptKey = await crypto.subtle.deriveKey(
      {
        name: "HKDF",
        hash: "SHA-256",
        salt: pairSecret,
        info: new TextEncoder().encode(`${context}|accept-key`),
      },
      desktopKeyMaterial,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt"]
    )
    const nonce = crypto.getRandomValues(new Uint8Array(12))
    const aad = new TextEncoder().encode(
      `codeg-relay-pair-v2|accept|${desktopId}|${pairId}|${deviceId}`
    )
    const plaintext = new TextEncoder().encode(
      JSON.stringify({ v: 2, routing_token: "secret-routing-token" })
    )
    const ciphertext = new Uint8Array(
      await crypto.subtle.encrypt(
        { name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: 128 },
        desktopAcceptKey,
        plaintext
      )
    )

    const opened = await openMobilePairingAccept(
      mobileMaterial.acceptKey,
      nonce,
      ciphertext,
      desktopId,
      pairId,
      deviceId
    )
    expect(Array.from(opened)).toEqual(Array.from(plaintext))
    await expect(
      openMobilePairingAccept(
        mobileMaterial.acceptKey,
        nonce,
        ciphertext,
        desktopId,
        pairId,
        "m_attacker"
      )
    ).rejects.toBeDefined()
  })
})
