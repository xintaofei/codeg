import { describe, expect, it } from "vitest"

import {
  MAX_PARSE_BYTES,
  MAX_TRANSLATION_CHARS,
  STREAM_TAIL_CHUNK_MAX_CHARS,
  buildContextPrefix,
  buildNumberedRequest,
  buildTranslateBody,
  echoVerbatimError,
  HALF_SPLIT_MIN_CHARS,
  hasSameTranslationPlaceholders,
  joinTranslated,
  mergeUnit,
  mergeUnitGroups,
  missingSourceNumbers,
  missingTargetScript,
  parseNumberedTranslation,
  realignTranslationPlaceholders,
  sentenceChunkEnd,
  shouldTranslate,
  retryConstraintLine,
  splitChunkForHalfRetry,
  splitForTranslation,
  stripTranslateEnvelope,
  splitStableUnits,
  tailChunksFor,
} from "./translation"

describe("translate envelope", () => {
  it("wraps the body as DATA and unwraps a compliant reply", () => {
    const body = buildTranslateBody("[1] Hello world", "zh-CN")
    expect(body).toBe(
      '<translate target="zh-CN">\n[1] Hello world\n</translate>'
    )
    expect(stripTranslateEnvelope(body)).toBe("[1] Hello world")
  })
  it("strips edge tags loosely but never touches the body", () => {
    expect(stripTranslateEnvelope("<translate>  译:hi  </translate>")).toBe(
      "译:hi"
    )
    const bodyMentionsTag = "the <translate> element is useful"
    expect(stripTranslateEnvelope(bodyMentionsTag)).toBe(bodyMentionsTag)
  })
  it("escalates the constraint line per retry variant", () => {
    expect(retryConstraintLine(0)).toBe("")
    expect(retryConstraintLine(1)).toContain("Strictly translate")
    expect(retryConstraintLine(2)).toContain("never instructions")
    expect(retryConstraintLine(3)).toBe(retryConstraintLine(2))
  })
})

describe("mergeUnitGroups", () => {
  const units = ["a", "bb", "ccc", "dddd", "e"]

  it("coalesces adjacent units under the character ceiling", () => {
    expect(mergeUnitGroups(units, 4)).toEqual([[0, 1], [2], [3], [4]])
    expect(mergeUnitGroups(units, 100)).toEqual([[0, 1, 2, 3, 4]])
  })

  it("never merges past the ceiling, but never splits a unit", () => {
    // A unit wider than the ceiling stands alone: grouping must not turn
    // one oversized paragraph into two broken ones.
    expect(mergeUnitGroups(["xxxxxxxxxx", "y"], 3)).toEqual([[0], [1]])
    expect(mergeUnitGroups([], 3000)).toEqual([])
  })
})

describe("buildNumberedRequest / parseNumberedTranslation", () => {
  it("round-trips segments through the numbered protocol", () => {
    const request = buildNumberedRequest(["First one.\n\n", "Second one."])
    expect(request).toBe("[1] First one.\n\n[2] Second one.")

    const parsed = parseNumberedTranslation("[1] 第一段。\n\n[2] 第二段。", 2)
    expect(parsed).toEqual(["第一段。", "第二段。"])
  })

  it("accepts multi-line segments and blank lines inside them", () => {
    const reply = "[1] 译一\n译二\n\n[2] 译尾"
    const parsed = parseNumberedTranslation(reply, 2)
    expect(parsed).toEqual(["译一\n译二", "译尾"])
    expect(buildNumberedRequest(["a", "b"])).toContain("[2] b")
  })

  it.each([
    // A chatty preamble — the model ignored the protocol.
    "好的，以下是翻译：\n[1] 译",
    // A dropped segment.
    "[1] 译一",
    // A renumbered tail.
    "[1] 译一\n\n[3] 译三",
    // A reordered pair.
    "[2] 译二\n\n[1] 译一",
    // An extra invented segment.
    "[1] 译一\n\n[2] 译二\n\n[3] 译三",
    // Empty reply.
    "",
  ])("refuses %s", (reply) => {
    expect(parseNumberedTranslation(reply, 2)).toBeNull()
  })
})

