import { describe, expect, it } from "vitest"

import { extractEmbeddedJsonObject } from "./embedded-json"

describe("extractEmbeddedJsonObject", () => {
  it("returns null when there is no JSON object", () => {
    expect(extractEmbeddedJsonObject("just text, no braces")).toBeNull()
    expect(extractEmbeddedJsonObject("")).toBeNull()
  })

  it("parses a bare JSON object", () => {
    expect(extractEmbeddedJsonObject('{"a":1}')).toEqual({ a: 1 })
  })

  it("recovers a JSON object behind a textual prefix (Codex 'Wall time')", () => {
    const wrapped = 'Wall time: 2 seconds\nOutput:\n{"status":"completed"}'
    expect(extractEmbeddedJsonObject(wrapped)).toEqual({ status: "completed" })
  })

  it("tolerates a trailing terminal-cursor character after the object", () => {
    const wrapped = 'Output:\n{"status":"running","id":7}_'
    expect(extractEmbeddedJsonObject(wrapped)).toEqual({
      status: "running",
      id: 7,
    })
  })

  it("ignores a top-level array (only objects are recovered)", () => {
    expect(extractEmbeddedJsonObject("[1,2,3]")).toBeNull()
  })
})
