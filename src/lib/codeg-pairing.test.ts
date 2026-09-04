import { describe, expect, it } from "vitest"
import { pairingMode, pairingQrValue } from "./codeg-pairing"

describe("codeg pairing QR", () => {
  it("encodes url, token, and private mode for iOS scan", () => {
    const raw = pairingQrValue({
      url: "https://codeg.tail123.ts.net",
      token: "secret-token",
      mode: "private",
    })
    expect(JSON.parse(raw)).toEqual({
      url: "https://codeg.tail123.ts.net",
      token: "secret-token",
      mode: "private",
      name: "Codeg",
    })
  })

  it("falls back to the bare URL when there is no token", () => {
    expect(
      pairingQrValue({
        url: "http://127.0.0.1:3080",
        token: "  ",
        mode: "local",
      })
    ).toBe("http://127.0.0.1:3080")
  })

  it("picks private over public", () => {
    expect(pairingMode({ serveEnabled: true, funnelEnabled: true })).toBe(
      "private"
    )
    expect(pairingMode({ funnelEnabled: true })).toBe("public")
    expect(pairingMode({})).toBe("local")
  })
})
