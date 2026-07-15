import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { PromptDraft } from "@/lib/types"
import {
  AUTO_REPLY_SETTINGS_KEY,
  createDefaultAutoReplySettings,
} from "@/lib/auto-reply/storage"
import { __resetAutoReplySettingsStoreForTests } from "@/lib/auto-reply/settings-store"
import { useAutoReplyEngine } from "./use-auto-reply-engine"

function draftTexts(calls: PromptDraft[][]): string[] {
  return calls.map((args) => args[0]?.displayText ?? "")
}

function baseArgs(overrides: Partial<Parameters<typeof useAutoReplyEngine>[0]> = {}) {
  return {
    enabled: true,
    status: "connected" as const,
    error: null,
    claudeApiRetry: {
      sessionId: "s1",
      attempt: 1,
      maxRetries: 3,
      error: "Too Many Requests",
      errorStatus: 429,
      retryDelayMs: 1000,
    },
    pendingPermission: false,
    pendingQuestion: false,
    pendingAskQuestion: false,
    onSend: vi.fn(),
    ...overrides,
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  window.localStorage.clear()
  window.localStorage.setItem(
    AUTO_REPLY_SETTINGS_KEY,
    JSON.stringify(createDefaultAutoReplySettings())
  )
  __resetAutoReplySettingsStoreForTests()
})

afterEach(() => {
  vi.useRealTimers()
  window.localStorage.clear()
  __resetAutoReplySettingsStoreForTests()
})

describe("useAutoReplyEngine", () => {
  it("does not schedule when disabled", () => {
    const onSend = vi.fn()
    const { result } = renderHook(() =>
      useAutoReplyEngine(baseArgs({ enabled: false, onSend }))
    )
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(result.current.pending).toBeNull()
    expect(onSend).not.toHaveBeenCalled()
  })

  it("schedules 429 and sends continue after delayMs", () => {
    const onSend = vi.fn()
    const { result } = renderHook(() => useAutoReplyEngine(baseArgs({ onSend })))
    expect(result.current.pending?.replyText).toBe("\u7ee7\u7eed")
    expect(result.current.pending?.ruleId).toBe("builtin-http-429")

    act(() => {
      vi.advanceTimersByTime(2999)
    })
    expect(onSend).not.toHaveBeenCalled()

    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(onSend).toHaveBeenCalledTimes(1)
    expect(draftTexts(onSend.mock.calls)).toEqual(["\u7ee7\u7eed"])
    expect(result.current.pending).toBeNull()
  })

  it("cancelPending prevents send", () => {
    const onSend = vi.fn()
    const { result } = renderHook(() => useAutoReplyEngine(baseArgs({ onSend })))
    act(() => {
      result.current.cancelPending()
    })
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("notifyManualSend prevents send", () => {
    const onSend = vi.fn()
    const { result } = renderHook(() => useAutoReplyEngine(baseArgs({ onSend })))
    act(() => {
      result.current.notifyManualSend()
    })
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("cancels when connection becomes unsafe", () => {
    const onSend = vi.fn()
    const { result, rerender } = renderHook(
      (props) => useAutoReplyEngine(props),
      { initialProps: baseArgs({ onSend }) }
    )
    expect(result.current.pending).not.toBeNull()
    rerender(baseArgs({ onSend, status: "prompting" }))
    expect(result.current.pending).toBeNull()
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("cancels when signal clears before fire", () => {
    const onSend = vi.fn()
    const { result, rerender } = renderHook(
      (props) => useAutoReplyEngine(props),
      { initialProps: baseArgs({ onSend }) }
    )
    expect(result.current.pending).not.toBeNull()
    rerender(
      baseArgs({
        onSend,
        claudeApiRetry: null,
        error: null,
      })
    )
    expect(result.current.pending).toBeNull()
    act(() => {
      vi.advanceTimersByTime(5000)
    })
    expect(onSend).not.toHaveBeenCalled()
  })

  it("enforces maxPerBurst and surfaces stop notice", () => {
    const onSend = vi.fn()
    // maxPerBurst default is 3; fire three times with enough cooldown gap.
    // Use a short-cooldown custom settings set.
    const settings = createDefaultAutoReplySettings()
    settings.rules = settings.rules.map((rule) =>
      rule.id === "builtin-http-429"
        ? { ...rule, delayMs: 100, cooldownMs: 0, maxPerBurst: 2 }
        : rule
    )
    window.localStorage.setItem(AUTO_REPLY_SETTINGS_KEY, JSON.stringify(settings))
    __resetAutoReplySettingsStoreForTests()

    const { result, rerender } = renderHook(
      (props) => useAutoReplyEngine(props),
      { initialProps: baseArgs({ onSend }) }
    )

    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(onSend).toHaveBeenCalledTimes(1)

    // Re-introduce the same signal after send cleared pending.
    rerender(baseArgs({ onSend, claudeApiRetry: null }))
    rerender(baseArgs({ onSend }))
    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(onSend).toHaveBeenCalledTimes(2)

    rerender(baseArgs({ onSend, claudeApiRetry: null }))
    rerender(baseArgs({ onSend }))
    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(onSend).toHaveBeenCalledTimes(2)
    expect(result.current.stopNotice?.reason).toBe("max_per_burst")
  })

  it("allows a new burst after the signal changes", () => {
    const onSend = vi.fn()
    const settings = createDefaultAutoReplySettings()
    settings.rules = settings.rules.map((rule) => ({
      ...rule,
      delayMs: 50,
      cooldownMs: 0,
      maxPerBurst: 1,
    }))
    window.localStorage.setItem(AUTO_REPLY_SETTINGS_KEY, JSON.stringify(settings))
    __resetAutoReplySettingsStoreForTests()

    const { rerender } = renderHook((props) => useAutoReplyEngine(props), {
      initialProps: baseArgs({ onSend }),
    })
    act(() => {
      vi.advanceTimersByTime(50)
    })
    expect(onSend).toHaveBeenCalledTimes(1)

    // Same 429 signal should be blocked by maxPerBurst=1.
    rerender(baseArgs({ onSend, claudeApiRetry: null }))
    rerender(baseArgs({ onSend }))
    act(() => {
      vi.advanceTimersByTime(50)
    })
    expect(onSend).toHaveBeenCalledTimes(1)

    // Distinct 503 signal is a new burst.
    rerender(
      baseArgs({
        onSend,
        claudeApiRetry: {
          sessionId: "s1",
          attempt: 1,
          maxRetries: 3,
          error: "Service Unavailable",
          errorStatus: 503,
          retryDelayMs: 1000,
        },
      })
    )
    act(() => {
      vi.advanceTimersByTime(50)
    })
    expect(onSend).toHaveBeenCalledTimes(2)
  })
})
