import { useRef, useState, type ReactElement, type ReactNode } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import {
  FindInChatBar,
  NO_MEASURED_ROWS,
  estimateMatchCount,
  findMatchAt,
  findNeedle,
  findRangesInRow,
  findableSegments,
  foldForFind,
  matchOffsets,
  measuredCount,
  mergeMeasuredRows,
  resolveFindCursor,
  totalMatches,
  transcriptOwnsKeystroke,
  useFindHighlights,
  type FindRow,
  type MeasuredRows,
} from "./find-in-chat"
import type {
  ResolvedMessageGroup,
  ThreadRenderItem,
} from "./message-list-view"

type TurnItem = Extract<ThreadRenderItem, { kind: "turn" }>

const text = (value: string): AdaptedContentPart => ({
  type: "text",
  text: value,
})
const toolCall = {
  type: "tool-call",
  toolCallId: "t1",
  toolName: "bash",
  state: "output-available",
} as unknown as AdaptedContentPart
const reasoning: AdaptedContentPart = {
  type: "reasoning",
  content: "thinking hard",
  isStreaming: false,
}

function turnItem(
  parts: AdaptedContentPart[],
  {
    role = "assistant",
    key = "row",
    complete = true,
  }: { role?: "user" | "assistant"; key?: string; complete?: boolean } = {}
): TurnItem {
  return {
    key,
    kind: "turn",
    group: { id: key, role, parts, resources: [], images: [] },
    phase: "persisted",
    isResponseComplete: complete,
    showStats: false,
    isRoleTransition: false,
    previousUserIndex: null,
    isLastAssistantRun: false,
    isThreadTail: false,
    sourceTurns: [],
  }
}

function el(html: string): HTMLElement {
  const host = document.createElement("div")
  host.innerHTML = html
  document.body.appendChild(host)
  return host
}

afterEach(() => {
  document.body.innerHTML = ""
  vi.restoreAllMocks()
})

describe("foldForFind / findNeedle / matchOffsets", () => {
  it("folds case and whitespace without changing the length", () => {
    const source = "Hello\u00a0World\nX"
    const folded = foldForFind(source)
    expect(folded).toBe("hello world x")
    expect(folded).toHaveLength(source.length)
  })

  it("keeps a character whose lower case is longer", () => {
    // "İ".toLowerCase() is two code units; offsets must still line up.
    expect(foldForFind("İstanbul X")).toBe("İstanbul x")
  })

  it("has nothing to find for a blank query, and keeps inner spaces", () => {
    expect(findNeedle("   ")).toBe("")
    expect(findNeedle(" Foo Bar")).toBe(" foo bar")
  })

  it("finds non-overlapping occurrences in order", () => {
    expect(matchOffsets("aaaa", "aa")).toEqual([0, 2])
    expect(matchOffsets("abc", "")).toEqual([])
  })
})

describe("findableSegments / estimateMatchCount", () => {
  it("keeps one segment per text part and skips tools and reasoning", () => {
    const item = turnItem(
      [text("first paragraph"), toolCall, reasoning, text("second paragraph")],
      { complete: false }
    )
    expect(findableSegments(item, true)).toEqual([
      "first paragraph",
      "second paragraph",
    ])
  })

  it("reads a folded reply as only the answer it keeps in view", () => {
    const item = turnItem([text("let me look"), toolCall, text("the answer")])
    expect(findableSegments(item, false)).toEqual(["the answer"])
    expect(findableSegments(item, true)).toEqual(["let me look", "the answer"])
  })

  it("is empty for non-turn items and tool-only turns", () => {
    expect(findableSegments({ key: "typing", kind: "typing" }, true)).toEqual(
      []
    )
    expect(
      findableSegments({ key: "c", kind: "compaction", meta: null }, true)
    ).toEqual([])
    expect(findableSegments(turnItem([toolCall]), true)).toEqual([])
  })

  it("drops link destinations, which render as their label", () => {
    const item = turnItem([
      text("see [the docs](https://example.com/guide) ![chart](c.png)"),
    ])
    expect(findableSegments(item, true)).toEqual(["see the docs "])
    expect(estimateMatchCount(item, true, "example")).toBe(0)
    expect(estimateMatchCount(item, true, "docs")).toBe(1)
  })

  it("keeps user text verbatim", () => {
    const item = turnItem([text("see [x](https://a.b)")], { role: "user" })
    expect(findableSegments(item, false)).toEqual(["see [x](https://a.b)"])
  })
})

