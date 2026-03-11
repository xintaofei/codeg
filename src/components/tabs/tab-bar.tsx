"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { useTabContext } from "@/contexts/tab-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { TabItem } from "./tab-item"
import { cn } from "@/lib/utils"

export function TabBar() {
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
  const { mode, activePane } = useWorkspaceContext()
  const { shortcuts } = useShortcutSettings()
  const scrollRef = useRef<HTMLDivElement>(null)
  const [isHovered, setIsHovered] = useState(false)

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
        (mode === "fusion" && activePane === "conversation")
      if (!shouldHandleShortcut) return
      if (!matchShortcutEvent(event, shortcuts.close_current_tab)) return
      if (!activeTabId) return

      event.preventDefault()
      closeTab(activeTabId)
    }

    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [activePane, activeTabId, closeTab, mode, shortcuts.close_current_tab])

  if (tabs.length === 0) return null

  return (
    <Reorder.Group
      as="div"
      ref={scrollRef}
      role="tablist"
      axis="x"
      values={tabs}
      onReorder={reorderTabs}
      onWheel={handleWheel}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      className={cn(
        "h-10 pt-1.5 px-1.5 flex items-stretch gap-1.5 border-b border-border",
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
      {tabs.map((tab) => (
        <TabItem
          key={tab.id}
          tab={tab}
          isActive={tab.id === activeTabId}
          isTileMode={isTileMode}
          onSwitch={switchTab}
          onClose={closeTab}
          onCloseOthers={closeOtherTabs}
          onCloseAll={closeAllTabs}
          onPin={pinTab}
          onToggleTile={toggleTileMode}
        />
      ))}
    </Reorder.Group>
  )
}
