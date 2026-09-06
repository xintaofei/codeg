import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  translate: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  getTranslationSettings: mocks.getSettings,
  translateTexts: mocks.translate,
}))

beforeEach(() => {
  vi.resetModules()
  mocks.getSettings.mockReset()
  mocks.translate.mockReset()
})

const ENABLED = {
  enabled: true,
  providers: [],
  baseUrl: "https://api.example.com",
  apiKey: "••••••••",
  model: "translator",
  targetLang: null,
  translateThinking: false,
  apiFormat: "auto" as const,
  selectionTranslate: true,
  selectionTargetLang: null,
  toggleAlwaysVisible: false,
  batchMaxChars: null,
  carryContext: true,
}

async function setup(settings = ENABLED) {
  mocks.getSettings.mockResolvedValue(settings)
  return import("./use-translated-text")
}

describe("useTranslatedText", () => {
  it.each([
    ["streaming", { isStreaming: true }],
    ["user message", { isUser: true }],
    ["outside the translation viewport", { shouldLoad: false }],
  ])("does not request translation while %s", async (_name, override) => {
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello `code`",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
        ...override,
      })
    )

    await waitFor(() => expect(mocks.getSettings).toHaveBeenCalledTimes(1))
    expect(mocks.translate).not.toHaveBeenCalled()
    expect(result.current.display).toBe("Hello `code`")
    expect(result.current.isTranslated).toBe(false)
  })

  it("defaults to original text while settings are disabled", async () => {
    const { useTranslatedText } = await setup({ ...ENABLED, enabled: false })
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(mocks.getSettings).toHaveBeenCalledTimes(1))
    expect(mocks.translate).not.toHaveBeenCalled()
    expect(result.current.display).toBe("Hello")
  })

  it("masks literals, translates settled prose, and restores literals", async () => {
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好 [[CBLK0]]", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello `const x = 1`",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(result.current.isTranslated).toBe(true))
    expect(result.current.display).toBe("你好 `const x = 1`")
    expect(mocks.translate).toHaveBeenCalledWith(
      ["<translate target=\"zh-CN\">\nHello [[CBLK0]]\n</translate>"],
      "zh-CN",
      false,
      null
    )
  })

  it("canonicalizes loose bracket forms the model imitates", async () => {
    // A model imitating the sentinel sometimes drops an outer bracket pair;
    // the tolerant recovery canonicalizes it before the sequence gate.
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好 [CBLK0]", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello `const x = 1`",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(result.current.isTranslated).toBe(true))
    expect(result.current.display).toBe("你好 `const x = 1`")
  })

  it("strips a stray numbered prefix the model adds to an un-numbered chunk", async () => {
    // The protocol example in the system prompt makes some endpoints prefix
    // even single-chunk input with "[1] " — it must not ride into the text.
    mocks.translate.mockResolvedValue([
      { key: "k", text: "[1] 你好 [[CBLK0]]", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello `const x = 1`",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(result.current.isTranslated).toBe(true))
    expect(result.current.display).toBe("你好 `const x = 1`")
  })

  it("sends a plain-text mask verbatim for selection translation", async () => {
    // Selection text read from the DOM is not Markdown: the conflict-marker
    // run must reach the endpoint as-is, not masked into a fake `<tag>`
    // placeholder that the model then leaves untranslated.
    mocks.translate.mockResolvedValue([
      { key: "k", text: "冲突标记 <<<<<<< HEAD", fromCache: false },
    ])
    const { requestTranslationDetailed } = await setup()
    const attempt = await requestTranslationDetailed(
      "a <<<<<<< HEAD hunk",
      "zh-CN",
      "k-plain",
      true,
      null,
      (text) => ({ masked: text, restore: (rewritten) => rewritten })
    )

    expect(attempt.text).toBe("冲突标记 <<<<<<< HEAD")
    expect(mocks.translate).toHaveBeenCalledWith(
      ["<translate target=\"zh-CN\">\na <<<<<<< HEAD hunk\n</translate>"],
      "zh-CN",
      true,
      null
    )
  })

  it("falls back to original when the model corrupts a placeholder", async () => {
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello `const x = 1`",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(mocks.translate).toHaveBeenCalledTimes(1))
    expect(result.current.display).toBe("Hello `const x = 1`")
    expect(result.current.isTranslated).toBe(false)
  })

  it("falls back to original when the endpoint answers with an empty translation", async () => {
    // A chunk already written in the target language is the one a model likes
    // to "translate" into nothing; storing that would erase the source.
    mocks.translate.mockResolvedValue([
      { key: "k", text: "  ", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(mocks.translate).toHaveBeenCalledTimes(1))
    expect(result.current.display).toBe("Hello")
    expect(result.current.isTranslated).toBe(false)
  })

  it("falls back to original when the endpoint answers with an invented essay", async () => {
    // The observed failure: a one-line source, a self-written essay back.
    // Serving it would graft content the source never had into the message.
    mocks.translate.mockImplementation(async (chunks: string[]) =>
      chunks.map((chunk) => ({
        key: chunk,
        text: `这是一篇与源文无关的小作文。${"补".repeat(chunk.length * 5 + 400)}`,
        fromCache: false,
      }))
    )
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "下面按要求用英文分多段详细展开。",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(mocks.translate).toHaveBeenCalledTimes(1))
    expect(result.current.display).toBe("下面按要求用英文分多段详细展开。")
    expect(result.current.isTranslated).toBe(false)
  })

  it("toggles to original and back without requesting again", async () => {
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result } = renderHook(() =>
      useTranslatedText({
        text: "Hello",
        isStreaming: false,
        isUser: false,
        shouldLoad: true,
        uiLocale: "zh-CN",
        blockKey: "block-1",
      })
    )

    await waitFor(() => expect(result.current.display).toBe("你好"))
    act(() => result.current.showOriginal())
    expect(result.current.display).toBe("Hello")
    act(() => result.current.showTranslation())
    expect(result.current.display).toBe("你好")
    expect(mocks.translate).toHaveBeenCalledTimes(1)
  })

  it("shares a cached translation across remounts", async () => {
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const props = {
      text: "Hello",
      isStreaming: false,
      isUser: false,
      shouldLoad: true,
      uiLocale: "zh-CN",
      blockKey: "block-1",
    }

    const first = renderHook(() => useTranslatedText(props))
    await waitFor(() => expect(first.result.current.display).toBe("你好"))
    first.unmount()

    const second = renderHook(() => useTranslatedText(props))
    await waitFor(() => expect(second.result.current.display).toBe("你好"))
    expect(mocks.translate).toHaveBeenCalledTimes(1)
  })

  it("ignores a stale result after the source text changes", async () => {
    let resolveFirst!: (
      value: Array<{ key: string; text: string; fromCache: boolean }>
    ) => void
    mocks.translate
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = resolve
          })
      )
      .mockResolvedValueOnce([{ key: "new", text: "新的", fromCache: false }])

    const { useTranslatedText } = await setup()
    const { result, rerender } = renderHook(
      ({ text }) =>
        useTranslatedText({
          text,
          isStreaming: false,
          isUser: false,
          shouldLoad: true,
          uiLocale: "zh-CN",
          blockKey: "block-1",
        }),
      { initialProps: { text: "Old" } }
    )

    await waitFor(() => expect(mocks.translate).toHaveBeenCalledTimes(1))
    rerender({ text: "New" })
    await waitFor(() => expect(result.current.display).toBe("新的"))

    await act(async () => {
      resolveFirst([{ key: "old", text: "旧的", fromCache: false }])
      await Promise.resolve()
    })
    expect(result.current.display).toBe("新的")
  })

  it("makes zero requests while disabled, then requests once re-enabled", async () => {
    mocks.translate.mockResolvedValue([
      { key: "k", text: "你好", fromCache: false },
    ])
    const { useTranslatedText } = await setup()
    const { result, rerender } = renderHook(
      ({ disabled }) =>
        useTranslatedText({
          text: "Hello",
          isStreaming: false,
          isUser: false,
          shouldLoad: true,
          uiLocale: "zh-CN",
          blockKey: "block-1",
          disabled,
        }),
      { initialProps: { disabled: true } }
    )

    await waitFor(() => expect(mocks.getSettings).toHaveBeenCalledTimes(1))
    expect(mocks.translate).not.toHaveBeenCalled()
    expect(result.current.display).toBe("Hello")

    rerender({ disabled: false })
    await waitFor(() => expect(result.current.display).toBe("你好"))
    expect(mocks.translate).toHaveBeenCalledTimes(1)
  })
})

describe("useTranslationEnabled", () => {
  it("mirrors the enabled setting", async () => {
    const { useTranslationEnabled } = await setup()
    const { result } = renderHook(() => useTranslationEnabled())

    await waitFor(() => expect(result.current).toBe(true))
  })

  it("stays false while disabled", async () => {
    const { useTranslationEnabled } = await setup({
      ...ENABLED,
      enabled: false,
    })
    const { result } = renderHook(() => useTranslationEnabled())

    await waitFor(() => expect(mocks.getSettings).toHaveBeenCalledTimes(1))
    expect(result.current).toBe(false)
  })

  it("reacts to settings primed after mount", async () => {
    const { primeTranslationSettings, useTranslationEnabled } = await setup({
      ...ENABLED,
      enabled: false,
    })
    const { result } = renderHook(() => useTranslationEnabled())
    expect(result.current).toBe(false)

    act(() =>
      primeTranslationSettings({
        ...ENABLED,
        translateThinking: true,
      })
    )
    expect(result.current).toBe(true)
  })
})