describe("findMatchAt / resolveFindCursor", () => {
  const rows: FindRow[] = [
    { key: "a", threadIndex: 1, count: 2 },
    { key: "b", threadIndex: 3, count: 1 },
    { key: "c", threadIndex: 6, count: 3 },
  ]
  const indexOf = (keys: Record<string, number>) => (key: string) => keys[key]

  it("maps a flat position to its row and occurrence", () => {
    expect(totalMatches(rows)).toBe(6)
    expect(findMatchAt(rows, 3)).toEqual({
      key: "c",
      occ: 1,
      threadIndex: 6,
      index: 3,
    })
    expect(findMatchAt(rows, 6)).toBeNull()
    expect(findMatchAt(rows, -1)).toBeNull()
  })

  it("starts at the first match", () => {
    expect(resolveFindCursor(rows, null, indexOf({}))?.index).toBe(0)
    expect(resolveFindCursor([], null, indexOf({}))).toBeNull()
  })

  it("stays on its match while counts elsewhere move", () => {
    const cursor = { key: "c", occ: 2, threadIndex: 6, dir: 1 as const }
    expect(resolveFindCursor(rows, cursor, indexOf({ c: 6 }))?.index).toBe(4)
    // Row "a" measures one match fewer once it mounts: same match, new number.
    const measured = [{ ...rows[0], count: 1 }, rows[1], rows[2]]
    expect(resolveFindCursor(measured, cursor, indexOf({ c: 6 }))).toEqual({
      key: "c",
      occ: 2,
      threadIndex: 6,
      index: 3,
    })
  })

  it("lands on the row's last match when the row now holds fewer", () => {
    const cursor = { key: "c", occ: 3, threadIndex: 6, dir: -1 as const }
    const measured = [rows[0], rows[1], { ...rows[2], count: 2 }]
    expect(
      resolveFindCursor(measured, cursor, indexOf({ c: 6 }))
    ).toMatchObject({ key: "c", occ: 2, index: 4 })
  })

  it("steps past a row that turns out to show no match", () => {
    // "x" sits at thread index 4 and, once measured, holds nothing.
    const at = indexOf({ x: 4 })
    const forward = { key: "x", occ: 1, threadIndex: 4, dir: 1 as const }
    const back = { ...forward, dir: -1 as const }
    expect(resolveFindCursor(rows, forward, at)).toMatchObject({
      key: "c",
      occ: 1,
    })
    expect(resolveFindCursor(rows, back, at)).toMatchObject({
      key: "b",
      occ: 1,
    })
  })

  it("wraps around at either end", () => {
    const last = { key: "z", occ: 1, threadIndex: 9, dir: 1 as const }
    expect(resolveFindCursor(rows, last, indexOf({ z: 9 }))?.key).toBe("a")
    const first = { key: "z", occ: 1, threadIndex: 0, dir: -1 as const }
    expect(resolveFindCursor(rows, first, indexOf({ z: 0 }))).toMatchObject({
      key: "c",
      occ: 3,
      index: 5,
    })
  })

  it("follows a row that was re-keyed in place", () => {
    // A reply settling renames its row; the one now at its index is it.
    const cursor = {
      key: "streaming-b",
      occ: 1,
      threadIndex: 3,
      dir: 1 as const,
    }
    expect(resolveFindCursor(rows, cursor, indexOf({}))).toMatchObject({
      key: "b",
      occ: 1,
    })
  })
})

