"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Reorder } from "motion/react"
import { FileText, GitCompare, Maximize2, Minimize2, X } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  useWorkspaceActions,
  useWorkspaceFileTabs,
  useWorkspaceView,
} from "@/contexts/workspace-context"
import type { FileWorkspaceTab } from "@/contexts/workspace-context"
import { useIsCoarsePointer } from "@/hooks/use-is-coarse-pointer"
import { useLongPressDrag } from "@/hooks/use-long-press-drag"
import { cn, handleMiddleClickClose } from "@/lib/utils"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"

// Rendered only inside the desktop file-column title strip (embedded). The old
// standalone mobile variant is gone — mobile shows the FileWorkspaceHeader
// (folder › file breadcrumb) instead and opens files from the file tree.
export function FileWorkspaceTabBar() {
  const t = useTranslations("Folder.fileWorkspace")
  const { mode, filesMaximized } = useWorkspaceView()
  const { fileTabs, activeFileTabId } = useWorkspaceFileTabs()
  const {
    switchFileTab,
    closeFileTab,
    closeOtherFileTabs,
    closeAllFileTabs,
    reorderFileTabs,
    toggleFilesMaximized,
  } = useWorkspaceActions()
  const scrollRef = useRef<HTMLDivElement>(null)
  const isCoarsePointer = useIsCoarsePointer()
  const [touchSortingTabId, setTouchSortingTabId] = useState<string | null>(
    null
  )

  useEffect(() => {
    if (!activeFileTabId || !scrollRef.current) return
    const el = scrollRef.current.querySelector(
      `[data-file-tab-id="${activeFileTabId}"]`
    )
    el?.scrollIntoView({ block: "nearest", inline: "nearest" })
  }, [activeFileTabId])

  const handleReorder = useCallback(
    (nextTabs: FileWorkspaceTab[]) => {
      if (isCoarsePointer && !touchSortingTabId) return
      reorderFileTabs(nextTabs)
    },
    [isCoarsePointer, reorderFileTabs, touchSortingTabId]
  )

  const handleTouchSortingEnd = useCallback(
    () => setTouchSortingTabId(null),
    []
  )

  const activeFileIndex = fileTabs.findIndex(
    (tab) => tab.id === activeFileTabId
  )

  if (fileTabs.length === 0) return null

  return (
    <div className="flex h-full w-full min-w-0 items-stretch">
      <Reorder.Group
        as="div"
        ref={scrollRef}
        role="tablist"
        axis="x"
        values={fileTabs}
        onReorder={handleReorder}
        // Tabs shrink browser-style and sit flush (`gap-0`) so their hairline
        // separators read as dividers (see FileWorkspaceTabItem); no scrollbar
        // (`overflow-hidden` still scrolls programmatically) and no bottom padding
        // so they reach the strip's bottom and the active (white) tab merges into
        // the file detail header below.
        className="pt-1.5 px-2 min-w-0 flex h-full items-stretch gap-0 overflow-hidden"
      >
        {fileTabs.map((tab, index) => (
          <FileWorkspaceTabItem
            key={tab.id}
            tab={tab}
            active={tab.id === activeFileTabId}
            adjacentActive={
              activeFileIndex < 0
                ? undefined
                : index === activeFileIndex - 1
                  ? "before"
                  : index === activeFileIndex + 1
                    ? "after"
                    : undefined
            }
            embedded
            closeLabel={t("closeFileTab")}
            closeText={t("close")}
            closeOthersText={t("closeOthers")}
            closeAllText={t("closeAll")}
            isCoarsePointer={isCoarsePointer}
            isTouchSorting={touchSortingTabId === tab.id}
            onSwitch={switchFileTab}
            onClose={closeFileTab}
            onCloseOthers={closeOtherFileTabs}
            onCloseAll={closeAllFileTabs}
            onTouchSortingStart={setTouchSortingTabId}
            onTouchSortingEnd={handleTouchSortingEnd}
          />
        ))}
      </Reorder.Group>
      {/* Trailing area: a drag spacer fills the leftover panel width (window-drag
          region) and, in fusion, a maximize/restore button sits flush right (it
          used to live in the file detail header). Wrapped in one `flex-1` box so
          the workspace-bg bottom hairline (ws-strip-line) runs unbroken under
          both. NO `min-w-0`: the wrapper's min-content (the spacer's `min-w-10` +
          the shrink-0 button) is its floor, so under many-tab overflow the group
          shrinks to reserve them instead of the wrapper collapsing to 0. */}
      <div className="flex h-full flex-1 items-stretch ws-strip-line">
        {/* Drag spacer, floored at `min-w-10` (40px): even when many tabs overflow
            and squeeze this region, a grabbable window-drag gap always remains
            between the last tab and the maximize button. */}
        <div data-tauri-drag-region className="h-full min-w-10 flex-1" />
        {mode === "fusion" && (
          <button
            type="button"
            onClick={toggleFilesMaximized}
            className={cn(
              // Ghost-style icon button following the file tabs (mirrors the
              // conversation new-tab button): `h-7 self-center` centers it on the
              // h-10 strip midline; hover darkens past the `bg-muted` strip.
              "mr-1.5 flex h-7 w-7 shrink-0 items-center justify-center self-center rounded-md text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground",
              filesMaximized && "text-primary"
            )}
            aria-label={filesMaximized ? t("restore") : t("maximize")}
            aria-pressed={filesMaximized}
            title={filesMaximized ? t("restore") : t("maximize")}
          >
            {filesMaximized ? (
              <Minimize2 className="h-4 w-4" />
            ) : (
              <Maximize2 className="h-4 w-4" />
            )}
          </button>
        )}
      </div>
    </div>
  )
}

