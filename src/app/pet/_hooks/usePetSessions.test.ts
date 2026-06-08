import { act, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/pet/api", () => ({ listActivePetSessions: vi.fn() }))

// Capture the `pet://sessions` handler the hook registers so a test can fire a
// live event; the default impl resolves like a healthy Tauri subscription.
let capturedHandler: ((raw: unknown) => void) | null = null
const mockSubscribe = vi.fn()
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ subscribe: mockSubscribe }),
}))

import { usePetSessions } from "./usePetSessions"
import { listActivePetSessions } from "@/lib/pet/api"
import type { PetSessionEntry, PetSessionsPayload } from "@/lib/pet/types"

const mockSnapshot = vi.mocked(listActivePetSessions)

function entry(id: string): PetSessionEntry {
  return {
    connectionId: id,
    conversationId: 1,
    folderId: 1,
    agentType: "claude_code",
    title: id,
    status: "prompting",
  }
}

function payload(sessions: PetSessionEntry[]): PetSessionsPayload {
  return {
    runningCount: sessions.length,
    waitingCount: 0,
    errorCount: 0,
    sessions,
  }
}

beforeEach(() => {
  capturedHandler = null
  mockSubscribe.mockReset()
  mockSnapshot.mockReset()
  // Healthy default: subscription resolves and captures the live handler.
  mockSubscribe.mockImplementation(
    async (_event: string, handler: (raw: unknown) => void) => {
      capturedHandler = handler
      return () => {}
    }
  )
  // Quiet the intentional retry warnings.
  vi.spyOn(console, "warn").mockImplementation(() => {})
})

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe("usePetSessions", () => {
  it("populates from the snapshot even when the subscription fails (regression)", async () => {
    // The fresh panel window's first `subscribe()` IPC can reject; the snapshot
    // must still run (it used to be nested after subscribe, so it was skipped).
    mockSubscribe.mockRejectedValue(new Error("listen not ready"))
    mockSnapshot.mockResolvedValue(payload([entry("c1")]))

    const { result } = renderHook(() => usePetSessions())

    await waitFor(() => expect(result.current.sessions).toHaveLength(1))
    expect(result.current.sessions[0].connectionId).toBe("c1")
  })

  it("retries the snapshot after a transient failure", async () => {
    vi.useFakeTimers()
    // Isolate the snapshot retry path (no post-subscribe re-fetch).
    mockSubscribe.mockRejectedValue(new Error("listen not ready"))
    mockSnapshot
      .mockRejectedValueOnce(new Error("ipc not ready"))
      .mockResolvedValue(payload([entry("c1")]))

    const { result } = renderHook(() => usePetSessions())

    // Initial attempt rejected → empty; advance past the first backoff (150ms).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(200)
    })
    expect(result.current.sessions).toHaveLength(1)
    expect(result.current.sessions[0].connectionId).toBe("c1")
  })

  it("lets a live event win over a later snapshot", async () => {
    // Snapshot stays pending so the live event lands first.
    let resolveSnapshot: (p: PetSessionsPayload) => void = () => {}
    mockSnapshot.mockReturnValue(
      new Promise<PetSessionsPayload>((r) => {
        resolveSnapshot = r
      })
    )

    const { result } = renderHook(() => usePetSessions())
    await waitFor(() => expect(capturedHandler).not.toBeNull())

    act(() => capturedHandler?.(payload([entry("live")])))
    expect(result.current.sessions[0].connectionId).toBe("live")

    // The (now stale) snapshot resolves — it must NOT overwrite the live value.
    await act(async () => {
      resolveSnapshot(payload([entry("stale")]))
      await Promise.resolve()
    })
    expect(result.current.sessions[0].connectionId).toBe("live")
  })

  it("stops snapshot retries once a snapshot has loaded", async () => {
    vi.useFakeTimers()
    // First attempt fails; the rest succeed. The subscription succeeds, so its
    // post-subscribe fetch loads data and latches — a pending retry must not
    // keep re-fetching forever after that.
    mockSnapshot
      .mockRejectedValueOnce(new Error("ipc not ready"))
      .mockResolvedValue(payload([entry("c1")]))

    const { result } = renderHook(() => usePetSessions())
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300)
    })
    expect(result.current.sessions).toHaveLength(1)

    const callsSoFar = mockSnapshot.mock.calls.length
    // Far past any backoff — no further snapshot fetches should be scheduled.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(mockSnapshot.mock.calls.length).toBe(callsSoFar)
  })

  it("does not update state after unmount", async () => {
    let resolveSnapshot: (p: PetSessionsPayload) => void = () => {}
    mockSnapshot.mockReturnValue(
      new Promise<PetSessionsPayload>((r) => {
        resolveSnapshot = r
      })
    )

    const { result, unmount } = renderHook(() => usePetSessions())
    unmount()

    await act(async () => {
      resolveSnapshot(payload([entry("c1")]))
      await Promise.resolve()
    })
    // The post-unmount resolution is ignored (cancelled guard) — stays empty.
    expect(result.current.sessions).toHaveLength(0)
  })
})
