import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  translate: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  getTranslationSettings: mocks.getSettings,
  translateTexts: mocks.translate,
}))

const ENABLED = {
  enabled: true,
  providers: [],
  baseUrl: "https://api.example.com",
  apiKey: "••••••••",
  model: "translator",
  targetLang: null,
  translateThinking: true,
  apiFormat: "auto" as const,
  selectionTranslate: true,
  selectionTargetLang: null,
  toggleAlwaysVisible: false,
  batchMaxChars: null,
  carryContext: true,
}

type Texts = string[]

/** A well-behaved endpoint: prefix every chunk so restores stay verifiable,
 * and answer a numbered group in kind so grouped dispatches succeed. A
 * well-behaved endpoint also never outputs the carry-context reference
 * block or the <translate> envelope, so both are stripped from the request
 * before echoing — a real endpoint translates only the inner content. */
const unwrap = (raw: string) =>
  raw
    .replace(
      /^\[Reference for consistency only[\s\S]*?\[End of reference[^\n]*\n/,
      ""
    )
    // Retry-shape escalation prefixes the envelope with a constraint line
    // (retryConstraintLine) — instruction, not data, so a faithful
    // endpoint's reply still translates only the envelope's inner body.
    .replace(/^Strictly translate[^\n]*\n/, "")
    .replace(/^You are a translation engine[^\n]*\n/, "")
    .replace(/^<translate[^>]*>\n?/, "")
    .replace(/\n?<\/translate>\s*$/, "")
const ok = async (texts: Texts) =>
  texts.map((raw) => {
    const text = unwrap(raw)
    if (/^\[1\] /m.test(text)) {
      const segments = text.split(/(?:^|\n)\[\d+\] /).slice(1)
      const reply = segments
        .map((segment, index) => `[${index + 1}] 译:${segment.trim()}`)
        .join("\n\n")
      return { key: raw, text: reply, fromCache: false }
    }
    return { key: raw, text: `译:${text}`, fromCache: false }
  })

const WINDOW = 3_500

beforeEach(() => {
  vi.resetModules()
  vi.useFakeTimers()
  mocks.getSettings.mockReset()
  mocks.translate.mockReset()
  mocks.getSettings.mockResolvedValue(ENABLED)
})

afterEach(() => {
  vi.useRealTimers()
})

/** Flush the microtask chain without moving fake time. */
const flush = () => act(async () => void (await vi.advanceTimersByTimeAsync(0)))

const advance = (ms: number) =>
  act(async () => void (await vi.advanceTimersByTimeAsync(ms)))

async function setup() {
  const mod = await import("./use-streaming-translated-text")
  const { primeTranslationSettings } = await import("./use-translated-text")
  act(() => {
    primeTranslationSettings(ENABLED)
  })
  return mod
}

function renderStream(
  mod: Awaited<ReturnType<typeof setup>>,
  initial: { text: string; isStreaming: boolean },
  blockKey: string,
  enabled = true
) {
  return renderHook(
    ({ text, isStreaming }: { text: string; isStreaming: boolean }) =>
      mod.useStreamingTranslatedText({
        text,
        isStreaming,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey,
        enabled,
      }),
    { initialProps: initial }
  )
}

describe("useStreamingTranslatedText", () => {
  it("retries an invented whole-chunk reply as two halves", async () => {
    const mod = await setup()
    // A sealed wide paragraph: the endpoint answers it with a self-written
    // essay (far over the 2.5× invention bar), but translates each half
    // faithfully when the chunk goes back out split.
    const paragraph =
      "The commit graph walks every merge step by careful step. ".repeat(18)
    const full = `${paragraph}\n\ntail`
    mocks.translate.mockImplementation(async (texts: Texts) =>
      texts.map((raw) => {
        if (raw.includes(paragraph)) {
          // The invention: the endpoint answered the text, 3× over.
          return { key: raw, text: "编".repeat(3000), fromCache: false }
        }
        const text = unwrap(raw)
        return { key: raw, text: `译:${text.trim()}`, fromCache: false }
      })
    )
    const { rerender, result } = renderStream(
      mod,
      { text: full, isStreaming: true },
      "half-split"
    )
    await flush()
    await advance(WINDOW)
    // The halves landed as one chunk-sized piece — the paragraph shows
    // translated, no invented filler anywhere.
    expect(result.current.display).toContain("译:")
    expect(result.current.display).not.toContain("编")

    // Settle: the raw tail flushes through the same well-behaved path.
    rerender({ text: full, isStreaming: false })
    await flush()
    await advance(WINDOW)
    expect(result.current.display.endsWith("译:tail")).toBe(true)
  })

  it("sends nothing while no unit has sealed", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { result, rerender } = renderStream(
      mod,
      { text: "still one paragraph", isStreaming: true },
      "sealed"
    )

    rerender({
      text: "still one paragraph, and now more of it",
      isStreaming: true,
    })
    await advance(10 * WINDOW)

    expect(mocks.translate).not.toHaveBeenCalled()
    expect(result.current.display).toBe(
      "still one paragraph, and now more of it"
    )
  })

  it("sends at most one batch per pacing window", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "one\n\n", isStreaming: true },
      "throttle"
    )

    // The fake clock starts at the real epoch, so a fresh dispatch is always
    // past the pacing window: the first unit goes out immediately. The wire
    // body rides inside the <translate> envelope; judge the inner content.
    expect(mocks.translate).toHaveBeenCalledTimes(1)
    expect(unwrap(mocks.translate.mock.calls[0][0][0])).toBe("one\n\n")

    rerender({ text: "one\n\ntwo\n\n", isStreaming: true })
    await flush()
    rerender({ text: "one\n\ntwo\n\nthree\n\n", isStreaming: true })
    await flush()
    rerender({ text: "one\n\ntwo\n\nthree\n\nfour\n\n", isStreaming: true })
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    await advance(WINDOW)
    // The three sealed units ride ONE numbered request — that is what the
    // grouping buys: one round trip per pacing window, not one per paragraph.
    // carryContext is on: the request carries the previous piece ("one") as a
    // read-only reference ahead of the numbered body.
    expect(mocks.translate).toHaveBeenCalledTimes(2)
    expect(mocks.translate.mock.calls[1][0][0]).toContain(
      "[Reference for consistency only"
    )
    expect(mocks.translate.mock.calls[1][0][0]).toContain("Source: one")
    expect(mocks.translate.mock.calls[1][0][0]).toContain(
      "[1] two\n\n[2] three\n\n[3] four"
    )
  })

  it("dispatches before the window once enough new text sealed", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "seed\n\n", isStreaming: true },
      "chars"
    )
    await advance(WINDOW)
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    const filler = "x".repeat(300)
    rerender({ text: `seed\n\n${filler}\n\ntail`, isStreaming: true })
    await flush()

    expect(mocks.translate).toHaveBeenCalledTimes(2)
    // The batch carries the previous piece ("seed") as its consistency
    // reference, with the filler unit as the actual payload.
    expect(mocks.translate.mock.calls[1][0][0]).toContain("Source: seed")
    expect(mocks.translate.mock.calls[1][0][0]).toContain(filler)
  })

  it("sends each sealed unit byte for byte inside its numbered group", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    renderStream(
      mod,
      { text: "First para.\n\nSecond para.\n\n", isStreaming: true },
      "bytes"
    )
    await advance(WINDOW)

    // Two sealed units share one numbered request; each segment inside it is
    // the unit verbatim (trimmed by the framing, restored by the parser).
    const sent = mocks.translate.mock.calls.map((call) => unwrap(call[0][0]))
    expect(sent).toContainEqual("[1] First para.\n\n[2] Second para.")
  })

  it("converges a cold settled block and serves a remount from cache", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const initial = { text: "Alpha.\n\nBeta.", isStreaming: false }

    const first = renderStream(mod, initial, "cold")
    await flush()
    // Settled work rides MERGED segments: adjacent sealed units coalesce into
    // one request-sized span, so a 13k-char reply converges in a handful of
    // round trips instead of one per paragraph.
    expect(
      mocks.translate.mock.calls.map((call) => unwrap(call[0][0]))
    ).toEqual(["Alpha.\n\nBeta."])
    expect(first.result.current.display).toBe("译:Alpha.\n\nBeta.")

    first.unmount()
    const second = renderStream(mod, initial, "cold")
    await flush()
    expect(
      mocks.translate.mock.calls.map((call) => unwrap(call[0][0]))
    ).toEqual(["Alpha.\n\nBeta."])
    expect(second.result.current.display).toBe("译:Alpha.\n\nBeta.")
  })

  it("never sends the growing whole text while streaming", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const fullText = "p1\n\np2\n\np3"
    const { rerender, result } = renderStream(
      mod,
      { text: "p1\n\n", isStreaming: true },
      "whole"
    )
    await advance(WINDOW)
    rerender({ text: "p1\n\np2\n\n", isStreaming: true })
    await advance(WINDOW)
    rerender({ text: fullText, isStreaming: true })
    await flush()

    for (const call of mocks.translate.mock.calls) {
      expect(call[0]).not.toContain(fullText)
    }

    rerender({ text: fullText, isStreaming: false })
    await flush()
    // The settle flush rides without a reference (the whole block is one
    // request and the model sees the full text), so the payload is "p3" raw.
    const lastCall = mocks.translate.mock.calls[
      mocks.translate.mock.calls.length - 1
    ][0] as string[]
    expect(lastCall[0]).toContain("p3")
    expect(lastCall[0]).not.toContain("[Reference for consistency")
    expect(result.current.display).toBe("译:p1\n\n译:p2\n\n译:p3")
  })

  it("bounds the settle flush at the nearest piece beyond a chain gap", async () => {
    const mod = await setup()
    // The endpoint refuses the GAP chunk until settle: mid-stream, its
    // siblings (one/three/four) land as pieces while the chain stays broken
    // at the gap — later paragraphs are translated but unrenderable.
    let gapAllowed = false
    mocks.translate.mockImplementation(async (texts: Texts) =>
      texts.map((raw) => {
        if (!gapAllowed && raw.includes("GAP")) {
          return { key: raw, text: "", error: "RATE", fromCache: false }
        }
        const text = unwrap(raw)
        if (/^\[1\] /m.test(text)) {
          const segments = text.split(/(?:^|\n)\[\d+\] /).slice(1)
          return {
            key: raw,
            text: segments
              .map((segment, index) => `[${index + 1}] 译:${segment.trim()}`)
              .join("\n\n"),
            fromCache: false,
          }
        }
        return { key: raw, text: `译:${text.trim()}`, fromCache: false }
      })
    )
    const fullText = "one\n\nGAP\n\nthree\n\nfour\n\n"
    const { rerender, result } = renderStream(
      mod,
      { text: fullText, isStreaming: true },
      "gap"
    )
    // Exhaust the streaming retries (2 × backoff) and the pause cooldown so
    // the machine is in its settled-input state with the gap still open.
    await advance(60_000)
    expect(result.current.display).toBe("译:one\n\nGAP\n\nthree\n\nfour\n\n")
    // Every retry changed the request (temperature 0 makes an identical
    // retry an identical wrong answer): the escalated attempts carry the
    // strict constraint lines.
    const gapAttempts = mocks.translate.mock.calls
      .map((call) => (call[0] as string[])[0])
      .filter((text) => text.includes("GAP"))
    expect(
      gapAttempts.some((text) => text.includes("Strictly translate"))
    ).toBe(true)

    gapAllowed = true
    rerender({ text: fullText, isStreaming: false })
    await flush()
    await advance(WINDOW)

    // The settle flush re-requests ONLY the gap, inside the XML envelope:
    // the pieces behind it (three/four) already sit in the store, and an
    // unbounded flush would have re-translated all of them in one giant
    // request.
    const settleCall = mocks.translate.mock.calls[
      mocks.translate.mock.calls.length - 1
    ][0] as string[]
    expect(settleCall[0]).toContain("<translate target=")
    expect(settleCall[0]).toContain("GAP")
    expect(settleCall[0]).not.toContain("three")
    // The chain reconnects through the filled gap and the stored pieces
    // render immediately — no re-translation wait for the settled tail.
    expect(result.current.display).toBe(
      "译:one\n\n译:GAP\n\n译:three\n\n译:four\n\n"
    )
  })

  it("re-flushes the tail when in-flight units land after the settle flush", async () => {
    const mod = await setup()
    const resolvers: Array<{
      texts: Texts
      resolve: (
        value: Array<{ key: string; text: string; fromCache: boolean }>
      ) => void
    }> = []
    mocks.translate.mockImplementation(
      (texts: Texts) =>
        new Promise((resolve) => {
          resolvers.push({ texts, resolve })
        })
    )
    const fullText = "u1\n\nu2\n\ntail"
    const { rerender, result } = renderStream(
      mod,
      { text: fullText, isStreaming: true },
      "race"
    )
    await advance(WINDOW)
    // u1 and u2 ride one numbered group; the tail is not final yet.
    expect(mocks.translate).toHaveBeenCalledTimes(1)
    expect(unwrap(mocks.translate.mock.calls[0][0][0])).toBe("[1] u1\n\n[2] u2")

    // Settle while the group request is still in flight: the flush cannot
    // know its results yet, so it requests from the untranslated prefix —
    // the whole text (the accepted one-shot double-spend).
    rerender({ text: fullText, isStreaming: false })
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(2)
    expect(unwrap(mocks.translate.mock.calls[1][0][0])).toBe(fullText)

    // The group lands and moves the translated prefix past the remainder's
    // start. A one-shot settle guard would leave "tail" raw forever.
    await act(async () => {
      resolvers[0].resolve([
        { key: "", text: "[1] 译:u1\n\n[2] 译:u2", fromCache: false },
      ])
      await vi.advanceTimersByTimeAsync(0)
    })

    expect(result.current.display).toBe("译:u1\n\n译:u2\n\ntail")
    expect(mocks.translate).toHaveBeenCalledTimes(3)
    expect(unwrap(mocks.translate.mock.calls[2][0][0])).toBe("tail")

    await act(async () => {
      resolvers[2].resolve([{ key: "", text: "译:tail", fromCache: false }])
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(result.current.display).toBe("译:u1\n\n译:u2\n\n译:tail")
    expect(result.current.isTranslated).toBe(true)
  })

  it("restores the translation when the block mounts before its text arrives", async () => {
    // The settle-time re-split remounts blocks a frame before the reparse
    // fills their parts: the initializer restores nothing against the empty
    // text. Without the incremental re-lookup the block flashes raw, loses
    // its toggle, and restarts translation from zero — the "settled and the
    // translation vanished" report.
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const fullText = "Alpha.\n\nBeta."

    // Translate while streaming so the pieces land in the store.
    const first = renderStream(
      mod,
      { text: fullText, isStreaming: false },
      "flash"
    )
    await flush()
    expect(first.result.current.display).toBe("译:Alpha.\n\nBeta.")
    first.unmount()

    // Remount with the text MISSING, then arriving: the restore must pick
    // the stored pieces up without a single new request.
    const second = renderStream(mod, { text: "", isStreaming: false }, "flash")
    await flush()
    expect(second.result.current.display).toBe("")
    second.rerender({ text: fullText, isStreaming: false })
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)
    expect(second.result.current.display).toBe("译:Alpha.\n\nBeta.")
    expect(second.result.current.isTranslated).toBe(true)
  })

  it("pauses after repeated failed batches and still converges on settle", async () => {
    const mod = await setup()
    mocks.translate.mockRejectedValue(new Error("endpoint down"))
    const { rerender, result } = renderStream(
      mod,
      { text: "a\n\n", isStreaming: true },
      "pause"
    )

    // Each batch's unit retries STREAM_UNIT_RETRY_LIMIT extra times before
    // reporting failure: 3 units × (1 + 2) = 9 calls to reach the pause.
    await advance(WINDOW)
    rerender({ text: "a\n\nb\n\n", isStreaming: true })
    await advance(WINDOW)
    rerender({ text: "a\n\nb\n\nc\n\n", isStreaming: true })
    await advance(5 * WINDOW)
    const spent = mocks.translate.mock.calls.length
    expect(spent).toBe(9)

    // Three consecutive all-failed batches: PAUSED — more streaming windows
    // stay silent instead of spending more requests.
    rerender({ text: "a\n\nb\n\nc\n\nd\n\n", isStreaming: true })
    await advance(6 * WINDOW)
    expect(mocks.translate.mock.calls.length).toBe(spent)

    // The settle flush bypasses the pause and converges the block.
    mocks.translate.mockResolvedValue([
      { key: "", text: "译:abcd", fromCache: false },
    ])
    rerender({ text: "a\n\nb\n\nc\n\nd\n\n", isStreaming: false })
    await flush()
    expect(mocks.translate.mock.calls.length).toBe(spent + 1)
    // mergeUnit re-attaches the source's trailing blank line (the endpoint
    // trims every reply), so the display keeps the paragraph break.
    expect(result.current.display).toBe("译:abcd\n\n")
    expect(result.current.isTranslated).toBe(true)
  })

  it("retries a wholly failed batch once the endpoint recovers", async () => {
    const mod = await setup()
    let failing = true
    mocks.translate.mockImplementation(async (texts: Texts) => {
      if (failing) throw new Error("endpoint down")
      return ok(texts)
    })
    const { result } = renderStream(
      mod,
      { text: "only\n\nunit", isStreaming: true },
      "retry"
    )
    await advance(WINDOW)
    // The initial dispatch went out; the failure is mid-retry (the unit-level
    // backoff spans 3 s and 6 s, inside this window), so only assert that the
    // batch was sent and the raw text still shows.
    expect(mocks.translate).toHaveBeenCalled()
    // The failed batch strands nothing: the raw text still shows, and ...
    expect(result.current.display).toBe("only\n\nunit")

    // ... once the endpoint answers again, the retry converges the unit: the
    // batch-level retry fires after STREAM_FAILURE_RETRY_MS, and the unit's
    // own backoffs (3 s + 6 s) may still be running inside it.
    failing = false
    await advance(15_000)
    expect(result.current.display).toBe("译:only\n\nunit")
    expect(result.current.hasTranslation).toBe(true)
  })

  it("sends nothing while P6 is off", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "p\n\n", isStreaming: true },
      "off",
      false
    )
    await advance(4 * WINDOW)
    rerender({ text: "p\n\nmore", isStreaming: false })
    await flush()

    expect(mocks.translate).not.toHaveBeenCalled()
  })

  it("disarms the pacing timer on unmount", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { unmount, rerender } = renderStream(
      mod,
      { text: "one\n\n", isStreaming: true },
      "unmount"
    )
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    // A fresh sealed unit inside the pacing window arms a timer — the only
    // state where a fired callback would spend a request. Unmounting must
    // disarm it.
    rerender({ text: "one\n\ntwo\n\n", isStreaming: true })
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)
    unmount()
    await advance(4 * WINDOW)

    expect(mocks.translate).toHaveBeenCalledTimes(1)
  })

  it("keeps the user's show-original choice across flushes", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { result, rerender } = renderStream(
      mod,
      { text: "p1\n\n", isStreaming: true },
      "toggle"
    )
    await advance(WINDOW)
    await flush()
    expect(result.current.isTranslated).toBe(true)

    act(() => result.current.showOriginal())
    expect(result.current.display).toBe("p1\n\n")

    rerender({ text: "p1\n\np2", isStreaming: true })
    await advance(WINDOW)
    expect(result.current.display).toBe("p1\n\np2")

    act(() => result.current.showTranslation())
    expect(result.current.display).toBe("译:p1\n\np2")
  })
})

