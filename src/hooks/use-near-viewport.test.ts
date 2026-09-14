import { act, renderHook } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { useNearViewport } from "./use-near-viewport"

let callback: IntersectionObserverCallback | null = null
const observe = vi.fn()
const disconnect = vi.fn()

class FakeIntersectionObserver {
  constructor(next: IntersectionObserverCallback) {
    callback = next
  }
  observe = observe
  disconnect = disconnect
  unobserve = vi.fn()
  takeRecords = vi.fn(() => [])
  root = null
  rootMargin = "1000px 0px"
  thresholds = [0]
}

afterEach(() => {
  vi.unstubAllGlobals()
  observe.mockReset()
  disconnect.mockReset()
  callback = null
})

describe("useNearViewport", () => {
  it("loads immediately where IntersectionObserver is unavailable", () => {
    vi.stubGlobal("IntersectionObserver", undefined)
    const { result } = renderHook(() => useNearViewport<HTMLDivElement>())

    expect(result.current.shouldLoad).toBe(true)
  })

  it("waits until the observed node enters the buffered viewport", () => {
    vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver)
    const { result } = renderHook(() => useNearViewport<HTMLDivElement>())
    const node = document.createElement("div")

    act(() => result.current.ref(node))
    expect(observe).toHaveBeenCalledWith(node)
    expect(result.current.shouldLoad).toBe(false)

    act(() => {
      callback?.(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        {} as IntersectionObserver
      )
    })
    expect(result.current.shouldLoad).toBe(true)
  })
})
