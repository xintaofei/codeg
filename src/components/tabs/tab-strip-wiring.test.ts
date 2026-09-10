import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import { describe, it, expect } from "vitest"

const tabBar = readFileSync(
  resolve(process.cwd(), "src/components/tabs/tab-bar.tsx"),
  "utf8"
)
const tabItem = readFileSync(
  resolve(process.cwd(), "src/components/tabs/tab-item.tsx"),
  "utf8"
)

/**
 * Wiring the store's guarantees cannot enforce on its own.
 *
 * `moveTabToGroup` / `splitTab` reject a draft's cross-group move (unit-tested
 * in tab-context.test.tsx), but a rejected drop still looks broken — the tab
 * lifts, the ghost appears, and nothing happens. The strip must not offer the
 * affordance in the first place.
 */
describe("tab strip draft gating", () => {
  it("keeps drafts out of every cross-group affordance", () => {
    expect(tabBar).toContain("const isDraft = tab.conversationId == null")
    expect(tabBar).toContain("canSplitMove={canSplitMove && !isDraft}")
    expect(tabBar).toContain("canMoveToGroup={!isDraft}")
    // Both drag callbacks are withheld for drafts, so a draft drag can never
    // register a drop target (no ghost, no highlight, no move).
    expect(tabBar).toMatch(/onTabDrag=\{\s*crossDragEnabled && !isDraft/)
    expect(tabBar).toMatch(/onTabDragEnd=\{\s*crossDragEnabled && !isDraft/)
    // A draft has no conversation for a window to show, so the strip withholds
    // the handler entirely rather than offering an inert menu item.
    expect(tabBar).toContain(
      "onOpenInNewWindow={isDraft ? undefined : handleOpenInNewWindow}"
    )
  })

  it("gates only the move items, so a draft keeps the group-management menu", () => {
    expect(tabItem).toContain("{canMoveToGroup &&")
    // `moveTargets` still drives the Unsplit All gate — passing an empty array
    // for drafts (instead of this flag) would have hidden that item too.
    expect(tabItem).toContain("{moveTargets.length >= 2 && (")
  })
})

describe("open in new window wiring", () => {
  it("passes the tab's own identity and title to the window opener", () => {
    // The window is keyed by conversation id, and the title only names the
    // taskbar entry — everything else the view reads back for itself.
    expect(tabBar).toMatch(
      /openConversationWindow\(\s*\{\s*folderId: tab\.folderId,\s*conversationId: tab\.conversationId,\s*agentType: tab\.agentType,\s*\},\s*tab\.title\s*\)/
    )
  })

  it("leaves the tab where it is", () => {
    // `opened_tabs` is the workspace's own list and the new window is a
    // detached view that never writes to it, so there is no close here — a
    // removal would have to be pushed to every client and would take the
    // conversation out of the workspace for good.
    expect(tabBar).not.toMatch(/handleOpenInNewWindow[\s\S]{0,400}closeTab\(/)
  })
})

describe("tab drag selection guard wiring", () => {
  it("suppresses text selection for EVERY tab drag, composed with the long-press handlers", () => {
    // Held on drag start / released on drag end + unmount, so within-group
    // sorting and the unsplit strip are covered too (the ghost only exists for
    // cross-group drags).
    expect(tabItem).toContain("acquireDragSelectionGuard")
    expect(tabItem).toContain("releaseDragSelectionGuard")
    expect(tabItem).toContain("useEffect(() => releaseGuard, [releaseGuard])")
    // The long-press hook ships its own onDragStart/onDragEnd (coarse-pointer
    // cleanup + post-drag click suppression); ours must call through, never
    // replace them via spread order.
    expect(tabItem).toContain("onDragStart: longPressDragStart")
    expect(tabItem).toContain("onDragEnd: longPressDragEnd")
    expect(tabItem).toMatch(
      /handleDragStart[\s\S]{0,160}longPressDragStart\(\)/
    )
    expect(tabItem).toMatch(/handleDragEnd[\s\S]{0,200}longPressDragEnd\(\)/)
  })
})
