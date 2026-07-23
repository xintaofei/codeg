"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { SquarePen } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import type { TabItem as TabItemData } from "@/contexts/tab-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useIsCoarsePointer } from "@/hooks/use-is-coarse-pointer"
import { TabItem } from "./tab-item"

// Rendered only inside the desktop conversation-column title strip (embedded).
// The old standalone mobile variant is gone — mobile shows the conversation
// detail header instead and navigates tabs from the sidebar.
export function TabBar() {
  const t = useTranslations("Folder.conversationCard")
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  const isTileMode = useTabStore((s) => s.isTileMode)
  const {
    switchTab,
    closeTab,
    closeOtherTabs,
    closeAllTabs,
    pinTab,
    toggleTileMode,
    reorderTabs,
    openNewConversationTab,
    openChatModeTab,
  } = useTabActions()
  const allFolders = useAppWorkspaceStore((s) => s.allFolders)
  const branches = useAppWorkspaceStore((s) => s.branches)
  const { activeFolder } = useActiveFolder()
  const { openConversations } = useWorkbenchRoute()

  // New-conversation affordance at the end of the tab strip. Mirrors the
  // sidebar's "New chat": return to the conversation workspace, then open a
  // draft in the active folder — or a folderless chat when nothing is open, so
  // the button is never a dead end.
  const handleNewConversation = useCallback(() => {
    openConversations()
    if (!activeFolder) {
      openChatModeTab()
      return
    }
    openNewConversationTab(activeFolder.id, activeFolder.path)
  }, [activeFolder, openChatModeTab, openConversations, openNewConversationTab])

  const folderIndex = useMemo(() => {
    const map = new Map<number, { name: string }>()
    for (const f of allFolders) map.set(f.id, { name: f.name })
    return map
  }, [allFolders])

  const scrollRef = useRef<HTMLDivElement>(null)
  const isCoarsePointer = useIsCoarsePointer()
  const [touchSortingTabId, setTouchSortingTabId] = useState<string | null>(
    null
  )

  useEffect(() => {
    if (!activeTabId || !scrollRef.current) return
    const el = scrollRef.current.querySelector(`[data-tab-id="${activeTabId}"]`)
    el?.scrollIntoView({ block: "nearest", inline: "nearest" })
  }, [activeTabId])

  const handleReorder = useCallback(
    (nextTabs: TabItemData[]) => {
      if (isCoarsePointer && !touchSortingTabId) return
      reorderTabs(nextTabs)
    },
    [isCoarsePointer, reorderTabs, touchSortingTabId]
  )

  const handleTouchSortingEnd = useCallback(
    () => setTouchSortingTabId(null),
    []
  )

  if (tabs.length === 0) return null

  const activeIndex = tabs.findIndex((tab) => tab.id === activeTabId)
  // When the LAST tab is active, the trailing new-conversation wrapper is its
  // right neighbour — it needs the same baseline inset a tab neighbour gets
  // (`data-adjacent-active`), so the active tab's right reverse-corner foot
  // doesn't leave a stray line poking out from under it (globals.css).
  const lastTabActive = activeIndex >= 0 && activeIndex === tabs.length - 1

  return (
    <Reorder.Group
      as="div"
      ref={scrollRef}
      role="tablist"
      axis="x"
      values={tabs}
      onReorder={handleReorder}
      // Fills the title-bar strip and shrinks browser-style to share the row (see
      // TabItem): flush (`gap-0`) so hairline separators read as dividers, no
      // scrollbar (`overflow-hidden` still scrolls programmatically), and no
      // bottom border so the active (white) tab merges into the detail header
      // below. It hosts the trailing new-conversation button + drag spacer as its
      // own last children so the tabs, button, and spacer size in ONE flex line:
      // the tabs keep their equal `basis-48` width until the row fills, then
      // shrink together, and the button always hugs the last tab. `pl-2` only
      // (NOT `px-2`): the first tab keeps its left gutter for the first-child
      // seam-patch, but there's NO right padding so the trailing wrapper's
      // `ws-strip-line` reaches the group's right edge and the bottom hairline
      // stays continuous into the right reserve.
      className="pt-1.5 flex h-full min-w-0 flex-1 items-stretch gap-0 overflow-hidden pl-2"
    >
      {tabs.map((tab, index) => {
        const folderInfo = folderIndex.get(tab.folderId)
        // Neighbours of the active tab inset their workspace-bg baseline so the
        // active tab's transparent reverse-corner foot (which flares over them)
        // doesn't leave a stray line under it (globals.css `data-adjacent-active`).
        const adjacentActive =
          activeIndex < 0
            ? undefined
            : index === activeIndex - 1
              ? "before"
              : index === activeIndex + 1
                ? "after"
                : undefined
        return (
          <TabItem
            key={tab.id}
            tab={tab}
            isActive={tab.id === activeTabId}
            isTileMode={isTileMode}
            embedded
            adjacentActive={adjacentActive}
            folderName={folderInfo?.name ?? null}
            folderBranch={branches.get(tab.folderId) ?? null}
            onSwitch={switchTab}
            onClose={closeTab}
            onCloseOthers={closeOtherTabs}
            onCloseAll={closeAllTabs}
            onPin={pinTab}
            onToggleTile={toggleTileMode}
            isCoarsePointer={isCoarsePointer}
            isTouchSorting={touchSortingTabId === tab.id}
            onTouchSortingStart={setTouchSortingTabId}
            onTouchSortingEnd={handleTouchSortingEnd}
          />
        )
      })}
      {/* The new-conversation button + drag spacer are the Reorder.Group's own
          trailing children, so they share the tabs' flex line — the button hugs
          the last tab and the spacer fills the leftover row as a window-drag
          region. They are not Reorder.Items, so dragging a tab only ever permutes
          the tabs. Wrapped in one `flex-1` `ws-strip-line` box so the
          workspace-bg bottom hairline runs unbroken under both — the short
          `self-start h-7` button can't carry the line itself. NO `min-w-0`: its
          min-content (the shrink-0 button + the spacer's `min-w-10`) is its floor,
          so under many-tab overflow the tabs shrink to reserve it instead of it
          collapsing to 0 and clipping the button. */}
      <div
        // `relative` anchors two decorative pseudo-elements: the
        // `data-adjacent-active` inset baseline (globals.css `.ws-strip-line`
        // `::after`) used when the last tab is active, and the `tab-strip-tail`
        // `::before` vertical separator shown between the last NON-active tab and
        // the new-conversation button. Inter-tab separators sit on each tab's
        // LEFT edge (`.browser-tab-item::before`), so the last tab's RIGHT edge —
        // where this flush-pinned button begins — otherwise has none. Only the
        // conversation strip carries `tab-strip-tail`; the file strip floats its
        // trailing button far-right past a drag spacer, so it stays divider-free.
        data-adjacent-active={lastTabActive ? "after" : undefined}
        className="tab-strip-tail relative flex h-full flex-1 items-stretch ws-strip-line"
      >
        <button
          type="button"
          onClick={handleNewConversation}
          // Ghost-style CIRCULAR icon button, evenly inset from the strip's three
          // visible edges so its round hover fill never touches the last tab.
          // `self-start` seats it against the group's `pt-1.5` top rather than
          // centering in the pt-shortened trailing box: with `h-7` on the `h-10`
          // strip that yields an equal 6px top and 6px bottom gap, so its center
          // still lands on the strip midline (matching the tab content). `ml-1.5`
          // adds a matching 6px LEFT gap from the last tab's edge. The hover uses
          // the chrome-standard adaptive tint (`bg-foreground/10`, matching the
          // bottom branch/command blocks) plus `backdrop-blur-sm`: over the fully
          // transparent strip (workspace bg image on) the fill reads as frosted
          // glass rather than a muddy patch, and the tint is clearly visible in
          // both light and dark themes (unlike the old near-white `bg-accent/40`).
          className="ml-1.5 mr-0.5 flex h-7 w-7 shrink-0 items-center justify-center self-start rounded-full text-muted-foreground backdrop-blur-sm transition-colors hover:bg-foreground/10 hover:text-foreground"
          aria-label={t("newConversation")}
          title={t("newConversation")}
        >
          <SquarePen className="h-3.5 w-3.5" />
        </button>
        {/* Drag spacer, floored at `min-w-10` (40px) instead of `min-w-0`: even
            when many tabs overflow and squeeze this region, a grabbable
            window-drag gap always remains to the RIGHT of the new-conversation
            button, so the button never reaches the strip's right edge and the
            packed title bar stays draggable. */}
        <div data-tauri-drag-region className="h-full min-w-10 flex-1" />
      </div>
    </Reorder.Group>
  )
}