describe("findRangesInRow", () => {
  const all = () => true

  it("matches only message prose, not the chrome around it", () => {
    const row = el(
      `<div data-find-text=""><p>see <a href="https://example.com">the docs</a></p></div>
       <div class="stats">docs example</div>`
    )
    expect(findRangesInRow(row, "docs", all)).toHaveLength(1)
    expect(findRangesInRow(row, "example", all)).toHaveLength(0)
  })

  it("matches across element boundaries inside one text part", () => {
    const row = el(`<div data-find-text="">fo<strong>O</strong>bar</div>`)
    const [range] = findRangesInRow(row, findNeedle("foob"), all)
    expect(range.toString()).toBe("foOb")
  })

  it("never matches across two text parts", () => {
    const row = el(
      `<div data-find-text="">foo</div><div data-find-text="">bar</div>`
    )
    expect(findRangesInRow(row, "foobar", all)).toHaveLength(0)
    expect(findRangesInRow(row, "bar", all)).toHaveLength(1)
  })

  it("skips text that is not prose on screen", () => {
    const row = el(
      `<div data-find-text=""><span class="katex-mathml">x</span><span class="katex-html">x</span><svg><text>x</text></svg></div>`
    )
    expect(findRangesInRow(row, "x", all)).toHaveLength(1)
  })

  it("counts only the ranges the visibility check accepts", () => {
    const row = el(`<div data-find-text="">docs and docs</div>`)
    let seen = 0
    const firstOnly = () => ++seen === 1
    expect(findRangesInRow(row, "docs", firstOnly)).toHaveLength(1)
  })
})

describe("mergeMeasuredRows / measuredCount", () => {
  const groupA = turnItem([text("a")], { key: "a" }).group
  const groupB = turnItem([text("b")], { key: "b" }).group
  type Measured = { group: ResolvedMessageGroup; count: number }
  const pass = (entries: [string, ResolvedMessageGroup, number][]) =>
    new Map(
      entries.map(([key, group, count]): [string, Measured] => [
        key,
        { group, count },
      ])
    )

  it("returns the same state when a pass changes nothing", () => {
    const first = mergeMeasuredRows(
      NO_MEASURED_ROWS,
      "q",
      0,
      pass([["a", groupA, 2]])
    )
    expect(first).not.toBe(NO_MEASURED_ROWS)
    expect(mergeMeasuredRows(first, "q", 0, pass([["a", groupA, 2]]))).toBe(
      first
    )
  })

  it("trusts a mounted row, and an unmounted one only while unchanged", () => {
    let measured: MeasuredRows = mergeMeasuredRows(
      NO_MEASURED_ROWS,
      "q",
      0,
      pass([["a", groupA, 2]])
    )
    const item = turnItem([text("a")], { key: "a" })
    const streamed = { ...item, group: { ...groupA } }
    // Still mounted: the next pass re-measures it, so its count holds even
    // though its content moved on.
    expect(measuredCount(measured, "q", 0, streamed)).toBe(2)

    // Scrolled out of the virtualizer's range.
    measured = mergeMeasuredRows(measured, "q", 0, pass([["b", groupB, 1]]))
    expect(measuredCount(measured, "q", 0, { ...item, group: groupA })).toBe(2)
    expect(measuredCount(measured, "q", 0, streamed)).toBeUndefined()
    expect(
      measuredCount(measured, "q", 1, { ...item, group: groupA })
    ).toBeUndefined()
    expect(
      measuredCount(measured, "other", 0, { ...item, group: groupA })
    ).toBeUndefined()
  })
})

describe("transcriptOwnsKeystroke", () => {
  it("belongs to the transcript the keyboard is in", () => {
    el(`
      <div id="shell-a">
        <div data-transcript="" id="frame-a"><button id="in-a"></button></div>
        <textarea id="composer-a"></textarea>
      </div>
      <div id="shell-b">
        <div data-transcript="" id="frame-b"></div>
        <textarea id="composer-b"></textarea>
      </div>
      <div data-terminal-panel-region="true"><textarea id="term"></textarea></div>
      <div role="dialog"><input id="palette" /></div>
      <div role="dialog">
        <div data-transcript="" id="frame-c"></div>
        <input id="in-c" />
      </div>
    `)
    const byId = (id: string) => document.getElementById(id)
    const frameA = byId("frame-a")

    expect(transcriptOwnsKeystroke(byId("in-a"), frameA)).toBe(true)
    expect(transcriptOwnsKeystroke(byId("composer-a"), frameA)).toBe(true)
    expect(transcriptOwnsKeystroke(document.body, frameA)).toBe(true)
    expect(transcriptOwnsKeystroke(null, frameA)).toBe(true)

    expect(transcriptOwnsKeystroke(byId("composer-b"), frameA)).toBe(false)
    expect(transcriptOwnsKeystroke(byId("term"), frameA)).toBe(false)
    expect(transcriptOwnsKeystroke(byId("palette"), frameA)).toBe(false)
    expect(transcriptOwnsKeystroke(byId("in-c"), byId("frame-c"))).toBe(true)
  })

  it("declines while the transcript is covered", () => {
    el(`<div inert=""><div data-transcript="" id="frame"></div></div>`)
    expect(
      transcriptOwnsKeystroke(document.body, document.getElementById("frame"))
    ).toBe(false)
  })
})