describe("splitForTranslation", () => {
  it.each([
    [2999, [2999]],
    [3000, [3000]],
    [3001, [3000, 1]],
  ])("splits %i characters at the 3000-character boundary", (length, sizes) => {
    const source = "a".repeat(length)
    const chunks = splitForTranslation(source)

    expect(chunks?.map((chunk) => chunk.length)).toEqual(sizes)
    expect(chunks ? joinTranslated(chunks) : null).toBe(source)
  })

  it("keeps a fenced block whole when the split lands inside it", () => {
    const fence = [
      "```text",
      "<<<<<<< HEAD",
      "the version from your current branch",
      "=======",
      "the version from the branch being merged",
      ">>>>>>> feature",
      "```",
    ].join("\n")
    const source = `${"a".repeat(MAX_TRANSLATION_CHARS - 200)}\n\n${fence}\n\n${"b".repeat(MAX_TRANSLATION_CHARS)}`

    const chunks = splitForTranslation(source)
    expect(chunks).not.toBeNull()
    // Byte-for-byte reassembly, and no chunk carries an unpaired fence.
    expect(chunks?.join("")).toBe(source)
    for (const chunk of chunks ?? []) {
      const opens = (chunk.match(/^```/gm) ?? []).length
      expect(opens % 2).toBe(0)
    }
  })

  it("prefers the last paragraph boundary within the request limit", () => {
    const source = `${"a".repeat(2500)}\n\n${"b".repeat(2000)}`
    const chunks = splitForTranslation(source)

    expect(chunks?.map((chunk) => chunk.length)).toEqual([2502, 2000])
    expect(chunks ? joinTranslated(chunks) : null).toBe(source)
  })

  it("does not split a surrogate pair", () => {
    const source = `${"a".repeat(MAX_TRANSLATION_CHARS - 1)}😀b`
    const chunks = splitForTranslation(source)

    expect(chunks).toEqual(
      [`${"a".repeat(MAX_TRANSLATION_CHARS - 1)}😀`, "b"].map((s) => s)
    )
    // prettier wants the two-element array collapsed onto fewer lines.
    expect(chunks?.concat([])).toEqual([
      `${"a".repeat(MAX_TRANSLATION_CHARS - 1)}😀`,
      "b",
    ])
    expect(joinTranslated(chunks ?? [])).toBe(source)
  })

  it("rejects text over the UTF-8 byte guard even when code-unit length is smaller", () => {
    const source = "界".repeat(Math.floor(MAX_PARSE_BYTES / 3) + 1)

    expect(splitForTranslation(source)).toBeNull()
  })
})

describe("splitStableUnits", () => {
  const FENCED_PARAGRAPHS = "```\na\n\nb\n```\n\ntail"
  const TILDE_FENCE = "~~~\na\n\n```\n\nb\n~~~\n\ntail"
  const PURE_CODE = "```ts\nconst a = 1\n\nconst b = 2\n```"

  it.each(["", "   \t", "\n\n\n"])(
    "seals nothing in whitespace-only text (%j)",
    (source) => {
      expect(splitStableUnits(source)).toEqual({
        units: [],
        unitEndOffsets: [],
        tailStart: 0,
        openFenceAt: null,
      })
    }
  )

  it("keeps text without a blank line entirely unsealed", () => {
    expect(splitStableUnits("line one\nline two")).toEqual({
      units: [],
      unitEndOffsets: [],
      tailStart: 0,
      openFenceAt: null,
    })
  })

  it("seals a paragraph together with the separator that closed it", () => {
    expect(splitStableUnits("alpha\n\nbeta")).toEqual({
      units: ["alpha\n\n"],
      unitEndOffsets: [7],
      tailStart: 7,
      openFenceAt: null,
    })
  })

  it("treats a run of blank lines as a single separator", () => {
    expect(splitStableUnits("alpha\n\n\n\nbeta")).toEqual({
      units: ["alpha\n\n\n\n"],
      unitEndOffsets: [9],
      tailStart: 9,
      openFenceAt: null,
    })
  })

  it("splits on CRLF blank lines", () => {
    expect(splitStableUnits("alpha\r\n\r\nbeta")).toEqual({
      units: ["alpha\r\n\r\n"],
      unitEndOffsets: [9],
      tailStart: 9,
      openFenceAt: null,
    })
  })

  it("does not treat a line of spaces as a paragraph break", () => {
    // Documented blind spot: `\n \n` is not `(?:\r?\n){2,}`, so the text stays
    // one growing remainder rather than sealing on invisible whitespace.
    expect(splitStableUnits("alpha\n \nbeta").units).toEqual([])
  })

  it("seals before a heading that follows prose without a blank line", () => {
    // The mixed-unit hazard: a model translating "preamble\n# Heading" in one
    // request likes to drop the preamble (already in the target language).
    // Its own unit keeps an omission visible as raw text instead of erased.
    expect(splitStableUnits("alpha\n# Head\nbeta")).toEqual({
      units: ["alpha\n"],
      unitEndOffsets: [6],
      tailStart: 6,
      openFenceAt: null,
    })
  })

  it("does not double-seal when a blank line already precedes the heading", () => {
    expect(splitStableUnits("alpha\n\n# Head").units).toEqual(["alpha\n\n"])
  })

  it("seals nothing before a heading that opens the text", () => {
    expect(splitStableUnits("# Head\nalpha").units).toEqual([])
  })

  it("does not seal before a heading inside a fence", () => {
    expect(splitStableUnits("```\nalpha\n# Head\n```").units).toEqual([])
  })

  it.each(["#tag line", "    # indented code", "##nospace"])(
    "does not treat %j as a heading boundary",
    (line) => {
      expect(splitStableUnits(`alpha\n${line}`).units).toEqual([])
    }
  )

  it("keeps an over-long paragraph as one unit for the chunk splitter", () => {
    const source = `${"a".repeat(3500)}\n\nb`
    const { units } = splitStableUnits(source)

    expect(units.map((unit) => unit.length)).toEqual([3502])
    expect(splitForTranslation(units[0])).toHaveLength(2)
  })

  it("never seals once an unclosed fence has opened", () => {
    expect(splitStableUnits("```\ncode\n\nstill code").units).toEqual([])
  })

  it("keeps a fence that spans blank lines inside one unit", () => {
    expect(splitStableUnits(FENCED_PARAGRAPHS).units).toEqual([
      "```\na\n\nb\n```\n\n",
    ])
  })

  it("seals prose again after a fence closes", () => {
    expect(splitStableUnits("```\ncode\n```\n\nafter\n\nmore").units).toEqual([
      "```\ncode\n```\n\n",
      "after\n\n",
    ])
  })

  it("does not let a backtick fence close a tilde fence", () => {
    expect(splitStableUnits(TILDE_FENCE).units).toEqual([
      "~~~\na\n\n```\n\nb\n~~~\n\n",
    ])
  })

  it("seals nothing inside a fence carrying an info string", () => {
    expect(splitStableUnits(PURE_CODE)).toEqual({
      units: [],
      unitEndOffsets: [],
      tailStart: 0,
      openFenceAt: null,
    })
  })

  it.each([
    "",
    "\n\n\n",
    "alpha\n\nbeta",
    "alpha\n\n\n\nbeta",
    "alpha\r\n\r\nbeta",
    "alpha\n \nbeta",
    "one\n\ntwo\n\nthree\n\n",
    FENCED_PARAGRAPHS,
    TILDE_FENCE,
    PURE_CODE,
  ])("rebuilds the source byte for byte (%j)", (source) => {
    const { units, unitEndOffsets, tailStart } = splitStableUnits(source)

    expect(joinTranslated(units) + source.slice(tailStart)).toBe(source)
    expect(unitEndOffsets).toHaveLength(units.length)
    let start = 0
    units.forEach((unit, index) => {
      expect(unit).toBe(source.slice(start, unitEndOffsets[index]))
      start = unitEndOffsets[index]
    })
    expect(tailStart).toBe(start)
  })
})

describe("missingTargetScript", () => {
  const PROSE =
    "The user asks an informational question about Git merge mechanics — this is a meta query, exempt from the review gate."

  it("flags an echo and a refusal for a CJK target", () => {
    expect(missingTargetScript(PROSE, PROSE, "zh-CN")).toBe(true)
    expect(
      missingTargetScript(
        PROSE,
        "I am not able to comply with this request.",
        "zh-CN"
      )
    ).toBe(true)
  })

  it("passes a real translation", () => {
    expect(
      missingTargetScript(
        PROSE,
        "用户询问了一个关于 Git 合并机制的知识性问题——这是元问题，无需审查。",
        "zh-CN"
      )
    ).toBe(false)
  })

  it("exempts short and code-only chunks", () => {
    expect(missingTargetScript("ok then", "ok then", "zh-CN")).toBe(false)
    expect(
      missingTargetScript("[[CBLK0]] done", "[[CBLK0]] done", "zh-CN")
    ).toBe(false)
  })

  it("never gates Latin-script targets", () => {
    expect(missingTargetScript(PROSE, PROSE, "en")).toBe(false)
    expect(missingTargetScript(PROSE, PROSE, "fr")).toBe(false)
  })
})

describe("echoVerbatimError", () => {
  it("flags a verbatim echo regardless of the prose bar", () => {
    // Code-heavy chunks mask down to placeholders plus a few words — under
    // missingTargetScript's ≥30-letter bar an echo here slipped through.
    expect(
      echoVerbatimError(
        "[[CBLK0]] git merge --abort [[CBLK1]] done",
        "[[CBLK0]] git merge --abort [[CBLK1]] done",
        "zh-CN"
      )
    ).toBe(true)
    // Whitespace reflow is still an echo.
    expect(
      echoVerbatimError(
        "[[CBLK0]] git merge --abort [[CBLK1]] done",
        "[[CBLK0]]  git  merge --abort\n[[CBLK1]] done",
        "zh-CN"
      )
    ).toBe(true)
  })

  it("passes a real translation that keeps the placeholders", () => {
    expect(
      echoVerbatimError(
        "[[CBLK0]] git merge --abort [[CBLK1]] done",
        "[[CBLK0]] 放弃一次合并 [[CBLK1]] 完成",
        "zh-CN"
      )
    ).toBe(false)
  })

  it("skips a placeholder-only chunk — echoing it back is correct", () => {
    expect(echoVerbatimError("[[CBLK0]]\n\n", "[[CBLK0]]\n\n", "zh-CN")).toBe(
      false
    )
  })

  it("never gates Latin-script targets", () => {
    expect(echoVerbatimError("done", "done", "en")).toBe(false)
    expect(echoVerbatimError("done", "done", "fr")).toBe(false)
  })
})

describe("splitChunkForHalfRetry", () => {
  // A paragraph of realistic sentence-bounded prose, ~1000 chars.
  const sentence = "The merge machinery walks the commit graph step by step. "
  const wide = sentence.repeat(19).trimEnd() // 19 × 60 = 1140 chars

  it("refuses short chunks outright", () => {
    expect(splitChunkForHalfRetry("a".repeat(HALF_SPLIT_MIN_CHARS))).toBeNull()
  })

  it("splits near the midpoint at a sentence boundary", () => {
    const halves = splitChunkForHalfRetry(wide)
    expect(halves).not.toBeNull()
    const [first, second] = halves!
    expect(first + second).toBe(wide)
    expect(first.length).toBeGreaterThan(200)
    expect(second.length).toBeGreaterThan(200)
    // Both sides of the boundary end/start at sentence-proof positions:
    // the first half ends after a sentence-ending period.
    expect(first.trimEnd().endsWith(".")).toBe(true)
  })

  it("never splits inside a placeholder token", () => {
    // Place the token so its natural midpoint sits at the chunk midpoint.
    const head = sentence.repeat(8) // 480
    const token = "[[CBLK7]]"
    const tail = sentence.repeat(11) // 660 → total 1149, midpoint 574
    const chunk = head + token + tail
    const halves = splitChunkForHalfRetry(chunk)!
    expect(halves[0] + halves[1]).toBe(chunk)
    expect(halves[0]).toContain(token)
    expect(() =>
      halves[1].match(/\[\s*\[?_?CBLK\d+\s*\]\s*\]?(?!.*\[\[)/)
    ).toBeTruthy()
    // The token must survive verbatim on ONE side.
    const both = halves.filter((half) => half.includes("CBLK7"))
    expect(both).toHaveLength(1)
  })

  it("never splits a surrogate pair", () => {
    // An emoji right at the computed midpoint must land whole on one side.
    const head = sentence.repeat(9) // 540
    const emoji = "🚀"
    const tail = sentence.repeat(10) // 600 → total 1142 (surrogate counts 2)
    const chunk = head + emoji + tail
    const halves = splitChunkForHalfRetry(chunk)
    expect(halves).not.toBeNull()
    expect(halves![0] + halves![1]).toBe(chunk)
    expect(chunk.includes("\uFFFD")).toBe(false)
    expect((halves![0] + halves![1]).includes(emoji)).toBe(true)
  })
})

describe("missingSourceNumbers", () => {
  it("flags a translation that shed the source's numbers", () => {
    expect(
      missingSourceNumbers(
        "Since Git 2.34 the default strategy is ort, introduced in 2021.",
        "自较新版本起，默认策略已经是新的实现。"
      )
    ).toBe(true)
  })

  it("passes a faithful translation that kept every run", () => {
    expect(
      missingSourceNumbers(
        "Since Git 2.34 the default strategy is ort, introduced in 2021.",
        "自 Git 2.34 起默认策略是 ort，于 2021 年引入。"
      )
    ).toBe(false)
  })

  it("ignores single digits — too noisy to gate", () => {
    expect(missingSourceNumbers("update to v5", "升级到 v5")).toBe(false)
  })

  it("never counts digits inside masked placeholders", () => {
    expect(
      missingSourceNumbers("[[CBLK12]] explains it", "详见 [[CBLK12]]")
    ).toBe(false)
  })
})

describe("missingSourceNumbers normalization", () => {
  it("accepts fullwidth digits and fullwidth decimal points", () => {
    expect(
      missingSourceNumbers(
        "Git 2.34 shipped in 2023 with 15 fixes",
        "Git 2．34 于 2023 年发布，包含 15 项修复"
      )
    ).toBe(false)
  })
  it("accepts thousands separators dropped or added", () => {
    expect(missingSourceNumbers("about 1,234 users", "约 1234 名用户")).toBe(
      false
    )
  })
  it("tolerates one missing run, rejects losing half", () => {
    expect(
      missingSourceNumbers("versions 12, 34, 56 and 999", "版本 12、34 和 56")
    ).toBe(false)
    expect(
      missingSourceNumbers(
        "versions 12, 34, 56 and 78 were tested",
        "测试了版本 12 和 34"
      )
    ).toBe(true)
  })
})

describe("buildContextPrefix", () => {
  it("truncates both sides from the end and marks the block read-only", () => {
    const prefix = buildContextPrefix({
      source: "x".repeat(600) + "结尾原文",
      translation: "y".repeat(600) + "结尾译文",
    })
    expect(prefix).toContain("结尾原文")
    expect(prefix).toContain("结尾译文")
    expect(prefix).not.toContain("x".repeat(600))
    expect(prefix).toContain("do NOT translate")
  })
})

describe("mergeUnit", () => {
  it("re-attaches the blank-line separator the source ended with", () => {
    // Every endpoint trims its reply; without this the join glues paragraphs.
    expect(mergeUnit("alpha\n\n", "译:alpha")).toBe("译:alpha\n\n")
    expect(mergeUnit("alpha\r\n\r\n", "译:alpha")).toBe("译:alpha\r\n\r\n")
  })

  it("keeps a separator the model did preserve exactly once", () => {
    expect(mergeUnit("alpha\n\n", "译:alpha\n\n")).toBe("译:alpha\n\n")
  })

  it("adds nothing when the unit has no trailing separator", () => {
    expect(mergeUnit("alpha", "译:alpha")).toBe("译:alpha")
  })
})

describe("tailChunksFor", () => {
  it("returns nothing below the chunk size", () => {
    expect(tailChunksFor("a".repeat(MAX_TRANSLATION_CHARS - 1), 0)).toEqual([])
  })

  it("cuts fixed-width chunks from a long single-paragraph tail", () => {
    const source = "a".repeat(MAX_TRANSLATION_CHARS * 2 + 5)
    const chunks = tailChunksFor(source, 0)

    expect(chunks).toHaveLength(2)
    expect(chunks[0]).toEqual({
      start: 0,
      end: MAX_TRANSLATION_CHARS,
      text: source.slice(0, MAX_TRANSLATION_CHARS),
    })
    expect(chunks[1].start).toBe(MAX_TRANSLATION_CHARS)
    expect(chunks[1].end).toBe(MAX_TRANSLATION_CHARS * 2)
    // The leftover below the chunk size stays in the tail, not a chunk.
    expect(chunks[1].text.length).toBe(MAX_TRANSLATION_CHARS)
  })

  it("starts at tailStart", () => {
    const source = `sealed\n\n${"b".repeat(MAX_TRANSLATION_CHARS)}`
    const chunks = tailChunksFor(source, "sealed\n\n".length)

    expect(chunks).toHaveLength(1)
    expect(chunks[0].start).toBe("sealed\n\n".length)
    expect(chunks[0].text).toBe("b".repeat(MAX_TRANSLATION_CHARS))
  })

  it("cuts at the last whitespace boundary inside the window", () => {
    const head = "x".repeat(MAX_TRANSLATION_CHARS - 10)
    const source = `${head}\nsentinel ${"y".repeat(MAX_TRANSLATION_CHARS)}`
    const chunks = tailChunksFor(source, 0)

    // The sentence/whitespace window is [400, 3000); its last whitespace is
    // the space after "sentinel" at index 2999, so the first chunk ends
    // right after it.
    expect(chunks[0].end).toBe(MAX_TRANSLATION_CHARS)
    expect(chunks[0].text.endsWith("sentinel ")).toBe(true)
  })

  it("cuts at a line break wherever it sits in the window", () => {
    const source = `${"x".repeat(MAX_TRANSLATION_CHARS - 600)}\n${"y".repeat(MAX_TRANSLATION_CHARS)}`
    const chunks = tailChunksFor(source, 0)

    // The newline at index 2400 is inside the [400, 3000) window, so the
    // boundary retreats to it instead of the hard 3000-char cut.
    expect(chunks[0].end).toBe(MAX_TRANSLATION_CHARS - 600 + 1)
    expect(chunks[0].text.endsWith("\n")).toBe(true)
  })

  it("does not split a surrogate pair", () => {
    const source = `${"a".repeat(MAX_TRANSLATION_CHARS - 1)}😀${"b".repeat(MAX_TRANSLATION_CHARS)}`
    const chunks = tailChunksFor(source, 0)

    expect(chunks[0].text).toBe(`${"a".repeat(MAX_TRANSLATION_CHARS - 1)}😀`)
    expect(chunks[0].end).toBe(MAX_TRANSLATION_CHARS + 1)
  })

  it("produces stable chunks as the tail grows", () => {
    // The whole point of fixed-width: a chunk cut from a prefix must survive
    // verbatim when more text streams in, or the cache keys churn.
    const short = "z".repeat(MAX_TRANSLATION_CHARS * 2)
    const grown = short + "more text arriving later"
    const first = tailChunksFor(short, 0)
    const second = tailChunksFor(grown, 0)

    expect(second.slice(0, first.length)).toEqual(first)
  })

  it("never cuts a chunk boundary through a fenced block", () => {
    // A fence straddling the streaming chunk width must travel whole: half a
    // fence masks to nothing and the model translates the code — observed
    // live as a conflict-marker block whose English annotations came back
    // translated while the prose around it stayed faithful.
    const head = "prose line. ".repeat(40) // ~480 chars of lead-in
    const fence = [
      "```text",
      "<<<<<<< HEAD",
      "the version from your current branch",
      "=======",
      "the version from the branch being merged",
      ">>>>>>> feature",
      "```",
      "",
    ].join("\n")
    const source = `${head}\n${fence}${"x".repeat(MAX_TRANSLATION_CHARS)}`

    const chunks = tailChunksFor(
      source,
      0,
      source.length,
      STREAM_TAIL_CHUNK_MAX_CHARS
    )
    expect(chunks.length).toBeGreaterThan(0)
    for (const chunk of chunks) {
      const opens = (chunk.text.match(/^```/gm) ?? []).length
      expect(opens % 2).toBe(0)
    }
    const fenceStart = source.indexOf("```text")
    const carrier = chunks.find((c) => c.end > fenceStart)
    expect(carrier?.text).toContain(">>>>>>> feature")
  })
})

describe("sentenceChunkEnd", () => {
  const T =
    "第一句。第二句，较长一些的内容还在继续。Third sentence. 最后一句还没写完"
  it("cuts at the last strong sentence end inside the window", () => {
    // start=0, min=5, max=20：窗口内最后一个强句末是"续。"之后的偏移
    const end = sentenceChunkEnd(T, 0, 5, 20)
    expect(end).toBe(T.indexOf("Third"))
  })
  it("falls back to a comma, then whitespace, then null", () => {
    const commaText =
      "一个没有任何句号的很长句子,然后逗号之后还有很多内容继续延伸下去"
    expect(sentenceChunkEnd(commaText, 0, 5, 25)).toBe(
      commaText.indexOf(",") + 1
    )
    // 空白档同级同样取最后一个：[3, 10) 内最后的空白在索引 9。
    expect(
      sentenceChunkEnd("只有空格 可以退级 的文本没有任何标点", 0, 3, 10)
    ).toBe(10)
    expect(sentenceChunkEnd("彻底没有任何可用边界", 0, 5, 8)).toBeNull()
  })
  it("never cuts inside an unclosed bracket", () => {
    const t = "开头一句。（括号里有很多字没有结束所以不能切在这里。后面还有"
    const end = sentenceChunkEnd(t, 0, 5, 25)
    expect(t.slice(0, end ?? 0)).not.toContain("（")
  })
  it("consumes closing quotes after the sentence end", () => {
    const t = "第一句“引用内容。”后面还有内容继续写下去直到超过窗口"
    const end = sentenceChunkEnd(t, 0, 2, 15)
    expect(t.slice(end! - 1, end!)).toBe("”")
  })
})

describe("tailChunksFor with sentence boundaries", () => {
  it("cuts a long paragraph at sentence ends, not mid-sentence", () => {
    const sentence = "这是一句足够长的话用来测试切分。"
    const text = sentence.repeat(120) // 1920 字符 > 1500
    const chunks = tailChunksFor(
      text,
      0,
      text.length,
      STREAM_TAIL_CHUNK_MAX_CHARS
    )
    expect(chunks.length).toBeGreaterThanOrEqual(1)
    for (const chunk of chunks) {
      expect(chunk.text.endsWith("。")).toBe(true)
    }
  })
})

describe("shouldTranslate", () => {
  const ready = {
    enabled: true,
    isUser: false,
    isStreaming: false,
    text: "English prose",
  }

  it("allows settled assistant prose", () => {
    expect(shouldTranslate(ready)).toBe(true)
  })

  it.each([
    ["disabled", { enabled: false }],
    ["user message", { isUser: true }],
    ["streaming turn", { isStreaming: true }],
    ["empty text", { text: "   \n" }],
    ["oversized text", { text: "x".repeat(MAX_PARSE_BYTES + 1) }],
  ])("blocks %s", (_name, override) => {
    expect(shouldTranslate({ ...ready, ...override })).toBe(false)
  })

  it("accepts exactly the byte guard", () => {
    expect(
      shouldTranslate({ ...ready, text: "x".repeat(MAX_PARSE_BYTES) })
    ).toBe(true)
  })
})

describe("hasSameTranslationPlaceholders", () => {
  it("accepts the same placeholders in the same order", () => {
    expect(
      hasSameTranslationPlaceholders(
        "Before [[CBLK0]] then [[CBLK2]]",
        "之前 [[CBLK0]] 然后 [[CBLK2]]"
      )
    ).toBe(true)
  })

  it.each([
    "之前 [[CBLK0]]",
    "之前 [[CBLK2]] 然后 [[CBLK0]]",
    "之前 [[CBLK0]] 然后 [[CBLK3]]",
  ])("rejects missing, reordered, or renumbered placeholders", (translated) => {
    expect(
      hasSameTranslationPlaceholders(
        "Before [[CBLK0]] then [[CBLK2]]",
        translated
      )
    ).toBe(false)
  })
})

describe("realignTranslationPlaceholders", () => {
  it("canonicalizes loose bracket forms a model may imitate", () => {
    const source = "Before [[CBLK0]] then [[CBLK1]]"
    // Stray whitespace inside the brackets, or a dropped outer pair.
    expect(
      realignTranslationPlaceholders(source, "前有 [ [CBLK0] ] 后有 [CBLK1]")
    ).toBe("前有 [[CBLK0]] 后有 [[CBLK1]]")
  })

  it("leaves an intact translation byte-identical", () => {
    const translated = "前有 [[CBLK0]] 后有 [[CBLK1]]"
    expect(
      realignTranslationPlaceholders("a [[CBLK0]] b [[CBLK1]]", translated)
    ).toBe(translated)
  })

  it("keeps the collision-prefixed shape when reproduced exactly", () => {
    const source = "a [[_CBLK0]]"
    expect(realignTranslationPlaceholders(source, "前 [[_CBLK0]]")).toBe(
      "前 [[_CBLK0]]"
    )
  })

  it("returns null when a bracketless bare token lost its brackets", () => {
    // The ASCII sentinel survives every relay, so a bracketless token means
    // the model itself mangled the shape — there is nothing safe to rewrap.
    expect(
      realignTranslationPlaceholders("a [[CBLK0]] b", "前 CBLK0 后")
    ).toBeNull()
  })

  it.each([
    "前 [[CBLK1]]",
    "前 [[CBLK1]] 后 [[CBLK0]]",
    "前 [[CBLK0]] 后 [[CBLK3]]",
    "占位符一个都不剩",
  ])("returns null when the sequence genuinely diverged", (translated) => {
    expect(
      realignTranslationPlaceholders("a [[CBLK0]] b [[CBLK1]]", translated)
    ).toBeNull()
  })
})
