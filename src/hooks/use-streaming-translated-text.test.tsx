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

  it("splits after a backend length-gate rejection, not just a judged one", async () => {
    const mod = await setup()
    // The backend's own gate rejects the reply before the frontend judge
    // ever sees it — the observed production shape ("far longer than its
    // source"). The chunk must still come back out as two halves.
    const paragraph =
      "The commit graph walks every merge step by careful step. ".repeat(18)
    const full = `${paragraph}\n\ntail`
    mocks.translate.mockImplementation(async (texts: Texts) =>
      texts.map((raw) => {
        if (raw.includes(paragraph)) {
          return {
            key: raw,
            text: "",
            error: `The translation is far longer than its source (3000 vs ${paragraph.length} characters)`,
            fromCache: false,
          }
        }
        const text = unwrap(raw)
        return { key: raw, text: `译:${text.trim()}`, fromCache: false }
      })
    )
    const { rerender, result } = renderStream(
      mod,
      { text: full, isStreaming: true },
      "backend-split"
    )
    await flush()
    await advance(WINDOW)
    expect(result.current.display).toContain("译:")
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

    // Settle while the group request is still in flight: the flush holds off
    // (the in-flight subsegments overlap this gap, and a flush sent past them
    // that got refused would stitch the whole block raw over their landings),
    // then re-runs once the landing re-opens the boundary.
    rerender({ text: fullText, isStreaming: false })
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    // The group lands and moves the translated prefix past the remainder's
    // start. A one-shot settle guard would leave "tail" raw forever.
    await act(async () => {
      resolvers[0].resolve([
        { key: "", text: "[1] 译:u1\n\n[2] 译:u2", fromCache: false },
      ])
      await vi.advanceTimersByTimeAsync(0)
    })

    expect(result.current.display).toBe("译:u1\n\n译:u2\n\ntail")
    expect(mocks.translate).toHaveBeenCalledTimes(2)
    expect(unwrap(mocks.translate.mock.calls[1][0][0])).toBe("tail")

    await act(async () => {
      resolvers[1].resolve([{ key: "", text: "译:tail", fromCache: false }])
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

  it("lands separator runs as identity pieces without a request", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender, result } = renderStream(
      mod,
      { text: "---\n\nreal paragraph\n\n", isStreaming: true },
      "separator"
    )
    await flush()
    await advance(WINDOW)
    // The separator never rides the wire (checked after unwrapping, since a
    // carry-context reference may legitimately quote a previous piece).
    for (const [texts] of mocks.translate.mock.calls) {
      for (const raw of texts as string[]) {
        expect(unwrap(raw)).not.toContain("---")
      }
    }
    // Settle: the separator stays raw, the paragraph translates.
    rerender({ text: "---\n\nreal paragraph\n\n", isStreaming: false })
    await flush()
    await advance(WINDOW)
    expect(result.current.display).toContain("---")
    expect(result.current.display).toContain("译:real paragraph")
  })

  it("gives up on a gap after three failed replays", async () => {
    const mod = await setup()
    const callTimes: number[] = []
    mocks.translate.mockImplementation(async (texts: Texts) => {
      callTimes.push(Date.now())
      return texts.map((raw) => ({
        key: raw,
        text: "",
        error: "endpoint refuses this chunk",
        fromCache: false,
      }))
    })
    const { rerender, result } = renderStream(
      mod,
      { text: "stubborn\n\n", isStreaming: true },
      "giveup"
    )
    // Streaming attempts fail; the settle flush and the gap replay take
    // over with their bounded retries (variant escalation each round).
    await advance(WINDOW)
    rerender({ text: "stubborn\n\n", isStreaming: false })
    await flush()
    // Every retry chain (streaming attempts, settle-flush retries, gap
    // replay rounds) is bounded — advance until the mock goes silent, then
    // verify it stays silent. The exact call count races between the two
    // retry chains, so only the convergence is asserted.
    let spent = -1
    for (let i = 0; i < 20; i += 1) {
      await advance(600_000)
      if (mocks.translate.mock.calls.length === spent) break
      spent = mocks.translate.mock.calls.length
    }
    expect(spent).toBeGreaterThan(0)
    // The gap has been dropped, not replayed forever: no further requests,
    // no recorded gap, and the raw source stays on display.
    await advance(600_000)
    expect(mocks.translate.mock.calls.length).toBe(spent)
    expect(mod.findPendingGaps("stubborn\n\n")).toEqual([])
    expect(result.current.display).toContain("stubborn")
    expect(result.current.display).not.toContain("译:")
  })

  it("persists a landing that races a re-key so the next instance restores it", async () => {
    const mod = await setup()
    let resolveRequest: (() => void) | null = null
    mocks.translate.mockImplementation(
      (texts: Texts) =>
        new Promise((resolve) => {
          resolveRequest = () =>
            resolve(
              texts.map((raw) => ({
                key: raw,
                text: `译:${unwrap(raw).trim()}`,
                fromCache: false,
              }))
            )
        })
    )
    const full = "one\n\n"
    const render = (blockKey: string) =>
      renderHook(
        ({ blockKey: key }: { blockKey: string }) =>
          mod.useStreamingTranslatedText({
            text: full,
            isStreaming: false,
            shouldLoad: true,
            uiLocale: "zh-CN",
            blockKey: key,
            enabled: true,
          }),
        { initialProps: { blockKey } }
      )
    const first = render("race-a")
    await flush()
    expect(mocks.translate).toHaveBeenCalledTimes(1)

    // The settle reparse re-keys the block while the request is on the wire.
    await act(async () => {
      first.rerender({ blockKey: "race-b" })
    })
    // The reply lands under the old key: it must still reach the store.
    await act(async () => {
      resolveRequest?.()
    })

    // A fresh instance at the new key restores by content and does not pay
    // for the segment again.
    const spent = mocks.translate.mock.calls.length
    const second = render("race-b")
    await flush()
    await advance(WINDOW)
    expect(second.result.current.display).toContain("译:one")
    expect(mocks.translate.mock.calls.length).toBe(spent)
  })

  it("keeps the display chain whole when the first gap is given up", async () => {
    const mod = await setup()
    // The lead segment is refused forever while its sibling lands. Giving up
    // on the lead gap must stitch the raw source, not just drop the record:
    // a missing piece at offset 0 breaks the chain at the first byte and
    // blanks every translated paragraph behind it.
    mocks.translate.mockImplementation(async (texts: Texts) =>
      texts.map((raw) => {
        if (raw.includes("stubborn")) {
          return {
            key: raw,
            text: "",
            error: "endpoint refuses this chunk",
            fromCache: false,
          }
        }
        const text = unwrap(raw)
        return { key: raw, text: `译:${text.trim()}`, fromCache: false }
      })
    )
    const { rerender, result } = renderStream(
      mod,
      { text: "stubborn opener\n\ngood follower\n\n", isStreaming: true },
      "chain"
    )
    await advance(WINDOW)
    await advance(WINDOW)
    rerender({
      text: "stubborn opener\n\ngood follower\n\n",
      isStreaming: false,
    })
    await flush()
    // Every retry chain is bounded; advance until the mock goes silent.
    let spent = -1
    for (let i = 0; i < 20; i += 1) {
      await advance(600_000)
      if (mocks.translate.mock.calls.length === spent) break
      spent = mocks.translate.mock.calls.length
    }
    // The refused lead shows raw; the follower's translation is NOT hidden
    // behind it.
    expect(result.current.display).toContain("stubborn opener")
    expect(result.current.display).toContain("译:good follower")
    expect(result.current.display).not.toContain("译:stubborn")
    expect(mod.findPendingGaps("stubborn opener\n\ngood follower\n\n")).toEqual(
      []
    )
  })

  it("lands a Chinese preamble in an English reply without a request", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const full =
      "这是一道纯知识讲解请求，按豁免清单直接回答。\n\nEnglish body.\n\n"
    const { rerender, result } = renderStream(
      mod,
      { text: full, isStreaming: true },
      "zh-preamble"
    )
    await flush()
    await advance(WINDOW)
    // The Han-dominant preamble never rides the wire; the English body does.
    for (const [texts] of mocks.translate.mock.calls) {
      for (const raw of texts as string[]) {
        expect(unwrap(raw)).not.toContain("纯知识讲解请求")
      }
    }
    rerender({ text: full, isStreaming: false })
    await flush()
    await advance(WINDOW)
    // Chain stays whole: preamble raw, body translated.
    expect(result.current.display).toContain("这是一道纯知识讲解请求")
    expect(result.current.display).toContain("译:English body.")
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

    rerender({ text: `seed\n\n${"more ".repeat(30)}\n\n`, isStreaming: true })
    await flush()
    await advance(2_000)
    // 2s < 3s 下限，且新增约 150 字符 < 800：不得派发。
    expect(mocks.translate).not.toHaveBeenCalledTimes(2)
    await advance(1_000) // 累计 3s
    expect(mocks.translate).toHaveBeenCalledTimes(2)
  })

  it("prepends the previous piece as a reference when carryContext is on", async () => {
    const mod = await setup()
    mocks.translate.mockImplementation(ok)
    const { rerender } = renderStream(
      mod,
      { text: "First paragraph content.\n\n", isStreaming: true },
      "k"
    )
    await flush()
    rerender({
      text: "First paragraph content.\n\nSecond paragraph follows.\n\n",
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
    expect(withRef[0]).toContain("First paragraph content.")
  })
})
