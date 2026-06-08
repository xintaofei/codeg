import { describe, expect, it } from "vitest"

import { classifyResourceKind, type ResourceKind } from "./resource-kind"

describe("classifyResourceKind", () => {
  it.each<[string, ResourceKind]>([
    // file:// URIs
    ["file:///Users/a/b.ts", "file"],
    ["file:///Users/a/b.ts#L12", "file"],
    ["FILE:///Users/a/b.ts", "file"],
    ["file://server/share/doc.md", "file"],
    // Windows drive paths
    ["C:\\Users\\a\\notes.txt", "file"],
    ["C:/Users/a/notes.txt", "file"],
    ["d:\\repo\\src\\main.rs", "file"],
    // POSIX absolute / explicitly-relative paths
    ["/abs/path/file.ts", "file"],
    ["/abs/path/file.ts:42", "file"],
    ["./relative.md", "file"],
    ["../up/one.md", "file"],
    ["~/home/config.toml", "file"],
    // Protocol-relative URLs begin with "/", so link-safety's isLocalPathLike
    // routes them to the file opener — the file icon matches that behavior.
    ["//cdn.example.com/app.js", "file"],
    // Web
    ["http://example.com", "web"],
    ["https://example.com/docs?q=1#frag", "web"],
    ["HTTPS://EXAMPLE.COM", "web"],
    // Email / phone
    ["mailto:hi@example.com", "email"],
    ["MAILTO:hi@example.com", "email"],
    ["tel:+1-555-0100", "phone"],
  ])("classifies %s as %s", (input, expected) => {
    expect(classifyResourceKind(input)).toBe(expected)
  })

  it.each<[string]>([
    [""],
    ["   "],
    // streaming placeholder injected by streamdown for unclosed links
    ["streamdown:incomplete-link"],
    // in-page fragment
    ["#section"],
    // bare-relative targets the click handler can't resolve
    ["src/main.rs"],
    ["README.md"],
    ["www.example.com"],
    // "name.ext:line" parses as a (bogus) scheme, matching link-safety which
    // also declines to open it
    ["app.ts:12"],
    // unsupported / unknown schemes
    ["ftp://example.com/file"],
    ["vscode://file/repo/src/app.ts"],
    ["data:text/plain,hello"],
    ["javascript:alert(1)"],
  ])("returns null for %s", (input) => {
    expect(classifyResourceKind(input)).toBeNull()
  })

  it("trims surrounding whitespace before classifying", () => {
    expect(classifyResourceKind("  https://example.com  ")).toBe("web")
    expect(classifyResourceKind("\t/abs/file.ts\n")).toBe("file")
  })
})
