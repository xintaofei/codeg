import { describe, expect, it } from "vitest"

import {
  isHostRule,
  matchHostRule,
  normalizeHostRulePattern,
  parseHostRulePattern,
  validateHostRulePattern,
  type HostRule,
} from "./host-rules"

// Mirror of the table in src-tauri/src/browser/policy.rs: the two sides must
// accept the same patterns and pick the same rule.
describe("host rule patterns", () => {
  it("accepts hostnames, wildcards, ports and IPv6 literals", () => {
    for (const ok of [
      "example.com",
      "EXAMPLE.com",
      " example.com ",
      "*.example.com",
      "*",
      "localhost:3000",
      "*.corp.example:8443",
      "[::1]:3000",
      "[::1]",
      "127.0.0.1",
      "10.0.0.1:8080",
      "*:443",
    ]) {
      expect(validateHostRulePattern(ok), ok).toBeNull()
    }
  })

  it("refuses URLs, paths, bad ports and malformed hosts", () => {
    expect(validateHostRulePattern("")).toBe("empty")
    expect(validateHostRulePattern("   ")).toBe("empty")
    for (const bad of [
      "https://example.com",
      "example.com/path",
      "example.com:",
      "example.com:0",
      "example.com:70000",
      "example.com:80a",
      "*.",
      "*example.com",
      "a b.com",
      ".example.com",
      "example..com",
      "[::1",
      "[::1]x",
      "*.*",
    ]) {
      expect(validateHostRulePattern(bad), bad).toBe("invalid")
      expect(parseHostRulePattern(bad), bad).toBeNull()
    }
  })

  it("normalizes to trimmed lower case", () => {
    expect(normalizeHostRulePattern("  Example.COM:8080 ")).toBe(
      "example.com:8080"
    )
  })

  it("recognizes stored rules and rejects junk", () => {
    expect(isHostRule({ pattern: "a.example", action: "block" })).toBe(true)
    expect(isHostRule({ pattern: "a.example", action: "explode" })).toBe(false)
    expect(isHostRule({ pattern: "", action: "block" })).toBe(false)
    expect(isHostRule("a.example")).toBe(false)
    expect(isHostRule(null)).toBe(false)
  })
})

describe("matchHostRule", () => {
  const block: HostRule[] = [{ pattern: "*.example.com", action: "block" }]

  it("wildcard semantics", () => {
    expect(
      matchHostRule(block, new URL("https://a.example.com/"))
    ).not.toBeNull()
    expect(
      matchHostRule(block, new URL("https://a.b.example.com/"))
    ).not.toBeNull()
    expect(matchHostRule(block, new URL("https://example.com/"))).toBeNull()
    expect(matchHostRule(block, new URL("https://notexample.com/"))).toBeNull()
    expect(
      matchHostRule([{ pattern: "*", action: "system" }], new URL("http://x/"))
    ).not.toBeNull()
    expect(
      matchHostRule(
        [{ pattern: "[::1]:3000", action: "builtin" }],
        new URL("http://[::1]:3000/")
      )
    ).not.toBeNull()
    expect(
      matchHostRule(
        [{ pattern: "[::1]:3000", action: "builtin" }],
        new URL("http://[::1]:3001/")
      )
    ).toBeNull()
  })

  it("ports pin a rule and compare against the scheme's default", () => {
    expect(
      matchHostRule(
        [{ pattern: "example.com:443", action: "builtin" }],
        new URL("https://EXAMPLE.com/")
      )
    ).not.toBeNull()
    expect(
      matchHostRule(
        [{ pattern: "example.com:80", action: "builtin" }],
        new URL("https://example.com/")
      )
    ).toBeNull()
    expect(
      matchHostRule(
        [{ pattern: "example.com:80", action: "builtin" }],
        new URL("http://example.com/")
      )
    ).not.toBeNull()
  })

  it("never matches an unparsable pattern", () => {
    expect(
      matchHostRule(
        [{ pattern: "https://example.com", action: "block" }],
        new URL("https://example.com/")
      )
    ).toBeNull()
    expect(matchHostRule([], new URL("https://example.com/"))).toBeNull()
    expect(matchHostRule(undefined, new URL("https://example.com/"))).toBeNull()
  })

  it("picks the most specific rule regardless of order, first listed on a tie", () => {
    const rules: HostRule[] = [
      { pattern: "*", action: "system" },
      { pattern: "*.corp.example", action: "builtin" },
      { pattern: "*.corp.example:8443", action: "system" },
      { pattern: "sso.corp.example", action: "block" },
      { pattern: "*.sso.corp.example", action: "builtin" },
    ]
    const action = (url: string) =>
      matchHostRule(rules, new URL(url))?.action ?? null
    expect(action("https://sso.corp.example/")).toBe("block")
    expect(action("https://sso.corp.example:8443/")).toBe("block")
    expect(action("https://wiki.corp.example:8443/")).toBe("system")
    expect(action("https://wiki.corp.example/")).toBe("builtin")
    expect(action("https://a.sso.corp.example/")).toBe("builtin")
    expect(action("https://elsewhere.example/")).toBe("system")
    expect(
      matchHostRule(
        [
          { pattern: "dup.example", action: "builtin" },
          { pattern: "dup.example", action: "block" },
        ],
        new URL("https://dup.example/")
      )?.action
    ).toBe("builtin")
  })
})
