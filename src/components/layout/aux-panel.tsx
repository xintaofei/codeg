"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  Folder,
  FolderPen,
  GitCommit,
  ReceiptText,
  type LucideIcon,
} from "lucide-react"
import { useTranslations } from "next-intl"
import {
  useAuxPanelContext,
  type AuxPanelTab,
} from "@/contexts/aux-panel-context"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useIsMobile } from "@/hooks/use-mobile"
import { usePlatform } from "@/hooks/use-platform"
import { useZoomLevel } from "@/hooks/use-appearance"
import { isDesktop } from "@/lib/platform"
import { rightChromeReserve } from "@/lib/window-chrome"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { SessionDetailsTab } from "./aux-panel-session-details-tab"
import { FileTreeTab } from "./aux-panel-file-tree-tab"
import { GitChangesTab } from "./aux-panel-git-changes-tab"
import { GitLogTab } from "./aux-panel-git-log-tab"

const LAZY_TABS: AuxPanelTab[] = ["file_tree", "changes", "git_log"]

// Visible order + icon for every aux tab. Both the desktop segmented control
// and the collapsed picker map over this, so the two surfaces can never drift.
const TAB_ORDER: AuxPanelTab[] = [
  "session_details",
  "file_tree",
  "changes",
  "git_log",
]
const TAB_ICONS: Record<AuxPanelTab, LucideIcon> = {
  session_details: ReceiptText,
  file_tree: Folder,
  changes: FolderPen,
  git_log: GitCommit,
}
// The three folder-scoped tabs share one label namespace (Folder.auxPanel.tabs);
// session details resolves from its own (Folder.sessionDetails.menuLabel). The
// value type is the literal key union so next-intl's typed `t()` accepts it.
const FOLDER_TAB_LABEL_KEY: Record<
  Exclude<AuxPanelTab, "session_details">,
  "files" | "changes" | "commits"
> = {
  file_tree: "files",
  changes: "changes",
  git_log: "commits",
}

// The desktop segmented control needs ~130px (4 icon triggers + gaps + track
// padding). It's pinned to the strip's LEFT while the fixed window-chrome
// overlay (terminal/aux/settings, plus the native caption on Windows/Linux)
// floats over the RIGHT edge. Once the panel is too narrow to seat the control
// left of that reserved region, we swap it for a single icon-button + dropdown.
const SEGMENTED_TABS_WIDTH = 130
const TAB_STRIP_GUTTER = 12 // pl-3
const TAB_STRIP_GAP = 12 // breathing room before the chrome overlay

/**
 * Whether the top tab strip should collapse into a single dropdown picker.
 *
 * `panelWidth` is the aux panel's measured width; `rightReserve` is the fixed
 * width the window-chrome overlay claims on the right (platform-dependent). A
 * zero/unknown width (first paint, before the ResizeObserver fires) never
 * collapses, so the segmented control stays the default until measured. Pure +
 * exported for unit tests.
 */
export function shouldCollapseAuxTabs(
  panelWidth: number,
  rightReserve: number
): boolean {
  if (panelWidth <= 0) return false
  const available = panelWidth - TAB_STRIP_GUTTER - rightReserve
  return available < SEGMENTED_TABS_WIDTH + TAB_STRIP_GAP
}

/**
 * Decide which aux-panel tabs are available and which to actually show.
 *
 * The folder-scoped tabs (files/changes/commits) only make sense with a real
 * folder workspace open, so chat sessions and the folderless state collapse to
 * just the Session Details tab. `effectiveTab` keeps the rendered selection
 * valid even when the stored `activeTab` is a now-hidden folder tab, avoiding a
 * one-frame flash before the reconciling effect corrects the stored value.
 */
export function resolveAuxTabView(
  activeTab: AuxPanelTab,
  activeFolderId: number | null,
  isChatMode: boolean
): { showFolderTabs: boolean; effectiveTab: AuxPanelTab } {
  const showFolderTabs = activeFolderId != null && !isChatMode
  return {
    showFolderTabs,
    effectiveTab: showFolderTabs ? activeTab : "session_details",
  }
}

