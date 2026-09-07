import { describe, expect, it } from "vitest"

import {
  displayHostPort,
  hostnameOf,
  isLoopbackHost,
  isLoopbackOrPrivateUrl,
  isPrivateNetworkHost,
  normalizeUrlForDedupe,
  originOf,
  portOf,
} from "./browser-url"

describe("isLoopbackHost", () => {
  it.each([
    "localhost",
    "LOCALHOST",
    "app.localhost",
    "127.0.0.1",
    "127.1.2.3",
    "::1",
    "[::1]",
    "::",
    "0.0.0.0",
    "::ffff:127.0.0.1",
  ])("%s is loopback", (host) => {
    expect(isLoopbackHost(host)).toBe(true)
  })

  it.each([
    "example.com",
    "localhost.example.com",
    "128.0.0.1",
    "10.0.0.1",
    "notlocalhost",
    "",
  ])("%s is not loopback", (host) => {
    expect(isLoopbackHost(host)).toBe(false)
  })
})

describe("isPrivateNetworkHost", () => {
  it.each([
    "10.0.0.1",
    "10.255.255.255",
    "172.16.0.1",
    "172.31.255.1",
    "192.168.1.20",
    "169.254.169.254",
    "fd12:3456::1",
    "fc00::1",
    "fe80::1%en0",
    "[fe80::1]",
    "mymac.local",
    "::ffff:192.168.0.1",
  ])("%s is private / link-local", (host) => {
    expect(isPrivateNetworkHost(host)).toBe(true)
  })

  it.each([
    "172.15.0.1",
    "172.32.0.1",
    "11.0.0.1",
    "192.169.0.1",
    "8.8.8.8",
    "2001:db8::1",
    "example.com",
    "127.0.0.1",
    "localhost",
  ])("%s is not private", (host) => {
    expect(isPrivateNetworkHost(host)).toBe(false)
  })
})

describe("URL helpers", () => {
  it("hostnameOf strips IPv6 brackets and lower-cases", () => {
    expect(hostnameOf("http://[::1]:3000/x")).toBe("::1")
    expect(hostnameOf("HTTPS://Example.COM/")).toBe("example.com")
    expect(hostnameOf("not a url")).toBeNull()
  })

  it("isLoopbackOrPrivateUrl looks at the host only", () => {
    expect(isLoopbackOrPrivateUrl("http://localhost:3000/api")).toBe(true)
    expect(isLoopbackOrPrivateUrl("http://192.168.0.5:8080/")).toBe(true)
    expect(isLoopbackOrPrivateUrl("https://github.com/x")).toBe(false)
    expect(isLoopbackOrPrivateUrl("garbage")).toBe(false)
  })

  it("originOf returns null for opaque origins", () => {
    expect(originOf("https://example.com:8443/a/b")).toBe(
      "https://example.com:8443"
    )
    expect(originOf("about:blank")).toBeNull()
    expect(originOf("blob:null/abc")).toBeNull()
  })

  it("portOf resolves defaults per scheme", () => {
    expect(portOf("http://example.com/")).toBe(80)
    expect(portOf("https://example.com/")).toBe(443)
    expect(portOf("http://example.com:3000/")).toBe(3000)
    expect(portOf("mailto:x@y")).toBeNull()
  })

  it("normalizeUrlForDedupe drops the fragment and serializes", () => {
    expect(normalizeUrlForDedupe("HTTP://Example.com/a#frag")).toBe(
      "http://example.com/a"
    )
    expect(normalizeUrlForDedupe("http://example.com")).toBe(
      "http://example.com/"
    )
    expect(normalizeUrlForDedupe("nope")).toBeNull()
  })

  it("displayHostPort keeps an explicit port and omits the default", () => {
    expect(displayHostPort("http://localhost:3000/app")).toBe("localhost:3000")
    expect(displayHostPort("https://example.com/")).toBe("example.com")
  })
})
