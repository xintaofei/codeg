import { describe, expect, it } from "vitest"

import { normalizeTypedAddress } from "./browser-toolbar"

describe("normalizeTypedAddress", () => {
  it("keeps full http(s) URLs and normalizes them", () => {
    expect(normalizeTypedAddress("  https://Example.com/a b ")).toBe(
      "https://example.com/a%20b"
    )
    expect(normalizeTypedAddress("http://localhost:3000")).toBe(
      "http://localhost:3000/"
    )
  })

  it("adds a scheme to bare hosts: http for local, https otherwise", () => {
    expect(normalizeTypedAddress("localhost:3000/app")).toBe(
      "http://localhost:3000/app"
    )
    expect(normalizeTypedAddress("127.0.0.1:8080")).toBe(
      "http://127.0.0.1:8080/"
    )
    expect(normalizeTypedAddress("192.168.1.5")).toBe("http://192.168.1.5/")
    expect(normalizeTypedAddress("example.com/docs?x=1")).toBe(
      "https://example.com/docs?x=1"
    )
  })

  it("refuses other schemes, words and blanks (no search fallback)", () => {
    expect(normalizeTypedAddress("javascript:alert(1)")).toBeNull()
    expect(normalizeTypedAddress("file:///etc/hosts")).toBeNull()
    expect(normalizeTypedAddress("hello world")).toBeNull()
    expect(normalizeTypedAddress("notes")).toBeNull()
    expect(normalizeTypedAddress("")).toBeNull()
  })
})
