"use client"

import { memo, useCallback, useRef } from "react"
import { Reorder } from "motion/react"
import { X } from "lucide-react"
import { useTranslations } from "next-intl"
import { cn } from "@/lib/utils"
import { STATUS_COLORS } from "@/lib/types"
import type { ConversationStatus } from "@/lib/types"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import type { TabItem as TabItemData } from "@/contexts/tab-context"

interface TabItemProps {
  tab: TabItemData
  isActive: boolean
  isTileMode: boolean
  onSwitch: (tabId: string) => void
  onClose: (tabId: string) => void
  onCloseOthers: (tabId: string) => void
  onCloseAll: () => void
  onPin: (tabId: string) => void
  onToggleTile: () => void
}

export const TabItem = memo(function TabItem({
  tab,
  isActive,
  isTileMode,
  onSwitch,
  onClose,
  onCloseOthers,
  onCloseAll,
  onPin,
  onToggleTile,
}: TabItemProps) {
  const t = useTranslations("Folder.tabs")
  const isDragging = useRef(false)
  const itemRef = useRef<HTMLDivElement>(null)

  const clearResidualStyles = useCallback(() => {
    const el = itemRef.current
    if (!el) return
    el.style.transform = ""
    el.style.zIndex = ""
    el.style.position = ""
    el.style.userSelect = ""
  }, [])

  const handleClick = useCallback(() => {
    if (isDragging.current) return
    onSwitch(tab.id)
  }, [onSwitch, tab.id])

  const handleDoubleClick = useCallback(() => {
    if (isDragging.current) return
    if (!tab.isPinned) {
      onPin(tab.id)
    }
  }, [onPin, tab.id, tab.isPinned])

  const handleClose = useCallback(() => {
    onClose(tab.id)
  }, [onClose, tab.id])

  const handleCloseOthers = useCallback(() => {
    onCloseOthers(tab.id)
  }, [onCloseOthers, tab.id])

  return (
    <Reorder.Item
      ref={itemRef}
      as="div"
      value={tab}
      data-tab-id={tab.id}
      onDragStart={() => {
        isDragging.current = true
      }}
      onDragEnd={() => {
        setTimeout(() => {
          isDragging.current = false
          clearResidualStyles()
        }, 200)
      }}
      onLayoutAnimationComplete={clearResidualStyles}
      className="shrink-0 rounded-full cursor-grab active:cursor-grabbing active:opacity-90 active:shadow-md active:z-50"
    >
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            role="tab"
            aria-selected={isActive}
            onClick={handleClick}
            onDoubleClick={handleDoubleClick}
            className={cn(
              "group/tab relative flex items-center h-full gap-1.5 px-3 text-xs rounded-full",
              "cursor-pointer select-none shrink-0",
              "hover:bg-primary/8 transition-colors",
              isActive
                ? "bg-primary/10 text-foreground"
                : "text-muted-foreground"
            )}
          >
            <span
              className={cn(
                "w-2 h-2 rounded-full shrink-0",
                tab.status
                  ? STATUS_COLORS[tab.status as ConversationStatus]
                  : "bg-gray-400 dark:bg-gray-500"
              )}
            />
            <span
              className={cn(
                "truncate max-w-[140px]",
                !tab.isPinned && "[font-style:oblique]"
              )}
              title={tab.title}
            >
              {tab.title}
            </span>
            <button
              type="button"
              className={cn(
                "rounded-full p-0.5 hover:bg-muted",
                isActive
                  ? "opacity-100"
                  : "opacity-0 group-hover/tab:opacity-100"
              )}
              onClick={(event) => {
                event.stopPropagation()
                handleClose()
              }}
              aria-label={t("closeConversationTab")}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={handleClose}>{t("close")}</ContextMenuItem>
          <ContextMenuItem onSelect={handleCloseOthers}>
            {t("closeOthers")}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={onToggleTile}>
            {isTileMode ? t("untileDisplay") : t("tileDisplay")}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={onCloseAll}>
            {t("closeAll")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    </Reorder.Item>
  )
})
