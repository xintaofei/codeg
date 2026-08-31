import { act, renderHook } from "@testing-library/react"
import type { Mock } from "vitest"
import { beforeEach, describe, expect, it, vi } from "vitest"

type InstallEvent = { task_id: string; kind: string; payload: string }
type Handler = (event: InstallEvent) => void

vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn(),
}))

import { subscribe } from "@/lib/platform"
import { MAX_INSTALL_LOG_LINES } from "@/lib/install-stream"
import { useAgentInstallStream } from "./use-agent-install-stream"

const mockSubscribe = subscribe as unknown as Mock

let handlers: Handler[]
let unsubs: Array<ReturnType<typeof vi.fn>>

beforeEach(() => {
  handlers = []
  unsubs = []
  mockSubscribe.mockReset()
  mockSubscribe.mockImplementation(async (_event: string, handler: Handler) => {
    handlers.push(handler)
    const unsub = vi.fn()
    unsubs.push(unsub)
    return unsub
  })
})

function emit(event: InstallEvent) {
  for (const handler of handlers) handler(event)
}

describe("useAgentInstallStream", () => {
  it("caps logs at MAX_INSTALL_LOG_LINES, keeping the tail", async () => {
    const { result } = renderHook(() => useAgentInstallStream())
    await act(async () => {
      await result.current.start("task-1")
    })

    act(() => {
      for (let i = 0; i < MAX_INSTALL_LOG_LINES + 100; i++) {
        emit({ task_id: "task-1", kind: "log", payload: `line-${i}` })
      }
    })

    expect(result.current.logs).toHaveLength(MAX_INSTALL_LOG_LINES)
    expect(result.current.logs[0]).toBe("line-100")
    expect(result.current.logs[result.current.logs.length - 1]).toBe(
      `line-${MAX_INSTALL_LOG_LINES + 99}`
    )
  })

  it("ignores events for other task ids", async () => {
    const { result } = renderHook(() => useAgentInstallStream())
    await act(async () => {
      await result.current.start("task-1")
    })

    act(() => {
      emit({ task_id: "other-task", kind: "log", payload: "nope" })
      emit({ task_id: "task-1", kind: "log", payload: "yes" })
    })

    expect(result.current.logs).toEqual(["yes"])
  })

  it("unsubscribes on unmount mid-install (no leaked listener)", async () => {
    const { result, unmount } = renderHook(() => useAgentInstallStream())
    await act(async () => {
      await result.current.start("task-1")
    })
    expect(unsubs).toHaveLength(1)
    expect(unsubs[0]).not.toHaveBeenCalled()

    unmount()

    expect(unsubs[0]).toHaveBeenCalledTimes(1)
  })

  it("does not leak a subscription that resolves after unmount", async () => {
    let resolveSub: ((unsub: () => void) => void) | null = null
    mockSubscribe.mockImplementationOnce(
      () =>
        new Promise<() => void>((resolve) => {
          resolveSub = resolve
        })
    )

    const { result, unmount } = renderHook(() => useAgentInstallStream())
    let startPromise: Promise<void> | undefined
    act(() => {
      startPromise = result.current.start("task-1")
    })
    // Tearing down while subscribe() is still pending must not leak it.
    unmount()

    const lateUnsub = vi.fn()
    await act(async () => {
      resolveSub?.(lateUnsub)
      await startPromise
    })

    expect(lateUnsub).toHaveBeenCalledTimes(1)
  })
})
