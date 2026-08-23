import { describe, expect, it } from "vitest"

import { browserSurfaceKeyEventParams } from "../chrome-backend.js"

describe("browserSurfaceKeyEventParams", () => {
  it("includes Windows virtual key codes for Ctrl+A", () => {
    expect(
      browserSurfaceKeyEventParams({
        kind: "key",
        event: "down",
        key: "a",
        code: "KeyA",
        modifiers: 2,
      })
    ).toEqual({
      type: "keyDown",
      key: "a",
      code: "KeyA",
      text: undefined,
      windowsVirtualKeyCode: 65,
      nativeVirtualKeyCode: 65,
      modifiers: 2,
    })
  })

  it("preserves text for ordinary character input", () => {
    expect(
      browserSurfaceKeyEventParams({
        kind: "key",
        event: "down",
        key: "a",
        code: "KeyA",
        text: "a",
      })
    ).toMatchObject({
      type: "keyDown",
      key: "a",
      code: "KeyA",
      text: "a",
      windowsVirtualKeyCode: 65,
      nativeVirtualKeyCode: 65,
    })
  })
})
