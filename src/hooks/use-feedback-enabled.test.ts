import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({ getFeedbackSettings: vi.fn() }))

// Capture the backend-broadcast handler the hook registers via `subscribe`, so
// tests can simulate a `feedback-settings://changed` event from another window.
let capturedEventHandler: ((s: { enabled: boolean }) => void) | null = null
vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(
    async (_event: string, handler: (s: { enabled: boolean }) => void) => {
      capturedEventHandler = handler
      return () => {}
    }
  ),
  onTransportReconnect: vi.fn(() => null),
}))

// The hook caches at module scope; reset the module registry per test so each
// starts with a fresh (uncached) singleton.
beforeEach(() => {
  vi.resetModules()
  capturedEventHandler = null
})

async function setup(getImpl: () => Promise<{ enabled: boolean }>) {
  const api = await import("@/lib/api")
  vi.mocked(api.getFeedbackSettings).mockImplementation(getImpl)
  return import("./use-feedback-enabled")
}

describe("useFeedbackEnabled", () => {
  it("reflects the fetched value on mount", async () => {
    // init is false (uncached); the fetch resolves true and must propagate via
    // the listener — proving the load path, not just the lazy default.
    const { useFeedbackEnabled } = await setup(async () => ({ enabled: true }))
    const { result } = renderHook(() => useFeedbackEnabled())
    await waitFor(() => expect(result.current).toBe(true))
  })

  it("reacts to a settings save without a remount", async () => {
    const { useFeedbackEnabled, primeFeedbackEnabled } = await setup(
      async () => ({ enabled: false })
    )
    const { result } = renderHook(() => useFeedbackEnabled())
    await waitFor(() => expect(result.current).toBe(false))

    act(() => primeFeedbackEnabled(true))
    expect(result.current).toBe(true)
  })

  it("converges to a cross-window broadcast (save made in the settings window)", async () => {
    // The exact production bug: settings runs in a separate window, so its save
    // can't reach this window through the in-process cache — only the backend
    // `feedback-settings://changed` broadcast does. The hook must apply it.
    const { useFeedbackEnabled } = await setup(async () => ({ enabled: false }))
    const { result } = renderHook(() => useFeedbackEnabled())
    await waitFor(() => expect(result.current).toBe(false))

    // A save in the settings window → backend broadcast lands here.
    act(() => capturedEventHandler?.({ enabled: true }))
    expect(result.current).toBe(true)
  })

  it("notifies every mounted hook (open conversations) on change", async () => {
    const { useFeedbackEnabled, primeFeedbackEnabled } = await setup(
      async () => ({ enabled: false })
    )
    const a = renderHook(() => useFeedbackEnabled())
    const b = renderHook(() => useFeedbackEnabled())
    await waitFor(() => expect(a.result.current).toBe(false))

    act(() => primeFeedbackEnabled(true))
    expect(a.result.current).toBe(true)
    expect(b.result.current).toBe(true)
  })

  it("a save during the in-flight initial load wins (no stale overwrite)", async () => {
    let resolveFetch: (v: { enabled: boolean }) => void = () => {}
    const { useFeedbackEnabled, primeFeedbackEnabled } = await setup(
      () =>
        new Promise<{ enabled: boolean }>((r) => {
          resolveFetch = r
        })
    )
    const { result } = renderHook(() => useFeedbackEnabled())

    // A save lands while the initial fetch is still pending.
    act(() => primeFeedbackEnabled(true))
    expect(result.current).toBe(true)

    // The stale fetch now resolves with the OLD value — it must NOT clobber the
    // newer save.
    await act(async () => {
      resolveFetch({ enabled: false })
      await Promise.resolve()
    })
    expect(result.current).toBe(true)
  })
})
