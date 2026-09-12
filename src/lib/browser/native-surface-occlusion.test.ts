import { act, renderHook } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  acquireNativeSurfaceOcclusion,
  acquireNativeSurfaceOcclusionFor,
  isNativeSurfaceOccluded,
  nativeSurfaceOcclusionHolders,
  resetNativeSurfaceOcclusionForTests,
  subscribeNativeSurfaceOcclusion,
  useFallbackOverlayOpen,
  useNativeSurfaceOccluded,
  useNativeSurfaceOcclusion,
  useNativeSurfaceOcclusionRef,
} from "./native-surface-occlusion"

describe("native surface occlusion leases", () => {
  beforeEach(() => resetNativeSurfaceOcclusionForTests())
  afterEach(() => {
    resetNativeSurfaceOcclusionForTests()
    document.body.innerHTML = ""
  })

  it("counts holders and notifies only on the 0↔1 edges", () => {
    const listener = vi.fn()
    subscribeNativeSurfaceOcclusion(listener)
    const releaseA = acquireNativeSurfaceOcclusion("dialog")
    const releaseB = acquireNativeSurfaceOcclusion("dialog")
    const releaseC = acquireNativeSurfaceOcclusion("menu")
    expect(isNativeSurfaceOccluded()).toBe(true)
    expect(nativeSurfaceOcclusionHolders()).toEqual({ dialog: 2, menu: 1 })
    expect(listener).toHaveBeenCalledTimes(1)
    releaseA()
    releaseA() // double release is a no-op
    expect(isNativeSurfaceOccluded()).toBe(true)
    expect(listener).toHaveBeenCalledTimes(1)
    releaseB()
    releaseC()
    expect(isNativeSurfaceOccluded()).toBe(false)
    expect(nativeSurfaceOcclusionHolders()).toEqual({})
    expect(listener).toHaveBeenCalledTimes(2)
  })

  it("useNativeSurfaceOcclusion holds a lease while mounted and active", () => {
    const { result } = renderHook(() => useNativeSurfaceOccluded())
    const holder = renderHook(
      ({ active }: { active: boolean }) =>
        useNativeSurfaceOcclusion("drawer", active),
      { initialProps: { active: true } }
    )
    expect(result.current).toBe(true)
    act(() => holder.rerender({ active: false }))
    expect(result.current).toBe(false)
    act(() => holder.rerender({ active: true }))
    expect(result.current).toBe(true)
    act(() => holder.unmount())
    expect(result.current).toBe(false)
  })

  it("the fallback detector sees lease-less open dialogs and menus", async () => {
    const { result } = renderHook(() => useFallbackOverlayOpen())
    expect(result.current).toBe(false)
    const dialog = document.createElement("div")
    dialog.setAttribute("role", "dialog")
    dialog.setAttribute("data-state", "open")
    await act(async () => {
      document.body.appendChild(dialog)
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(result.current).toBe(true)
    await act(async () => {
      dialog.setAttribute("data-state", "closed")
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(result.current).toBe(false)
  })
})

describe("DOM-bound leases", () => {
  it("useNativeSurfaceOcclusionRef holds a lease only while attached to a node", () => {
    resetNativeSurfaceOcclusionForTests()
    const { result } = renderHook(() => useNativeSurfaceOcclusionRef("menu"))
    expect(isNativeSurfaceOccluded()).toBe(false)
    const node = document.createElement("div")
    act(() => result.current(node))
    expect(isNativeSurfaceOccluded()).toBe(true)
    act(() => result.current(node)) // re-attach is not a second lease
    expect(nativeSurfaceOcclusionHolders()).toEqual({ menu: 1 })
    act(() => result.current(null))
    expect(isNativeSurfaceOccluded()).toBe(false)
  })

  it("acquireNativeSurfaceOcclusionFor is a no-op when inactive", () => {
    resetNativeSurfaceOcclusionForTests()
    const release = acquireNativeSurfaceOcclusionFor("drawer", false)
    expect(isNativeSurfaceOccluded()).toBe(false)
    release()
    const release2 = acquireNativeSurfaceOcclusionFor("drawer", true)
    expect(isNativeSurfaceOccluded()).toBe(true)
    release2()
    expect(isNativeSurfaceOccluded()).toBe(false)
  })
})
