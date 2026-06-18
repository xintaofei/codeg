"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { Columns3, PanelTop } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import type { TabItem as TabItemData } from "@/contexts/tab-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useIsCoarsePointer } from "@/hooks/use-is-coarse-pointer"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { Button } from "@/components/ui/button"
import { TabItem } from "./tab-item"
import { cn } from "@/lib/utils"

export function TabBar() {
  const t = useTranslations("Folder.tabs")
  const {
    tabs,
    activeTabId,
    isTileMode,
    switchTab,
    closeTab,
    closeOtherTabs,
    closeAllTabs,
    pinTab,
    toggleTileMode,
    reorderTabs,
  } = useTabContext()
  const { allFolders, branches } = useAppWorkspace()
  const { mode, activePane, filesMaximized } = useWorkspaceContext()

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
  const viewToggleLabel = isTileMode
    ? t("showCurrentTabOnly")
    : t("tileAllTabs")

  if (tabs.length === 0) return null

  return (
    <div className="flex h-10 items-stretch border-b border-border">
      <Reorder.Group
        as="div"
        ref={scrollRef}
        role="tablist"
        axis="x"
        values={tabs}
        onReorder={handleReorder}
        onWheel={handleWheel}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          "min-w-0 flex-1 pt-1.5 px-1.5 flex items-stretch gap-1.5",
          "overflow-x-scroll",
          isHovered
            ? [
                "pb-0.5",
                "[&::-webkit-scrollbar]:h-1",
                "[&::-webkit-scrollbar-track]:bg-transparent",
                "[&::-webkit-scrollbar-thumb]:rounded-full",
                "[&::-webkit-scrollbar-thumb]:bg-border",
              ]
            : ["pb-1.5", "[&::-webkit-scrollbar]:h-0"]
        )}
      >
        {tabs.map((tab) => {
          const folderInfo = folderIndex.get(tab.folderId)
          return (
            <TabItem
              key={tab.id}
              tab={tab}
              isActive={tab.id === activeTabId}
              isTileMode={isTileMode}
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
      <div className="flex shrink-0 items-center border-l border-border/60 px-1.5">
        <Button
          type="button"
          variant="ghost"
          size="xs"
          aria-pressed={isTileMode}
          aria-label={viewToggleLabel}
          title={viewToggleLabel}
          onClick={toggleTileMode}
          className={cn(
            "h-7 rounded-full px-2 text-xs",
            isTileMode && "bg-primary/10 text-foreground"
          )}
        >
          {isTileMode ? (
            <PanelTop className="h-3.5 w-3.5" />
          ) : (
            <Columns3 className="h-3.5 w-3.5" />
          )}
          <span>{t("viewToggle")}</span>
        </Button>
      </div>
    </div>
  )
}