describe("streaming batching width and pacing", () => {
  it("fills one numbered request up to the 3000-char width", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const paragraph = "A".repeat(90) + "\n\n" // 92 字符 × 8 段 = 736
    const { rerender } = renderStream(mod, { text: "", isStreaming: true }, "k")
    await flush()
    // 时钟未动（fake clock 从真实纪元起算，首次派发总是已 due），从空文本
    // 一次性累计 8 个封口段落后才产生第一次派发：一个 numbered 请求带全部
    // 段落（旧实现 5 单元上限会把它截成 5 段）。
    rerender({ text: paragraph.repeat(8), isStreaming: true })
    await flush()
    const sent = mocks.translate.mock.calls.map((call) =>
      (call[0] as string[]).join("\n---\n")
    )
    const numbered = sent.filter((text) => /^\[1\] /m.test(text))
    expect(numbered.length).toBe(1)
    expect(numbered[0].split(/\[\d+\] /).length - 1).toBeGreaterThanOrEqual(8)
  })

  it("does not dispatch before 3s or 800 new chars", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "seed\n\n", isStreaming: true },
      "k"
    )
    // fake clock 从真实纪元起算：首派发立即发出，节拍窗从它起算。
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    rerender({ text: `seed\n\n${"短".repeat(100)}\n\n`, isStreaming: true })
    await flush()
    await advance(2_000)
    // 2s < 3s 下限，且新增 102 字符 < 800：不得派发。
    expect(mocks.translate).not.toHaveBeenCalledTimes(2)
    await advance(1_000) // 累计 3s
    expect(mocks.translate).toHaveBeenCalledTimes(2)
  })

  it("prepends the previous piece as a reference when carryContext is on", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "第一段落内容。\n\n", isStreaming: true },
      "k"
    )
    await flush()
    rerender({
      text: "第一段落内容。\n\n第二段落紧随其后。\n\n",
      isStreaming: true,
    })
    await flush()
    await advance(3_000)
    const sent = mocks.translate.mock.calls.map((call) =>
      (call[0] as string[]).join("\n---\n")
    )
    const withRef = sent.filter((text) =>
      text.includes("[Reference for consistency")
    )
    expect(withRef.length).toBeGreaterThanOrEqual(1)
    expect(withRef[0]).toContain("第一段落内容。")
  })
})