describe("useFindHighlights", () => {
  function Harness({
    items,
    needle,
    onMeasured,
    children,
  }: {
    items: ThreadRenderItem[]
    needle: string
    onMeasured: (measured: MeasuredRows) => void
    children: ReactNode
  }) {
    const frameRef = useRef<HTMLDivElement>(null)
    const [measured, setMeasured] = useState<MeasuredRows>(NO_MEASURED_ROWS)
    useFindHighlights(frameRef, {
      enabled: true,
      needle,
      epoch: 0,
      items,
      activeKey: null,
      activeOcc: 1,
      setMeasured,
    })
    onMeasured(measured)
    return (
      <div ref={frameRef} data-transcript="">
        {children}
      </div>
    )
  }

  it("measures the mounted rows' rendered prose as their count", () => {
    // jsdom has no layout: give every range a box so the visibility check
    // passes.
    vi.spyOn(Range.prototype, "getClientRects").mockReturnValue([
      { top: 0, left: 0, right: 10, bottom: 10, width: 10, height: 10 },
    ] as unknown as DOMRectList)
    const items = [
      turnItem([text("Hello world, [hello](https://hello.dev)")], {
        key: "a",
      }),
      turnItem([text("nothing here")], { key: "b" }),
      turnItem([text("hello again")], { key: "c" }),
    ]
    let latest: MeasuredRows = NO_MEASURED_ROWS
    render(
      <Harness
        items={items}
        needle="hello"
        onMeasured={(measured) => (latest = measured)}
      >
        <div data-find-key="a">
          <div data-find-text="">
            Hello world, <a href="https://hello.dev">hello</a>
          </div>
          <span>hello from the stats row</span>
        </div>
        <div data-find-key="b">
          <div data-find-text="">nothing here</div>
        </div>
      </Harness>
    )
    const [a, b, c] = items
    expect(measuredCount(latest, "hello", 0, a)).toBe(2)
    expect(measuredCount(latest, "hello", 0, b)).toBe(0)
    // Not mounted: left to its estimate.
    expect(measuredCount(latest, "hello", 0, c)).toBeUndefined()
    expect(estimateMatchCount(c, true, "hello")).toBe(1)
  })
})

describe("FindInChatBar", () => {
  function renderBar(ui: ReactElement) {
    return render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        {ui}
      </NextIntlClientProvider>
    )
  }

  it("shows the position and steps with Enter / Shift+Enter / Escape", () => {
    const onNext = vi.fn()
    const onPrev = vi.fn()
    const onClose = vi.fn()
    renderBar(
      <FindInChatBar
        query="docs"
        onQueryChange={() => {}}
        count={5}
        index={1}
        focusToken={1}
        onNext={onNext}
        onPrev={onPrev}
        onClose={onClose}
      />
    )
    expect(screen.getByText("2 of 5")).toBeInTheDocument()
    const input = screen.getByRole("textbox")
    expect(input).toHaveFocus()
    fireEvent.keyDown(input, { key: "Enter" })
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true })
    fireEvent.keyDown(input, { key: "Escape" })
    expect(onNext).toHaveBeenCalledTimes(1)
    expect(onPrev).toHaveBeenCalledTimes(1)
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it("says so when nothing matches, and disables stepping", () => {
    renderBar(
      <FindInChatBar
        query="zzz"
        onQueryChange={() => {}}
        count={0}
        index={0}
        focusToken={1}
        onNext={() => {}}
        onPrev={() => {}}
        onClose={() => {}}
      />
    )
    expect(screen.getByText("No matches")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Next match" })).toBeDisabled()
    expect(
      screen.getByRole("button", { name: "Previous match" })
    ).toBeDisabled()
  })
})
