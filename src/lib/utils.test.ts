import type { MouseEvent } from "react"
import { describe, expect, it, vi } from "vitest"

import { handleMiddleClickClose, randomUUID } from "./utils"

function mouseEventWithButton(button: number) {
  const preventDefault = vi.fn()
  const event = { button, preventDefault } as unknown as MouseEvent
  return { event, preventDefault }
}

describe("handleMiddleClickClose", () => {
  it("closes and prevents default on middle-click (button 1)", () => {
    const onClose = vi.fn()
    const { event, preventDefault } = mouseEventWithButton(1)

    handleMiddleClickClose(event, onClose)

    expect(onClose).toHaveBeenCalledTimes(1)
    expect(preventDefault).toHaveBeenCalledTimes(1)
  })

  it("ignores left-click (button 0)", () => {
    const onClose = vi.fn()
    const { event, preventDefault } = mouseEventWithButton(0)

    handleMiddleClickClose(event, onClose)

    expect(onClose).not.toHaveBeenCalled()
    expect(preventDefault).not.toHaveBeenCalled()
  })

  it("ignores right-click (button 2) so the context menu still opens", () => {
    const onClose = vi.fn()
    const { event, preventDefault } = mouseEventWithButton(2)

    handleMiddleClickClose(event, onClose)

    expect(onClose).not.toHaveBeenCalled()
    expect(preventDefault).not.toHaveBeenCalled()
  })
})

const V4_UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

describe("randomUUID", () => {
  it("returns a valid v4 UUID when crypto.randomUUID is available", () => {
    expect(randomUUID()).toMatch(V4_UUID_RE)
  })

  it("falls back to crypto.getRandomValues in non-secure contexts", () => {
    // Simulate the server-over-HTTP-on-LAN case: a non-secure context where
    // crypto.randomUUID is undefined but crypto.getRandomValues still works.
    const realCrypto = globalThis.crypto
    vi.stubGlobal("crypto", {
      getRandomValues: (arr: Uint8Array) => realCrypto.getRandomValues(arr),
    })
    try {
      const id = randomUUID()
      expect(id).toMatch(V4_UUID_RE)
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
