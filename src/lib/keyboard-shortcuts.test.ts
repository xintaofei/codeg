import { describe, expect, it } from "vitest"

import {
  DEFAULT_SHORTCUTS,
  SHORTCUT_DEFINITIONS,
  matchShortcutEvent,
} from "./keyboard-shortcuts"

function keyEvent(
  key: string,
  modifiers: Partial<
    Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "altKey" | "shiftKey">
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
