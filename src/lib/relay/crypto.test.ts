import { webcrypto } from "node:crypto"
import { beforeAll, describe, expect, it } from "vitest"

import {
  deriveRelayDirectionalKeys,
  deriveRelaySharedSecret,
  exportRelayPublicKey,
  generateRelayEphemeralKeyPair,
  openRelayFrame,
  relayNonce,
  sealRelayFrame,
} from "./crypto"

beforeAll(() => {
  Object.defineProperty(globalThis, "crypto", {
    configurable: true,
    value: webcrypto,
  })
})

describe("Codeg Relay v1 crypto", () => {
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
})
