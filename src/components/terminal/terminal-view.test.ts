import { describe, expect, it } from "vitest"
import { isTerminalCopyShortcut } from "@/lib/terminal/shortcuts"

type ShortcutEvent = Parameters<typeof isTerminalCopyShortcut>[0]

function event(overrides: Partial<ShortcutEvent> = {}): ShortcutEvent {
  return {
    code: "KeyC",
    altKey: false,
    metaKey: false,
    ctrlKey: true,
    shiftKey: true,
    ...overrides,
  }
}

describe("isTerminalCopyShortcut", () => {
  it("reserves only Ctrl+Shift+C outside macOS", () => {
    expect(isTerminalCopyShortcut(event(), false)).toBe(true)
    expect(isTerminalCopyShortcut(event(), true)).toBe(false)
    expect(isTerminalCopyShortcut(event({ shiftKey: false }), false)).toBe(
      false
    )
    expect(isTerminalCopyShortcut(event({ altKey: true }), false)).toBe(false)
    expect(isTerminalCopyShortcut(event({ metaKey: true }), false)).toBe(false)
    expect(isTerminalCopyShortcut(event({ code: "KeyV" }), false)).toBe(false)
  })
})
