import { afterEach, describe, expect, it } from "vitest"

import {
  clearMobileServer,
  getMobileServerUrl,
  normalizeServerUrl,
  setMobileServerUrl,
} from "./mobile-config"

describe("mobile server configuration", () => {
  afterEach(() => {
    localStorage.clear()
  })

  it("normalizes a host to HTTPS and strips trailing slashes", () => {
    expect(normalizeServerUrl(" codeg.example.com/// ")).toBe(
      "https://codeg.example.com"
    )
    expect(normalizeServerUrl("http://192.168.1.8:3080/")).toBe(
      "http://192.168.1.8:3080"
    )
  })

  it("stores and clears the selected mobile server with its token", () => {
    localStorage.setItem("codeg_token", "secret")
    expect(setMobileServerUrl("https://codeg.example.com/")).toBe(
      "https://codeg.example.com"
    )
    expect(getMobileServerUrl()).toBe("https://codeg.example.com")

    clearMobileServer()
    expect(getMobileServerUrl()).toBe("")
    expect(localStorage.getItem("codeg_token")).toBeNull()
  })
})