interface FileWorkspaceTabItemProps {
  tab: FileWorkspaceTab
  active: boolean
  /** Whether this tab immediately precedes/follows the active tab — lets the
   *  neighbour inset its workspace-bg baseline so the active tab's transparent
   *  reverse-corner foot (which flares over it) leaves no stray line. */
  adjacentActive?: "before" | "after"
  embedded: boolean
  closeLabel: string
  closeText: string
  closeOthersText: string
  closeAllText: string
  isCoarsePointer: boolean
  isTouchSorting: boolean
  onSwitch: (tabId: string) => void
  onClose: (tabId: string) => void
  onCloseOthers: (tabId: string) => void
  onCloseAll: () => void
  onTouchSortingStart: (tabId: string) => void
  onTouchSortingEnd: () => void
}

const FileWorkspaceTabItem = memo(function FileWorkspaceTabItem({
  tab,
  active,
  adjacentActive,
  embedded,
  closeLabel,
  closeText,
  closeOthersText,
  closeAllText,
  isCoarsePointer,
  isTouchSorting,
  onSwitch,
  onClose,
  onCloseOthers,
  onCloseAll,
  onTouchSortingStart,
  onTouchSortingEnd,
}: FileWorkspaceTabItemProps) {
  const isDiff = tab.kind === "diff" || tab.kind === "rich-diff"
  const isDirty = tab.kind === "file" && Boolean(tab.isDirty)

  const handleLongPressStart = useCallback(
    () => onTouchSortingStart(tab.id),
    [onTouchSortingStart, tab.id]
  )

  const { dragControls, gestureHandlers } = useLongPressDrag({
    enabled: isCoarsePointer,
    onStart: handleLongPressStart,
    onEnd: onTouchSortingEnd,
  })

  const handleSwitch = useCallback(() => {
    onSwitch(tab.id)
  }, [onSwitch, tab.id])

  const whileDrag = useMemo(() => ({ scale: 1.03 }), [])

  return (
    <Reorder.Item
      as="div"
      value={tab}
      data-file-tab-id={tab.id}
      drag="x"
      dragControls={dragControls}
      dragListener={!isCoarsePointer}
      whileDrag={whileDrag}
      {...gestureHandlers}
      data-tab-item
      data-active={embedded && active ? "true" : undefined}
      data-adjacent-active={embedded ? adjacentActive : undefined}
      className={cn(
        "cursor-grab active:cursor-grabbing",
        // Embedded (browser-style): every tab is EQUAL width (`basis-48` = 12rem,
        // `grow-0` so they don't stretch to fill) — a long filename and a short one
        // read uniform instead of one wide / one narrow. They still `shrink`
        // together (down to `min-w-0`, the label fades) once the row fills; above
        // that the fixed basis keeps them equal. Leftover row stays a window-drag
        // region. `browser-tab-item` draws the left-edge hairline separator
        // (globals.css) as a 1px divider at each shared edge — tabs sit flush (no
        // gutter) so the line is the only separation, and the inner row owns its
        // own `overflow-hidden`. The active tab is raised (`z-10`) so its
        // reverse-corner seat is never covered by a hovered neighbour's flare.
        // Standalone: rounded pill, intrinsic width (scroll).
        embedded
          ? "browser-tab-item min-w-0 grow-0 shrink basis-48 data-[active=true]:z-10"
          : "rounded-full shrink-0",
        isTouchSorting && "z-50 opacity-90 shadow-md ring-1 ring-primary/25"
      )}
    >
      {/* Reverse (concave) bottom corners — the browser-tab seat (globals.css).
          Absolute + decorative, so it never affects layout. Rendered for every
          embedded tab; CSS reveals it when the tab is active or hovered. */}
      {embedded && <span aria-hidden className="browser-tab-seat" />}
      <ContextMenu>
        <ContextMenuTrigger asChild disabled={isTouchSorting}>
          <div
            role="tab"
            aria-selected={active}
            onClick={handleSwitch}
            onMouseDown={(event) =>
              handleMiddleClickClose(event, () => onClose(tab.id))
            }
            className={cn(
              "group/filetab relative flex items-center h-full gap-1.5 text-xs",
              "cursor-pointer select-none transition-colors",
              embedded
                ? [
                    // Browser-style tab: white (bg-background) active fill,
                    // rounded top, reaching the strip's bottom so it merges into
                    // the file detail header below. With a workspace background
                    // image on, the whole strip + all tabs go transparent (reveal
                    // the image); a hairline bottom border (ws-strip-line) runs
                    // under every non-active region while the active tab omits it
                    // and instead is outlined by a top+side "archway" (the
                    // browser-tab-item `::after`, globals.css) whose reverse-corner
                    // feet (browser-tab-seat) drop back onto that line — a gap the
                    // border detours around, not a filled
                    // box. `overflow-hidden` clips the
                    // shrunken row. `pb-1.5` balances the group's `pt-1.5` gap so
                    // the content centers on the h-10 strip midline, not 3px low
                    // in the shorter tab box (fill still reaches the bottom).
                    // `browser-tab-content` anchors the label's state-driven fade
                    // (globals.css `.browser-tab-content:hover` widens the mask so
                    // the close button never covers the title).
                    "browser-tab-content w-full min-w-0 overflow-hidden rounded-t-lg px-2 pb-1.5",
                    active
                      ? "bg-background ws-transparent-bg text-foreground"
                      : "isolate browser-tab-hover text-muted-foreground hover:text-foreground ws-strip-line",
                  ]
                : [
                    "shrink-0 rounded-full px-3 hover:bg-primary/8",
                    active
                      ? "bg-primary/10 text-foreground"
                      : "text-muted-foreground",
                  ]
            )}
            title={tab.description ?? tab.title}
          >
            {isDiff ? (
              <GitCompare className="h-3.5 w-3.5" />
            ) : (
              <FileText className="h-3.5 w-3.5" />
            )}
            <span
              className={cn(
                // Embedded: grow + shrink as the tab tightens, but instead of an
                // ellipsis the overflowing title fades out on the right
                // (browser-tab-label mask), dissolving toward the close button so
                // the full width is used. Standalone: ellipsis cap in the scroll row.
                embedded
                  ? "min-w-0 flex-1 overflow-hidden whitespace-nowrap browser-tab-label"
                  : "truncate max-w-[180px]"
              )}
            >
              {tab.title}
              {isDirty ? " *" : ""}
            </span>
            <button
              type="button"
              className={cn(
                "rounded-md hover:bg-foreground/10",
                // Embedded: an absolute overlay pinned to the right edge, so it
                // claims no row space — the label runs the full width and fades
                // under it (browser-tab-label) instead of stopping short of an
                // always-reserved in-flow button. Centered via `top-0 bottom-1.5
                // my-auto` (no transform → crisp on WebKit; `bottom-1.5` mirrors
                // the content's `pb-1.5`). Pointer events are gated off while
                // hidden so it can't eat clicks. Standalone: an in-flow chip.
                embedded
                  ? "absolute right-2 top-0 bottom-1.5 my-auto flex h-4 w-4 items-center justify-center"
                  : "shrink-0 p-0.5",
                active
                  ? "opacity-100"
                  : embedded
                    ? "opacity-0 pointer-events-none group-hover/filetab:opacity-100 group-hover/filetab:pointer-events-auto"
                    : "opacity-0 group-hover/filetab:opacity-100"
              )}
              onClick={(event) => {
                event.stopPropagation()
                onClose(tab.id)
              }}
              aria-label={closeLabel}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={() => onClose(tab.id)}>
            {closeText}
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => onCloseOthers(tab.id)}>
            {closeOthersText}
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={onCloseAll}>
            {closeAllText}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    </Reorder.Item>
  )
})
