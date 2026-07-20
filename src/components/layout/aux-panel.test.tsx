import { describe, expect, it } from "vitest"

import { resolveAuxTabView, shouldCollapseAuxTabs } from "./aux-panel"

describe("resolveAuxTabView", () => {
  it("shows all tabs and keeps the selection in a folder workspace", () => {
    expect(resolveAuxTabView("file_tree", 1, false)).toEqual({
      showFolderTabs: true,
      effectiveTab: "file_tree",
    })
    expect(resolveAuxTabView("session_details", 1, false)).toEqual({
      showFolderTabs: true,
      effectiveTab: "session_details",
    })
  })

  it("collapses to Session Details in chat mode, even with a bound folder", () => {
    // A bound chat conversation has a (hidden) folder id but is chat mode.
    expect(resolveAuxTabView("git_log", 1, true)).toEqual({
      showFolderTabs: false,
      effectiveTab: "session_details",
    })
  })

  it("collapses to Session Details when no folder is open", () => {
    expect(resolveAuxTabView("changes", null, false)).toEqual({
      showFolderTabs: false,
      effectiveTab: "session_details",
    })
  })

  it("overrides a stale folder-tab selection with a valid shown tab", () => {
    // Stored selection is a folder tab but folder tabs are hidden: the shown
    // tab must fall back so Radix never points at a triggerless value.
    expect(resolveAuxTabView("file_tree", null, false).effectiveTab).toBe(
      "session_details"
    )
  })
})

describe("shouldCollapseAuxTabs", () => {
  // rightReserve mirrors rightChromeReserve(): 116 on macOS/web (chrome only),
  // 254 on desktop Windows/Linux (chrome 116 + native caption 138).
  const MAC_WEB_RESERVE = 116
  const WIN_LINUX_RESERVE = 254

  it("keeps the segmented control when the panel has room", () => {
    // 320 − 12 gutter − 116 = 192 available ≥ 130 control + 12 gap.
    expect(shouldCollapseAuxTabs(320, MAC_WEB_RESERVE)).toBe(false)
  })

  it("collapses once the panel is too narrow for the control", () => {
    // 220 − 12 − 116 = 92 available < 142.
    expect(shouldCollapseAuxTabs(220, MAC_WEB_RESERVE)).toBe(true)
  })

  it("collapses at the default width when the win/linux caption is reserved", () => {
    // 320 − 12 − 254 = 54 available < 142: the wider reservation forces a
    // collapse the mac/web layout wouldn't at the same width.
    expect(shouldCollapseAuxTabs(320, WIN_LINUX_RESERVE)).toBe(true)
    expect(shouldCollapseAuxTabs(320, MAC_WEB_RESERVE)).toBe(false)
  })

  it("never collapses before the panel width is measured", () => {
    // First paint reports 0 until the ResizeObserver fires; stay expanded.
    expect(shouldCollapseAuxTabs(0, MAC_WEB_RESERVE)).toBe(false)
  })
})
