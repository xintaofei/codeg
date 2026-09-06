import { describe, expect, it } from "vitest"
import { splitStableUnits } from "@/lib/translation"
import { maskForTranslation } from "@/components/ai-elements/markdown-mask"

describe("probe", () => {
  it("display math spanning blank lines", () => {
    const t1 = "prose before\n\n$$\nx = 1\n\ny = 2\n$$\n\nprose after"
    const r1 = splitStableUnits(t1)
    console.log("units:", JSON.stringify(r1.units))
    for (const u of r1.units) {
      const m = maskForTranslation(u)
      console.log("  masked:", JSON.stringify(m.masked))
    }
    expect(true).toBe(true)
  })
  it("indented fence with blank line", () => {
    const t2 = "- item\n\n  ```js\n  const a = 1\n\n  const b = 2\n  ```\n\nafter"
    const r2 = splitStableUnits(t2)
    console.log("units2:", JSON.stringify(r2.units))
    for (const u of r2.units) {
      const m = maskForTranslation(u)
      console.log("  masked:", JSON.stringify(m.masked))
    }
    expect(true).toBe(true)
  })
  it("surrogate split check in splitForTranslation paragraph path", () => {
    // paragraph boundary right before a surrogate pair: end lands between them
    const emoji = "\u{1F600}"
    const source = "a".repeat(2498) + "\n\n" + emoji + emoji + "b".repeat(3000)
    const chunks = splitStableUnitsForProbe(source)
    console.log("chunks lens:", chunks?.map((c) => c.length))
    expect(true).toBe(true)
  })
})

function splitStableUnitsForProbe(text: string) {
  // re-import splitForTranslation
  return splitForTranslationProbe(text)
}
import { splitForTranslation as splitForTranslationProbe } from "@/lib/translation"
