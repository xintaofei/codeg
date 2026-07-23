import { describe, expect, it } from "vitest"
import {
  rankByTextMatch,
  scoreTextMatch,
  subsequenceFirstIndex,
} from "@/lib/fuzzy-text-match"

describe("subsequenceFirstIndex", () => {
  it("matches contiguous and gapped subsequences", () => {
    expect(subsequenceFirstIndex("bmadhelp", "bmad-help")).toBe(0)
    expect(subsequenceFirstIndex("bmhp", "bmad-help")).toBe(0)
    expect(subsequenceFirstIndex("help", "bmad-help")).toBe(5)
  })

  it("returns -1 when a character is missing", () => {
    expect(subsequenceFirstIndex("bmadx", "bmad-help")).toBe(-1)
  })
})

describe("scoreTextMatch tiers", () => {
  it("orders exact > prefix > substring > subsequence", () => {
    const q = "help"
    const exact = scoreTextMatch(q, "help")!
    const prefix = scoreTextMatch(q, "help-me")!
    const substring = scoreTextMatch(q, "bmad-help")!
    const subseq = scoreTextMatch("bmhp", "bmad-help")!

    expect(exact).toBeGreaterThan(prefix)
    expect(prefix).toBeGreaterThan(substring)
    expect(scoreTextMatch("bmadh", "bmad-help")!).toBeLessThan(
      scoreTextMatch("bmad-", "bmad-help")!
    )
    // subsequence still scores positive
    expect(subseq).toBeGreaterThan(0)
  })

  it("returns null when nothing matches", () => {
    expect(scoreTextMatch("zzz", "bmad-help")).toBeNull()
  })
})

describe("rankByTextMatch", () => {
  const cmds = [
    { name: "bmad-help", description: "Show BMAD help" },
    { name: "bmad-create-story", description: "Create a story" },
    { name: "clear", description: "Clear the chat" },
  ]

  it("returns all items for an empty query", () => {
    expect(rankByTextMatch("", cmds, (c) => c.name)).toEqual(cmds)
  })

  it("matches hyphenated names via subsequence (bmadhelp / bmhp)", () => {
    const ranked = rankByTextMatch("bmadhelp", cmds, (c) => c.name)
    expect(ranked.map((c) => c.name)).toEqual(["bmad-help"])

    const short = rankByTextMatch("bmhp", cmds, (c) => c.name)
    expect(short.map((c) => c.name)).toContain("bmad-help")
  })

  it("ranks exact > prefix > subsequence (shorter wins ties)", () => {
    const items = [
      { name: "batch" }, // subsequence b..h
      { name: "bh-tool" }, // prefix
      { name: "bh" }, // exact
      { name: "bmad-help" }, // subsequence, longer than batch
    ]
    const ranked = rankByTextMatch("bh", items, (i) => i.name)
    expect(ranked.map((i) => i.name)).toEqual([
      "bh",
      "bh-tool",
      "batch",
      "bmad-help",
    ])
  })

  it("keeps any primary match above any secondary match", () => {
    const items = [
      { name: "zzz", tag: "bh" }, // secondary exact
      { name: "batch", tag: "zzz" }, // primary subsequence (b..h)
    ]
    // Weakest primary (subsequence) still outranks strongest secondary (exact).
    const ranked = rankByTextMatch(
      "bh",
      items,
      (i) => i.name,
      (i) => i.tag
    )
    expect(ranked.map((i) => i.name)).toEqual(["batch", "zzz"])
  })

  it("preserves input order for equally-scored matches (stable sort)", () => {
    const items = [
      { id: 1, name: "abc" },
      { id: 2, name: "abc" },
      { id: 3, name: "abc" },
    ]
    const ranked = rankByTextMatch("abc", items, (i) => i.name)
    expect(ranked.map((i) => i.id)).toEqual([1, 2, 3])
  })

  it("skips items with empty primary text without matching", () => {
    const items = [{ name: "" }, { name: "bmad-help" }]
    const ranked = rankByTextMatch("bmad", items, (i) => i.name)
    expect(ranked.map((i) => i.name)).toEqual(["bmad-help"])
  })

  it("falls back to secondary field below primary hits", () => {
    const items = [
      { name: "clear", description: "bmad helpers" },
      { name: "bmad-help", description: "other" },
    ]
    const ranked = rankByTextMatch(
      "bmad",
      items,
      (i) => i.name,
      (i) => i.description
    )
    // name prefix/substring on bmad-help beats description-only on clear
    expect(ranked[0].name).toBe("bmad-help")
    expect(ranked.map((i) => i.name)).toContain("clear")
  })

  it("is case-insensitive", () => {
    const ranked = rankByTextMatch("BMADHELP", cmds, (c) => c.name)
    expect(ranked.map((c) => c.name)).toEqual(["bmad-help"])
  })
})