export function AuxPanel() {
  const t = useTranslations("Folder.auxPanel.tabs")
  const tDetails = useTranslations("Folder.sessionDetails")
  const { isOpen, width, activeTab, setActiveTab } = useAuxPanelContext()
  const { activeFolderId } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const isMobile = useIsMobile()
  const { isWindows, isLinux } = usePlatform()
  const { zoomLevel } = useZoomLevel()
  const [mountedTabs, setMountedTabs] = useState<Set<AuxPanelTab>>(
    () => new Set(LAZY_TABS.filter((tab) => tab === activeTab))
  )

  // Measure the panel's real rendered width. The context `width` is the user's
  // requested size, which the shell scales DOWN when the window is too narrow
  // to honor both side panels — so it over-reports in exactly the cramped case
  // that matters here. The observed width is ground truth; context width only
  // seeds the first paint before the observer fires.
  const asideRef = useRef<HTMLElement | null>(null)
  const [measuredWidth, setMeasuredWidth] = useState(0)
  useEffect(() => {
    const el = asideRef.current
    if (!el) return
    const update = (next: number) =>
      setMeasuredWidth((prev) => (Math.abs(prev - next) < 1 ? prev : next))
    update(el.clientWidth)
    const observer = new ResizeObserver((entries) =>
      update(entries[0]?.contentRect.width ?? el.clientWidth)
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [isOpen])

  const { showFolderTabs, effectiveTab } = resolveAuxTabView(
    activeTab,
    activeFolderId,
    isChatMode
  )

  // Ensure the shown tab is mounted (covers both user clicks and programmatic changes)
  if (
    isOpen &&
    LAZY_TABS.includes(effectiveTab) &&
    !mountedTabs.has(effectiveTab)
  ) {
    setMountedTabs((prev) => new Set(prev).add(effectiveTab))
  }

  // Reconcile the stored selection when folder tabs disappear (e.g. entering a
  // chat session), so other consumers of `activeTab` stay in sync with what's
  // shown. Done in an effect — never a render-time setState on the provider.
  useEffect(() => {
    if (!showFolderTabs && activeTab !== "session_details") {
      setActiveTab("session_details")
    }
  }, [showFolderTabs, activeTab, setActiveTab])

  const handleTabValueChange = useCallback(
    (value: string) => {
      setActiveTab(value as AuxPanelTab)
    },
    [setActiveTab]
  )

  // The window-chrome overlay claims a fixed strip on the panel's right edge;
  // on desktop Windows/Linux the native caption buttons sit beyond it. The
  // segmented control has to fit LEFT of all that — otherwise collapse it into
  // a dropdown. Only relevant to the desktop layout (mobile is a full-width
  // Sheet), and only when there's more than the lone Session Details tab.
  const winLinuxControls = isDesktop() && (isWindows || isLinux)
  const rightReserve = rightChromeReserve(winLinuxControls, zoomLevel)
  const collapsed =
    !isMobile &&
    showFolderTabs &&
    shouldCollapseAuxTabs(
      measuredWidth > 0 ? measuredWidth : width,
      rightReserve
    )

  const tabLabel = useCallback(
    (tab: AuxPanelTab) =>
      tab === "session_details"
        ? tDetails("menuLabel")
        : t(FOLDER_TAB_LABEL_KEY[tab]),
    [t, tDetails]
  )

  // Shared across the mobile underline row and the desktop segmented control.
  // `compact` overrides the base full-height, equal-flex trigger into a short,
  // content-width pill for the segmented look; mobile keeps the base styling.
  const renderTabTriggers = (compact: boolean) => {
    const triggerClassName = compact
      ? "h-6 flex-none rounded-md px-2"
      : undefined
    return TAB_ORDER.filter(
      (tab) => tab === "session_details" || showFolderTabs
    ).map((tab) => {
      const Icon = TAB_ICONS[tab]
      const label = tabLabel(tab)
      return (
        <TabsTrigger
          key={tab}
          value={tab}
          title={label}
          aria-label={label}
          className={triggerClassName}
        >
          <Icon className="h-3.5 w-3.5" />
        </TabsTrigger>
      )
    })
  }

  // Collapsed stand-in for the segmented control on a too-narrow panel: a single
  // recessed square showing the active tab's icon that opens a radio menu of all
  // tabs. Mirrors the segmented track's look so the strip reads consistently.
  // Pinned to the strip's LEFT, so it stays clear of the RIGHT window-chrome
  // overlay (incl. the Windows/Linux native caption) at any usable width.
  const renderCollapsedPicker = () => {
    const ActiveIcon = TAB_ICONS[effectiveTab]
    const activeLabel = tabLabel(effectiveTab)
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            title={activeLabel}
            aria-label={activeLabel}
            className="h-7 w-7 rounded-lg bg-foreground/[0.06] text-foreground/70 hover:bg-foreground/10 hover:text-foreground"
          >
            <ActiveIcon className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuRadioGroup
            value={effectiveTab}
            onValueChange={handleTabValueChange}
          >
            {TAB_ORDER.map((tab) => {
              const Icon = TAB_ICONS[tab]
              return (
                <DropdownMenuRadioItem key={tab} value={tab}>
                  <Icon className="h-4 w-4" />
                  {tabLabel(tab)}
                </DropdownMenuRadioItem>
              )
            })}
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  if (!isOpen) return null

  return (
    // Desktop: background matches the middle workspace (bg-background), not the
    // darker sidebar shade, so the right column reads as one surface with it.
    // Mobile (Sheet) is unchanged — keep the sidebar shade.
    <aside
      ref={asideRef}
      className={cn(
        "group/aux-panel flex h-full min-h-0 flex-col overflow-hidden text-sidebar-foreground select-none",
        // 桌面态背景交给 workspace 的 ws-surface wrapper，让背景图透出（未启用时
        // wrapper 等价 bg-background，零回归）；移动态是抽屉，保持不透明。
        isMobile ? "bg-sidebar" : ""
      )}
    >
      <Tabs
        value={effectiveTab}
        onValueChange={handleTabValueChange}
        className="flex h-full min-h-0 flex-col gap-0"
      >
        {isMobile ? (
          // Mobile (Sheet): unchanged — full-width underline tabs + a divider.
          <TabsList
            variant="line"
            className="h-10 w-full shrink-0 justify-start border-b border-border ws-chrome-border px-3 group-data-horizontal/tabs:h-10"
          >
            {renderTabTriggers(false)}
            {/* Trailing drag region lets the empty part of the tab row move
                the window. */}
            <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
          </TabsList>
        ) : (
          // Desktop: a compact segmented control pinned top-LEFT of the h-10
          // strip. It shares that row with the fixed top-right window-chrome
          // overlay (terminal / aux / settings), which floats over the trailing
          // drag region — the tabs sit left, the buttons float right. At
          // comfortable widths they clear each other; when the panel is too
          // narrow for the control to stay left of that overlay, it collapses
          // into a single dropdown picker (see `collapsed`). The strip is
          // always h-10 (reserving the overlay's height); when Session Details
          // is the only tab (chat / folderless) the control is `hidden`
          // (display:none) — that drops the lone trigger out of the tab order
          // (unlike `sr-only`, which stays keyboard
          // focusable and would trap Tab on an invisible control) while the
          // TabsContent's aria-labelledby still resolves the panel's name from
          // the directly-referenced hidden trigger, so it stays labelled without
          // showing a pointless single-tab control.
          <div className="flex h-10 shrink-0 items-center gap-2 bg-muted ws-transparent-bg ws-strip-line pl-3 pr-2">
            {/* Off-image `bg-muted` matches the conversation/file strips +
                bottom StatusBar. With a workspace background image on, the
                composition-family pair `ws-transparent-bg ws-strip-line` makes
                this top strip go transparent (revealing the real background
                like the column body / conversation + file tab strips do,
                instead of the old frosted `ws-surface-muted`) and draws a
                hairline bottom border to separate it from the tab content —
                both are `[data-workspace-bg="on"]`-gated, so off-image is
                unchanged. The segmented track then needs a recessed groove
                (`bg-foreground/[0.06]`) instead of the old `bg-muted/60`, which
                would vanish against the muted strip; the active trigger
                (bg-background) still reads as a raised white pill. */}
            {collapsed && renderCollapsedPicker()}
            {/* When collapsed we keep the TabsList mounted but `hidden`
                (display:none): its triggers stay in the DOM so each
                TabsContent's aria-labelledby still resolves the panel name,
                while dropping out of the tab order (unlike sr-only). The
                dropdown above is the visible switcher. */}
            <TabsList
              variant="default"
              className={cn(
                "h-7 gap-0.5 rounded-lg bg-foreground/[0.06] p-0.5 group-data-horizontal/tabs:h-7",
                (!showFolderTabs || collapsed) && "hidden"
              )}
            >
              {renderTabTriggers(true)}
            </TabsList>
            {/* Empty row remainder (under the floating overlay) stays a
                window-drag region. */}
            <div data-tauri-drag-region className="h-full min-w-0 flex-1" />
          </div>
        )}

        <TabsContent
          value="session_details"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          <SessionDetailsTab />
        </TabsContent>
        <TabsContent
          value="file_tree"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("file_tree") ? <FileTreeTab /> : null}
        </TabsContent>
        <TabsContent
          value="changes"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("changes") ? <GitChangesTab /> : null}
        </TabsContent>
        <TabsContent
          value="git_log"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("git_log") ? <GitLogTab /> : null}
        </TabsContent>
      </Tabs>
    </aside>
  )
}
