import { describe, expect, it } from "vitest"
import { splitSnippet } from "@/lib/message-search-snippet"

describe("splitSnippet", () => {
  it("separates the marked terms from the surrounding text", () => {
    expect(
      splitSnippet(
        "…why the [[mark]]retry[[/mark]] [[mark]]loop[[/mark]] races"
      )
    ).toEqual([
      { text: "…why the ", marked: false },
      { text: "retry", marked: true },
      { text: " ", marked: false },
      { text: "loop", marked: true },
      { text: " races", marked: false },
    ])
  })

  it("returns text without markers as one plain run", () => {
    expect(splitSnippet("nothing marked here")).toEqual([
      { text: "nothing marked here", marked: false },
    ])
  })

  it("leaves markup in the message text as literal text", () => {
    expect(splitSnippet("<b>[[mark]]<i>x</i>[[/mark]]</b>")).toEqual([
      { text: "<b>", marked: false },
      { text: "<i>x</i>", marked: true },
      { text: "</b>", marked: false },
    ])
  })

  it("returns no runs for an empty snippet", () => {
    expect(splitSnippet("")).toEqual([])
  })
})
