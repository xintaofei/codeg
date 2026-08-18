import { describe, expect, it } from "vitest"

import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_DEFINITIONS,
  formatShortcutLabel,
  matchShortcutEvent,
  normalizeShortcut,
  resolveWindowZoomAction,
  shortcutFromKeyboardEvent,
} from "./keyboard-shortcuts"

function keyEvent(
  key: string,
  modifiers: Partial<
    Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "altKey" | "shiftKey"> & {
      code: string
    }
  > = {}
) {
  return {
    key,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...modifiers,
  }
}

describe("tab cycling shortcuts", () => {
  it("registers next_tab and prev_tab with defaults", () => {
    const ids = SHORTCUT_DEFINITIONS.map((definition) => definition.id)
    expect(ids).toContain("next_tab")
    expect(ids).toContain("prev_tab")
    expect(DEFAULT_SHORTCUTS.next_tab).toBe("mod+tab")
    expect(DEFAULT_SHORTCUTS.prev_tab).toBe("mod+shift+tab")
  })

  it("matches Ctrl+Tab against the next_tab default", () => {
    expect(
      matchShortcutEvent(keyEvent("Tab", { ctrlKey: true }), "mod+tab")
    ).toBe(true)
    expect(matchShortcutEvent(keyEvent("Tab"), "mod+tab")).toBe(false)
  })

  it("matches Ctrl+Shift+Tab against the prev_tab default", () => {
    expect(
      matchShortcutEvent(
        keyEvent("Tab", { ctrlKey: true, shiftKey: true }),
        "mod+shift+tab"
      )
    ).toBe(true)
    // Without Shift it must not match prev_tab, and with Shift it must not
    // match next_tab, so the two bindings stay distinct.
    expect(
      matchShortcutEvent(keyEvent("Tab", { ctrlKey: true }), "mod+shift+tab")
    ).toBe(false)
    expect(
      matchShortcutEvent(
        keyEvent("Tab", { ctrlKey: true, shiftKey: true }),
        "mod+tab"
      )
    ).toBe(false)
  })
})

describe("alt combinations use event.code", () => {
  // macOS 上 ⌥S 报的 event.key 是 "ß"，不是 "s"。不看 event.code 的话，任何
  // 含 alt 的组合在 macOS 上都按不出来 —— 自定义样式的逃生舱正是这样一条。
  it("matches Cmd+Option+Shift+S even though the layout rewrote the key", () => {
    expect(
      matchShortcutEvent(
        keyEvent("ß", {
          metaKey: true,
          altKey: true,
          shiftKey: true,
          code: "KeyS",
        }),
        "mod+alt+shift+s"
      )
    ).toBe(true)
  })

  it("records the physical key rather than the rewritten character", () => {
    expect(
      shortcutFromKeyboardEvent(
        keyEvent("ß", {
          metaKey: true,
          altKey: true,
          shiftKey: true,
          code: "KeyS",
        })
      )
    ).toBe("mod+alt+shift+s")
  })

  it("leaves non-alt shortcuts on event.key", () => {
    // 没有 alt 时不改道：既有快捷键的行为必须逐字节不变。
    expect(
      matchShortcutEvent(
        keyEvent("k", { metaKey: true, code: "KeyK" }),
        "mod+k"
      )
    ).toBe(true)
    expect(
      matchShortcutEvent(
        keyEvent("Dead", { metaKey: true, code: "KeyK" }),
        "mod+k"
      )
    ).toBe(false)
  })

  it("still resolves the escape hatch when the runtime omits event.code", () => {
    expect(
      matchShortcutEvent(
        keyEvent("s", { metaKey: true, altKey: true, shiftKey: true }),
        "mod+alt+shift+s"
      )
    ).toBe(true)
  })
})

const defaultZoom = {
  zoom_in: DEFAULT_SHORTCUTS.zoom_in,
  zoom_out: DEFAULT_SHORTCUTS.zoom_out,
  zoom_reset: DEFAULT_SHORTCUTS.zoom_reset,
}

