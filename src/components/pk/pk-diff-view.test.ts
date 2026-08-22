import { describe, expect, it } from "vitest"
import { parseUnifiedDiff } from "./pk-diff-view"

describe("parseUnifiedDiff", () => {
  it("classifies add, delete, hunk and context lines", () => {
    const diff = [
      "diff --git a/main.py b/main.py",
      "index 123..456 100644",
      "--- a/main.py",
      "+++ b/main.py",
      "@@ -1,3 +1,4 @@",
      " context line",
      "-removed line",
      "+added line",
      "+another addition",
    ].join("\n")

    const lines = parseUnifiedDiff(diff)
    // File header lines are dropped entirely.
    expect(lines.map((l) => l.kind)).toEqual([
      "hunk",
      "ctx",
      "del",
      "add",
      "add",
    ])
    expect(lines[0].text).toBe("@@ -1,3 +1,4 @@")
  })

  it("keeps blank context lines as renderable entries", () => {
    const lines = parseUnifiedDiff("+x\n\n-y")
    expect(lines).toHaveLength(3)
    expect(lines[1]).toEqual({ kind: "ctx", text: "" })
  })

  it("handles an empty diff without inventing lines", () => {
    expect(parseUnifiedDiff("")).toEqual([])
  })
})
