import { describe, expect, it } from "vitest"
import {
  FunnelError,
  displayAddresses,
  funnelDisableArgs,
  funnelEnableArgs,
  funnelTarget,
  isLoopbackTarget,
  serveDisableArgs,
  serveEnableArgs,
} from "./tailscale-funnel"

describe("tailscale serve and funnel commands", () => {
  it("only targets loopback HTTP", () => {
    expect(funnelTarget(3080)).toBe("http://127.0.0.1:3080")
    expect(serveEnableArgs(3080)).toEqual([
      "serve",
      "--bg",
      "--yes",
      "http://127.0.0.1:3080",
    ])
    expect(serveDisableArgs()).toEqual(["serve", "reset"])
    expect(funnelEnableArgs(3080)).toEqual([
      "funnel",
      "--bg",
      "--yes",
      "http://127.0.0.1:3080",
    ])
    expect(funnelDisableArgs()).toEqual(["funnel", "reset"])
    expect(isLoopbackTarget("http://127.0.0.1:3080")).toBe(true)
    expect(isLoopbackTarget("http://0.0.0.0:3080")).toBe(false)
    expect(isLoopbackTarget("http://192.168.1.5:3080")).toBe(false)
  })

  it("rejects a bad port", () => {
    expect(() => funnelTarget(0)).toThrow(FunnelError)
    expect(() => funnelTarget(70_000)).toThrow(FunnelError)
  })

  it("puts the public HTTPS URL first", () => {
    expect(
      displayAddresses(
        ["http://127.0.0.1:3080"],
        "https://codeg.tail123.ts.net"
      )
    ).toEqual(["https://codeg.tail123.ts.net", "http://127.0.0.1:3080"])
    expect(displayAddresses(["http://127.0.0.1:3080"], null)).toEqual([
      "http://127.0.0.1:3080",
    ])
  })
})
