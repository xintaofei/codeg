import { describe, expect, it } from "vitest"

import {
  DEFAULT_SHORTCUTS,
  NUMBERED_TAB_ACTION_IDS,
  SHORTCUT_DEFINITIONS,
  matchShortcutEvent,
  numberedTabIndexFromEvent,
  pickNumberedTabId,
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

describe("numbered tab shortcuts", () => {
  it("registers Ctrl/Cmd+1 through 9 as switch_tab_N", () => {
    const ids = SHORTCUT_DEFINITIONS.map((definition) => definition.id)
    for (const [index, actionId] of NUMBERED_TAB_ACTION_IDS.entries()) {
      expect(ids).toContain(actionId)
      expect(DEFAULT_SHORTCUTS[actionId]).toBe(`mod+${index + 1}`)
    }
  })

  it("maps Ctrl+1 to the first tab and ignores a missing ninth tab", () => {
    const tabs = ["conv-a", "conv-b", "conv-c"]
    expect(pickNumberedTabId(tabs, 0)).toBe("conv-a")
    expect(pickNumberedTabId(tabs, 2)).toBe("conv-c")
    expect(pickNumberedTabId(tabs, 8)).toBeNull()
    expect(pickNumberedTabId([], 0)).toBeNull()
  })

  it("resolves Ctrl+2 against the default numbered-tab bindings", () => {
    expect(
      numberedTabIndexFromEvent(
        keyEvent("2", { ctrlKey: true }),
        DEFAULT_SHORTCUTS
      )
    ).toBe(1)
    expect(
      numberedTabIndexFromEvent(keyEvent("2"), DEFAULT_SHORTCUTS)
    ).toBeNull()
    expect(
      numberedTabIndexFromEvent(
        keyEvent("Tab", { ctrlKey: true }),
        DEFAULT_SHORTCUTS
      )
    ).toBeNull()
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