describe("window zoom shortcuts", () => {
  it("registers zoom_in / zoom_out / zoom_reset defaults", () => {
    const ids = SHORTCUT_DEFINITIONS.map((definition) => definition.id)
    expect(ids).toContain("zoom_in")
    expect(ids).toContain("zoom_out")
    expect(ids).toContain("zoom_reset")
    expect(DEFAULT_SHORTCUTS.zoom_in).toBe("mod+=")
    expect(DEFAULT_SHORTCUTS.zoom_out).toBe("mod+-")
    expect(DEFAULT_SHORTCUTS.zoom_reset).toBe("mod+0")
  })

  it("lets + survive normalize so Ctrl/Cmd Shift = can be recorded", () => {
    expect(normalizeShortcut("mod++")).toBe("mod++")
    expect(normalizeShortcut("mod+shift++")).toBe("mod+shift++")
    expect(
      shortcutFromKeyboardEvent(
        keyEvent("+", { ctrlKey: true, shiftKey: true })
      )
    ).toBe("mod+shift++")
    expect(formatShortcutLabel("mod++", false)).toBe("Ctrl++")
    expect(formatShortcutLabel("mod++", true)).toBe("⌘+")
  })

  it("treats = and + as the same physical key on the bound zoom-in shortcut", () => {
    expect(matchShortcutEvent(keyEvent("=", { ctrlKey: true }), "mod+=")).toBe(
      true
    )
    expect(
      matchShortcutEvent(
        keyEvent("+", { ctrlKey: true, shiftKey: true }),
        "mod+="
      )
    ).toBe(true)
    expect(matchShortcutEvent(keyEvent("k", { ctrlKey: true }), "mod+=")).toBe(
      false
    )
  })

  it("treats - and _ as the same physical key on the bound zoom-out shortcut", () => {
    expect(matchShortcutEvent(keyEvent("-", { ctrlKey: true }), "mod+-")).toBe(
      true
    )
    expect(
      matchShortcutEvent(
        keyEvent("_", { ctrlKey: true, shiftKey: true }),
        "mod+-"
      )
    ).toBe(true)
    expect(matchShortcutEvent(keyEvent("=", { ctrlKey: true }), "mod+-")).toBe(
      false
    )
  })

  it("follows a remapped zoom-in binding and ignores the old default", () => {
    const remapped = {
      ...defaultZoom,
      zoom_in: "mod+shift+z",
    }
    expect(
      resolveWindowZoomAction(
        keyEvent("z", { ctrlKey: true, shiftKey: true }),
        remapped
      )
    ).toBe("in")
    expect(
      resolveWindowZoomAction(keyEvent("=", { ctrlKey: true }), remapped)
    ).toBeNull()
    expect(
      resolveWindowZoomAction(
        keyEvent("+", { ctrlKey: true, shiftKey: true }),
        remapped
      )
    ).toBeNull()
  })

  it("still matches a repeat of the bound zoom key", () => {
    expect(
      resolveWindowZoomAction(keyEvent("=", { ctrlKey: true }), defaultZoom)
    ).toBe("in")
    expect(
      resolveWindowZoomAction(keyEvent("-", { ctrlKey: true }), defaultZoom)
    ).toBe("out")
    expect(
      resolveWindowZoomAction(keyEvent("0", { ctrlKey: true }), defaultZoom)
    ).toBe("reset")
  })

  it("does not treat AZERTY Ctrl+) as zoom out just because code is Minus", () => {
    expect(
      matchShortcutEvent(
        keyEvent(")", { ctrlKey: true, code: "Minus" }),
        "mod+-"
      )
    ).toBe(false)
  })

  it("still matches numpad + / - against the default zoom bindings", () => {
    expect(
      matchShortcutEvent(
        keyEvent("Add", { ctrlKey: true, code: "NumpadAdd" }),
        "mod+="
      )
    ).toBe(true)
    expect(
      matchShortcutEvent(
        keyEvent("Subtract", { ctrlKey: true, code: "NumpadSubtract" }),
        "mod+-"
      )
    ).toBe(true)
  })
})
