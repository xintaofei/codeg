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
  // rightReserve mirrors captionOverhangPastRail(): 0 on macOS/web (the rail
  // is a beside-column, nothing floats over us), 98 on desktop Windows/Linux
  // (native caption 138 − rail width 40 still overhangs the strip).
  const MAC_WEB_RESERVE = 0
  const WIN_LINUX_RESERVE = 98

  it("keeps the segmented control when the panel has room", () => {
    // 320 − 12 gutter − 0 = 308 available ≥ 130 control + 12 gap.
    expect(shouldCollapseAuxTabs(320, MAC_WEB_RESERVE)).toBe(false)
  })

  it("collapses once the panel is too narrow for the control", () => {
    // 150 − 12 − 0 = 138 available < 142.
    expect(shouldCollapseAuxTabs(150, MAC_WEB_RESERVE)).toBe(true)
  })

  it("collapses at a width the mac/web layout keeps expanded when the caption overhangs", () => {
    // 210 − 12 − 98 = 100 available < 142, while mac/web (reserve 0) has 198
    // and stays expanded at the same width.
    expect(shouldCollapseAuxTabs(210, WIN_LINUX_RESERVE)).toBe(true)
    expect(shouldCollapseAuxTabs(210, MAC_WEB_RESERVE)).toBe(false)
  })

  it("never collapses before the panel width is measured", () => {
    // First paint reports 0 until the ResizeObserver fires; stay expanded.
    expect(shouldCollapseAuxTabs(0, MAC_WEB_RESERVE)).toBe(false)
  })
})
