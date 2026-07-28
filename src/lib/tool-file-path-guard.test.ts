import { describe, expect, it } from "vitest"
import { isFilePathToolName, pathFromToolTitle } from "./tool-file-path-guard"

describe("isFilePathToolName", () => {
  it("accepts read/edit/write family names", () => {
    expect(isFilePathToolName("read")).toBe(true)
    expect(isFilePathToolName("Read File")).toBe(true)
    expect(isFilePathToolName("edit")).toBe(true)
    expect(isFilePathToolName("apply_patch")).toBe(true)
    expect(isFilePathToolName("write")).toBe(true)
  })

  it("rejects non-file tools", () => {
    expect(isFilePathToolName("webfetch")).toBe(false)
    expect(isFilePathToolName("WebFetch")).toBe(false)
    expect(isFilePathToolName("glob")).toBe(false)
    expect(isFilePathToolName("grep")).toBe(false)
    expect(isFilePathToolName("todowrite")).toBe(false)
    expect(isFilePathToolName("bash")).toBe(false)
  })
})

describe("pathFromToolTitle", () => {
  it("extracts path after Read/Edit prefixes", () => {
    expect(pathFromToolTitle("Read AGENTS.md")).toBe("AGENTS.md")
    expect(pathFromToolTitle("Edit src/a.ts")).toBe("src/a.ts")
    expect(pathFromToolTitle("读取 nested/foo.ts")).toBe("nested/foo.ts")
  })

  it("rejects WebFetch / Glob / multi-file / URL / glob-like titles", () => {
    expect(pathFromToolTitle("WebFetch https://example.com/a")).toBeNull()
    expect(pathFromToolTitle("Glob foo/bar.ts")).toBeNull()
    expect(pathFromToolTitle("Grep foo/bar")).toBeNull()
    expect(pathFromToolTitle("Edit (3 files)")).toBeNull()
    expect(pathFromToolTitle("Todos (2/5)")).toBeNull()
    expect(pathFromToolTitle("Read https://x.test/a")).toBeNull()
    expect(pathFromToolTitle("Read foo/*.md")).toBeNull()
  })
})
