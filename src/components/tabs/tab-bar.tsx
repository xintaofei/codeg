"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { SquarePen } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useTabActions, useTabStore } from "@/contexts/tab-context"
import type { TabItem as TabItemData } from "@/contexts/tab-context"
import { useWorkspaceView } from "@/contexts/workspace-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useIsCoarsePointer } from "@/hooks/use-is-coarse-pointer"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { TabItem } from "./tab-item"
import { cn } from "@/lib/utils"

export function TabBar({ embedded = false }: { embedded?: boolean } = {}) {
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
  const { mode, activePane, filesMaximized } = useWorkspaceView()

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

  const { shortcuts } = useShortcutSettings()
  const scrollRef = useRef<HTMLDivElement>(null)
  const isCoarsePointer = useIsCoarsePointer()
  const [isHovered, setIsHovered] = useState(false)
  const [touchSortingTabId, setTouchSortingTabId] = useState<string | null>(
    null
  )

  const handleWheel = useCallback((e: React.WheelEvent<HTMLDivElement>) => {
    if (e.deltaY !== 0 && scrollRef.current) {
      e.preventDefault()
      scrollRef.current.scrollLeft += e.deltaY
    }
  }, [])

  useEffect(() => {
    if (!activeTabId || !scrollRef.current) return
    const el = scrollRef.current.querySelector(`[data-tab-id="${activeTabId}"]`)
    el?.scrollIntoView({ block: "nearest", inline: "nearest" })
  }, [activeTabId])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const shouldHandleShortcut =
        mode === "conversation" ||
        (mode === "fusion" && activePane === "conversation" && !filesMaximized)
      if (!shouldHandleShortcut) return
      const isNextTab = matchShortcutEvent(event, shortcuts.next_tab)
      const isPrevTab = matchShortcutEvent(event, shortcuts.prev_tab)
      if (isNextTab || isPrevTab) {
        if (tabs.length < 2 || !activeTabId) return
        const currentIndex = tabs.findIndex((tab) => tab.id === activeTabId)
        if (currentIndex === -1) return

        event.preventDefault()
        const offset = isNextTab ? 1 : -1
        const nextIndex = (currentIndex + offset + tabs.length) % tabs.length
        switchTab(tabs[nextIndex].id)
        return
      }

      if (!matchShortcutEvent(event, shortcuts.close_current_tab)) return
      if (!activeTabId) return

      event.preventDefault()
      closeTab(activeTabId)
    }

    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [
    activePane,
    activeTabId,
    closeTab,
    filesMaximized,
    mode,
    shortcuts.close_current_tab,
    shortcuts.next_tab,
    shortcuts.prev_tab,
    switchTab,
    tabs,
  ])

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

  const group = (
    <Reorder.Group
      as="div"
      ref={scrollRef}
      role="tablist"
      axis="x"
      values={tabs}
      onReorder={handleReorder}
      // Embedded tabs shrink to fit (no overflow), so wheel-to-scroll is both
      // unnecessary and wrong — `overflow-hidden` still scrolls programmatically.
      onWheel={embedded ? undefined : handleWheel}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      className={cn(
        "pt-1.5 px-1.5 flex items-stretch",
        // Embedded in the title bar: fill its height, no scrollbar — the tabs
        // shrink browser-style to share the row (see TabItem `embedded`), sit
        // flush (`gap-0`) so their hairline separators read as dividers, and the
        // strip owns no bottom border. No bottom padding, so the tabs reach the
        // strip's bottom edge and the active (white) one merges into the detail
        // header below. It sizes to its content (no `w-full`) so the wrapper's
        // trailing drag spacer claims the leftover row.
        // Standalone (mobile panel row): keep the h-10 row + border + horizontal
        // scroll with a hover scrollbar and the original inter-tab gap.
        embedded
          ? "h-full min-w-0 gap-0 overflow-hidden px-2"
          : [
              "h-10 gap-1.5 border-b border-border overflow-x-scroll",
              isHovered
                ? [
                    "pb-0.5",
                    "[&::-webkit-scrollbar]:h-1",
                    "[&::-webkit-scrollbar-track]:bg-transparent",
                    "[&::-webkit-scrollbar-thumb]:rounded-full",
                    "[&::-webkit-scrollbar-thumb]:bg-border",
                  ]
                : ["pb-1.5", "[&::-webkit-scrollbar]:h-0"],
            ]
      )}
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
            embedded={embedded}
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
    </Reorder.Group>
  )

  if (!embedded) return group

  // Title-bar strip: the tabs are equal width (fixed basis); a new-conversation
  // button follows the last tab (browser-style), then the trailing spacer fills
  // the leftover row and stays a window-drag region so a lightly-tabbed bar can
  // still be grabbed to move the window.
  return (
    <div className="flex h-full w-full min-w-0 items-stretch">
      {group}
      {/* Trailing area after the last tab: the new-conversation button hugs the
          tabs, then a drag spacer fills the leftover row (window-drag region).
          Wrapped in one `flex-1` box so the workspace-bg bottom hairline
          (ws-strip-line) runs unbroken under both — the short `self-center h-7`
          button can't carry the line at the strip's bottom edge itself. NO
          `min-w-0`: the wrapper's min-content (the shrink-0 button + the spacer's
          `min-w-10`) is its floor, so under many-tab overflow the group shrinks to
          reserve them instead of the wrapper collapsing to 0 and clipping. Off (no
          bg image): ws-strip-line is inert. */}
      <div className="flex h-full flex-1 items-stretch ws-strip-line">
        <button
          type="button"
          onClick={handleNewConversation}
          // Ghost-style icon button hugging the last (equal-width) tab: no left
          // margin, so it sits just past the group's `px-2` — close to the final
          // tab's trailing edge, its gap roughly matching its `self-center h-7`
          // top/bottom inset. `self-center` centers it on the h-10 strip's midline
          // (matching the tab content). Hover darkens past the `bg-muted` strip
          // (ghost's own `bg-muted` hover would be invisible on it).
          className="mr-0.5 flex h-7 w-7 shrink-0 items-center justify-center self-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
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
    </div>
  )
}
