"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { Reorder } from "motion/react"
import {
  Code,
  Eye,
  ExternalLink,
  FileText,
  GitCompare,
  Maximize2,
  Minimize2,
  X,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { openPath } from "@/lib/platform"
import { isHtmlPreviewable } from "@/lib/language-detect"
import {
  useWorkspaceActions,
  useWorkspaceFileTabs,
  useWorkspaceView,
} from "@/contexts/workspace-context"
import type { FileWorkspaceTab } from "@/contexts/workspace-context"
import { useIsCoarsePointer } from "@/hooks/use-is-coarse-pointer"
import { useIsMobile } from "@/hooks/use-mobile"
import { useLongPressDrag } from "@/hooks/use-long-press-drag"
import { useShortcutSettings } from "@/hooks/use-shortcut-settings"
import { matchShortcutEvent } from "@/lib/keyboard-shortcuts"
import { cn, handleMiddleClickClose } from "@/lib/utils"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"

export function FileWorkspaceTabBar({
  embedded = false,
}: {
  embedded?: boolean
} = {}) {
  const t = useTranslations("Folder.fileWorkspace")
  const { mode, activePane, filesMaximized } = useWorkspaceView()
  const { fileTabs, activeFileTabId, previewFileTabIds } =
    useWorkspaceFileTabs()
  const {
    switchFileTab,
    closeFileTab,
    closeOtherFileTabs,
    closeAllFileTabs,
    reorderFileTabs,
    toggleFileTabPreview,
    toggleFilesMaximized,
  } = useWorkspaceActions()
  const { shortcuts } = useShortcutSettings()
  const scrollRef = useRef<HTMLDivElement>(null)
  const isCoarsePointer = useIsCoarsePointer()
  const isMobile = useIsMobile()
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
    if (!activeFileTabId || !scrollRef.current) return
    const el = scrollRef.current.querySelector(
      `[data-file-tab-id="${activeFileTabId}"]`
    )
    el?.scrollIntoView({ block: "nearest", inline: "nearest" })
  }, [activeFileTabId])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // While maximized only the files pane is interactive, so route shortcuts
      // here regardless of the user's last-clicked pane.
      const shouldHandleShortcut =
        mode === "fusion" && (activePane === "files" || filesMaximized)
      if (!shouldHandleShortcut) return
      if (matchShortcutEvent(event, shortcuts.close_all_file_tabs)) {
        event.preventDefault()
        closeAllFileTabs()
        return
      }
      if (!matchShortcutEvent(event, shortcuts.close_current_tab)) return

      if (!activeFileTabId) return
      event.preventDefault()
      closeFileTab(activeFileTabId)
    }

    window.addEventListener("keydown", onKeyDown)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
    }
  }, [
    activeFileTabId,
    closeAllFileTabs,
    closeFileTab,
    mode,
    activePane,
    filesMaximized,
    shortcuts.close_all_file_tabs,
    shortcuts.close_current_tab,
  ])

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

  const activeTab = fileTabs.find((tab) => tab.id === activeFileTabId)
  const activeFileIndex = fileTabs.findIndex(
    (tab) => tab.id === activeFileTabId
  )
  const canPreview =
    activeTab?.kind === "file" &&
    (activeTab.language === "markdown" || isHtmlPreviewable(activeTab.path))
  const canOpenInBrowser =
    activeTab?.kind === "file" && isHtmlPreviewable(activeTab.path)
  const isPreviewActive =
    canPreview && activeFileTabId
      ? previewFileTabIds.has(activeFileTabId)
      : false

  // Embedded in the title bar: fill its height and let the bar own the bottom
  // border. Standalone (mobile panel row): keep the h-10 row + border.
  const rowHeight = embedded ? "h-full" : "h-10"
  const rowBorder = embedded ? "" : "border-b border-border"

  if (fileTabs.length === 0) {
    // In the title bar an empty file workspace shows nothing (only the
    // conversation tabs remain); the standalone panel row keeps its label.
    if (embedded) return null
    return (
      <div className="h-10 px-3 flex items-center border-b border-border text-xs text-muted-foreground">
        {t("files")}
      </div>
    )
  }

  return (
    <div
      className={cn(
        "flex items-stretch",
        // Embedded: fill the resizable panel that bounds our width in the title
        // bar. Standalone: intrinsic size in the mobile panel row.
        embedded && "h-full w-full min-w-0"
      )}
    >
      <Reorder.Group
        as="div"
        ref={scrollRef}
        role="tablist"
        axis="x"
        values={fileTabs}
        onReorder={handleReorder}
        // Embedded tabs shrink to fit (no overflow), so wheel-to-scroll is both
        // unnecessary and wrong — `overflow-hidden` still scrolls programmatically.
        onWheel={embedded ? undefined : handleWheel}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => setIsHovered(false)}
        className={cn(
          "pt-1.5 px-1.5 min-w-0 flex items-stretch",
          // Standalone row fills its container so the trailing action buttons
          // sit flush right; embedded sizes to content so the wrapper's drag
          // spacer claims the leftover row.
          !embedded && "flex-1",
          rowHeight,
          rowBorder,
          // Embedded: no scrollbar — tabs shrink browser-style and sit flush
          // (`gap-0`) so their hairline separators read as dividers (see
          // FileWorkspaceTabItem `embedded`); no bottom padding so they reach
          // the strip's bottom and the active (white) tab merges into the file
          // detail header below. Standalone: horizontal scroll with a hover
          // scrollbar + the original inter-tab gap (mobile panel row).
          embedded
            ? "gap-0 overflow-hidden px-2"
            : [
                "gap-1.5 overflow-x-scroll",
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
            embedded={embedded}
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
      {/* Title-bar trailing area (desktop title bar): a drag spacer fills the
          leftover panel width (window-drag region) and, in fusion, a
          maximize/restore button sits flush right (it used to live in the file
          detail header). Wrapped in one `flex-1` box so the workspace-bg bottom
          hairline (ws-strip-line) runs unbroken under both — the short
          `self-center h-7` button can't carry the line itself. NO `min-w-0`: the
          wrapper's min-content (the shrink-0 button) is its floor, so under
          many-tab overflow the group shrinks to reserve the button width instead
          of the wrapper collapsing to 0 and clipping it. Off (no bg image):
          ws-strip-line is inert. */}
      {embedded && (
        <div className="flex h-full flex-1 items-stretch ws-strip-line">
          <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
          {mode === "fusion" && (
            <button
              type="button"
              onClick={toggleFilesMaximized}
              className={cn(
                // Ghost-style icon button following the file tabs (mirrors the
                // conversation new-tab button): `h-7 self-center` centers it on
                // the h-10 strip midline (matching the tab content); hover darkens
                // past the `bg-muted` strip (ghost's own `bg-muted` hover would be
                // invisible on it).
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
      )}
      {/* Trailing file-action buttons render only in the standalone (mobile
          panel) row. In the desktop title bar (embedded) they live in the file
          detail header instead (FileWorkspaceHeader). */}
      {!embedded && canPreview && activeFileTabId && (
        <button
          type="button"
          onClick={() => toggleFileTabPreview(activeFileTabId)}
          className={cn(
            "shrink-0 flex items-center justify-center w-10 hover:bg-primary/8 transition-colors",
            rowBorder,
            isPreviewActive && "text-primary"
          )}
          aria-label={isPreviewActive ? t("editSource") : t("preview")}
          title={isPreviewActive ? t("editSource") : t("preview")}
        >
          {isPreviewActive ? (
            <Code className="h-4 w-4" />
          ) : (
            <Eye className="h-4 w-4" />
          )}
        </button>
      )}
      {!embedded && canOpenInBrowser && activeTab?.path && (
        <button
          type="button"
          onClick={() => {
            // File tab paths are absolute — hand the path straight to the OS.
            openPath(activeTab.path as string).catch(() => {})
          }}
          className={cn(
            "shrink-0 flex items-center justify-center w-10 hover:bg-primary/8 transition-colors",
            rowBorder
          )}
          aria-label={t("preview")}
          title={t("preview")}
        >
          <ExternalLink className="h-4 w-4" />
        </button>
      )}
      {!embedded && !isMobile && mode === "fusion" && (
        <button
          type="button"
          onClick={toggleFilesMaximized}
          className={cn(
            "shrink-0 flex items-center justify-center w-10 hover:bg-primary/8 transition-colors",
            rowBorder,
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
        // Embedded (browser-style): each tab sizes to its content (`basis-auto`)
        // up to `max-w-[15rem]` (leftover row stays a window-drag region);
        // `grow-0` keeps them from stretching to fill, and they still `shrink`
        // together (down to `min-w-0`, the label truncates) once full.
        // `browser-tab-item` draws the left-edge hairline separator (globals.css)
        // as a 1px divider at each shared edge — tabs sit flush (no gutter) so
        // the line is the only separation, and the inner row owns its own
        // `overflow-hidden`. The active tab is raised (`z-10`) so its
        // reverse-corner seat is never covered by a hovered neighbour's flare.
        // Standalone: rounded pill, intrinsic width (scroll).
        embedded
          ? "browser-tab-item min-w-0 grow-0 shrink basis-auto max-w-[15rem] data-[active=true]:z-10"
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
