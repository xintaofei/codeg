"use client"

import {
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type Ref,
} from "react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { toast } from "sonner"
import { Virtualizer, type VirtualizerHandle } from "virtua"
import {
  Folder,
  Bot,
  Check,
  Download,
  ExternalLink,
  FolderOpen,
  GitBranch,
  ListChecks,
  Loader2,
  Palette,
  Plus,
  Rocket,
  XCircle,
} from "lucide-react"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import { useTaskContext } from "@/contexts/task-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import { useThemeColor, useZoomLevel } from "@/hooks/use-appearance"
import { useSortedAvailableAgents } from "@/hooks/use-sorted-available-agents"
import {
  importLocalConversations,
  openProjectBootWindow,
  updateConversationTitle,
  updateConversationStatus,
  updateFolderColor,
  updateFolderDefaultAgent,
  deleteConversation,
} from "@/lib/api"
import { isDesktop, openFileDialog, revealItemInDir } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import type {
  AgentType,
  ConversationStatus,
  DbConversationSummary,
} from "@/lib/types"
import { AGENT_LABELS } from "@/lib/types"
import {
  loadFolderExpanded,
  saveFolderExpanded,
  type SidebarSortMode,
} from "@/lib/sidebar-view-mode-storage"
import {
  FOLDER_THEME_COLOR_INHERIT,
  THEME_COLOR_PREVIEW,
  THEME_COLORS,
  type FolderThemeColor,
  type ThemeColor,
} from "@/lib/theme-presets"
import { SidebarConversationCard } from "./sidebar-conversation-card"
import {
  applyReorder,
  buildOwnerHeaderIndex,
  buildRows,
  computeStickyState,
  flatIndexOfConversation,
  folderHeaderFlatIndices,
  formatRelative,
  groupByFolderWithReuse,
  headerIndexForFolder,
  nextHeaderAfter,
  pointerYToTargetIndex,
  reuseSelected,
  reuseSet,
  type SidebarRow,
} from "./sidebar-conversation-grouping"
import { ConversationManageDialog } from "./conversation-manage-dialog"
import { CloneDialog } from "@/components/layout/clone-dialog"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
} from "@/components/ui/context-menu"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { cn } from "@/lib/utils"
import { toErrorMessage } from "@/lib/app-error"

// Layout effect on the client (so the sticky overlay is positioned before
// paint) but a no-op-safe passive effect during the static-export prerender.
const useIsomorphicLayoutEffect =
  typeof window !== "undefined" ? useLayoutEffect : useEffect

const THEME_COLOR_SET = new Set<string>(THEME_COLORS)

const LEGACY_FOLDER_COLOR_MAP: Record<string, FolderThemeColor> = {
  foreground: FOLDER_THEME_COLOR_INHERIT,
  "#ef4444": "red",
  "#f97316": "orange",
  "#eab308": "yellow",
  "#84cc16": "green",
  "#22c55e": "green",
  "#06b6d4": "blue",
  "#8b5cf6": "violet",
  "#d946ef": "rose",
  "#ec4899": "rose",
}

function normalizeFolderThemeColor(
  color: string | null | undefined
): FolderThemeColor {
  if (!color) return FOLDER_THEME_COLOR_INHERIT
  const normalized = color.toLowerCase()
  if (normalized === FOLDER_THEME_COLOR_INHERIT) {
    return FOLDER_THEME_COLOR_INHERIT
  }
  if (THEME_COLOR_SET.has(normalized)) return normalized as ThemeColor
  return LEGACY_FOLDER_COLOR_MAP[normalized] ?? FOLDER_THEME_COLOR_INHERIT
}

const FolderHeader = memo(function FolderHeader({
  folderId,
  folderName,
  folderPath,
  count,
  expanded,
  importing,
  themeColor,
  appThemeColor,
  currentDefaultAgent,
  availableAgents,
  availableAgentsFresh,
  onToggle,
  onRemoveFromWorkspace,
  onNewConversation,
  onImport,
  onManageConversations,
  onChangeColor,
  onSetDefaultAgent,
  onOpenInSystemExplorer,
  onOpenInTerminal,
  isDragging,
  onGripPointerDown,
  suppressed = false,
}: {
  folderId: number
  folderName: string
  folderPath: string
  count: number
  expanded: boolean
  importing: boolean
  themeColor: FolderThemeColor
  appThemeColor: ThemeColor
  currentDefaultAgent: AgentType | null
  availableAgents: AgentType[]
  /**
   * False while `useSortedAvailableAgents` is still serving the
   * localStorage seed (i.e. `acpListAgents()` has not yet succeeded this
   * session). The "Set default agent" submenu disables agent selection
   * while not fresh — otherwise the user could persist a folder default
   * pointing at a stale/uninstalled agent. The "No default" option stays
   * usable since clearing a default doesn't depend on the live list.
   */
  availableAgentsFresh: boolean
  onToggle: (folderId: number) => void
  onRemoveFromWorkspace: (folderId: number) => void
  onNewConversation: (folderId: number) => void
  onImport: (folderId: number) => void
  onManageConversations: (folderId: number) => void
  onChangeColor: (folderId: number, color: FolderThemeColor) => void
  onSetDefaultAgent: (folderId: number, agentType: AgentType | null) => void
  onOpenInSystemExplorer: (folderId: number) => void
  onOpenInTerminal: (folderId: number) => void
  isDragging?: boolean
  /**
   * Starts a folder reorder gesture from the header's grip. Omitted on the drag
   * surface (already dragging) so headers there are pure drop-target visuals.
   */
  onGripPointerDown?: (folderId: number, event: React.PointerEvent) => void
  /**
   * True for the in-list copy of the folder whose floating sticky overlay is
   * currently showing: the overlay is the accessible control for that folder,
   * so the (scrolled-past, occluded) in-list copy is made `inert` + aria-hidden
   * to avoid a duplicate tab stop / double announcement during the window where
   * virtua still keeps it mounted in the buffer.
   */
  suppressed?: boolean
}) {
  // Own the translations here rather than receiving `t` as a prop: next-intl
  // returns a fresh `t` on every parent render, so passing it down would defeat
  // this component's memo and re-render every header on each status event.
  const t = useTranslations("Folder.sidebar")
  // Only flag a stale default once the live list is known; before fresh,
  // `availableAgents` is the localStorage seed and may legitimately omit a
  // newly-enabled agent.
  const showStaleDefault =
    availableAgentsFresh &&
    currentDefaultAgent !== null &&
    !availableAgents.includes(currentDefaultAgent)
  const tFileTree = useTranslations("Folder.fileTreeTab")
  const systemExplorerLabel =
    typeof navigator === "undefined"
      ? tFileTree("openInFileManager")
      : (() => {
          const platform =
            `${navigator.platform} ${navigator.userAgent}`.toLowerCase()
          if (platform.includes("mac")) return tFileTree("openInFinder")
          if (platform.includes("win")) return tFileTree("openInExplorer")
          return tFileTree("openInFileManager")
        })()
  // `revealItemInDir` only works inside Tauri; in web mode it is a no-op,
  // so disable the entry there to avoid silent failures.
  const isDesktopMode = isDesktop()
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          inert={suppressed || undefined}
          aria-hidden={suppressed || undefined}
          className={cn("relative h-[2rem]", isDragging && "opacity-60")}
        >
          <div
            onPointerDown={(e) => onGripPointerDown?.(folderId, e)}
            className={cn(
              "group flex h-[1.9375rem] w-full items-center",
              "rounded-full",
              "transition-colors duration-150",
              isDragging
                ? "cursor-grabbing"
                : "cursor-grab hover:bg-[color-mix(in_oklab,var(--sidebar-accent),var(--sidebar-foreground)_2%)]"
            )}
          >
            <button
              data-folder-id={folderId}
              onClick={() => onToggle(folderId)}
              title={folderPath}
              aria-expanded={expanded}
              className={cn(
                "relative flex h-full min-w-0 flex-1 items-center pr-[0.5rem] outline-none",
                "rounded-full focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
                "text-sidebar-foreground",
                isDragging ? "cursor-grabbing" : "cursor-grab"
              )}
              style={{ paddingLeft: "calc(var(--conv-rail-axis) + 0.875rem)" }}
            >
              <span
                aria-hidden
                className={cn(
                  "pointer-events-none absolute flex items-center justify-center text-muted-foreground/75"
                )}
                style={{
                  top: "50%",
                  left: "var(--conv-rail-axis)",
                  width: "0.875rem",
                  height: "0.875rem",
                  transform: "translate(-50%, -50%)",
                }}
              >
                {expanded ? (
                  <FolderOpen className="h-[0.875rem] w-[0.875rem]" />
                ) : (
                  <Folder className="h-[0.875rem] w-[0.875rem]" />
                )}
              </span>
              <div className="flex min-w-0 flex-1 items-center gap-[0.5rem]">
                <span
                  className={cn(
                    "min-w-0 flex-shrink truncate text-left text-[0.875rem] font-normal text-sidebar-foreground/75"
                  )}
                >
                  {folderName}
                </span>
                <span
                  className={cn(
                    "inline-flex shrink-0 items-center justify-center",
                    "h-[0.9375rem] min-w-[1rem] rounded-[0.3125rem] px-[0.25rem]",
                    "text-[0.625rem] font-semibold leading-none tabular-nums",
                    "bg-primary/10 text-primary"
                  )}
                >
                  {count}
                </span>
              </div>
            </button>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onNewConversation(folderId)
              }}
              title={t("newConversation")}
              aria-label={t("newConversation")}
              className={cn(
                "mr-[0.125rem] flex h-7 w-7 shrink-0 items-center justify-center",
                "rounded-[0.375rem] cursor-pointer outline-none text-muted-foreground/80",
                "opacity-0 group-hover:opacity-100 focus-visible:opacity-100 [@media(hover:none)]:opacity-100",
                "transition-[opacity,color,background-color] duration-150 hover:bg-sidebar-accent hover:text-sidebar-foreground"
              )}
            >
              <Plus className="h-[0.875rem] w-[0.875rem]" />
            </button>
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onSelect={() => onNewConversation(folderId)}>
          <Plus className="h-4 w-4" />
          {t("newConversation")}
        </ContextMenuItem>
        <ContextMenuItem
          disabled={importing}
          onSelect={() => onImport(folderId)}
        >
          <Download className="h-4 w-4" />
          {importing ? t("importing") : t("importLocalSessions")}
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <ExternalLink className="h-4 w-4" />
            {tFileTree("openIn")}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              disabled={!isDesktopMode}
              onSelect={() => onOpenInSystemExplorer(folderId)}
            >
              {systemExplorerLabel}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => onOpenInTerminal(folderId)}>
              {tFileTree("openInTerminal")}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => onManageConversations(folderId)}>
          <ListChecks className="h-4 w-4" />
          {t("folderHeaderMenu.manageConversations")}
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <Bot className="h-4 w-4" />
            {t("folderHeaderMenu.setDefaultAgent")}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent className="min-w-[12rem]">
            <ContextMenuItem
              onSelect={() => onSetDefaultAgent(folderId, null)}
              className="gap-2"
            >
              <span className="min-w-0 flex-1 truncate">
                {t("folderHeaderMenu.defaultAgentNone")}
              </span>
              {currentDefaultAgent === null ? (
                <Check className="h-3.5 w-3.5 shrink-0" />
              ) : null}
            </ContextMenuItem>
            <ContextMenuSeparator />
            {availableAgentsFresh ? (
              <>
                {availableAgents.map((agent) => {
                  const active = currentDefaultAgent === agent
                  return (
                    <ContextMenuItem
                      key={agent}
                      onSelect={() => onSetDefaultAgent(folderId, agent)}
                      className="gap-2"
                    >
                      <span className="min-w-0 flex-1 truncate">
                        {AGENT_LABELS[agent]}
                      </span>
                      {active ? (
                        <Check className="h-3.5 w-3.5 shrink-0" />
                      ) : null}
                    </ContextMenuItem>
                  )
                })}
                {showStaleDefault && currentDefaultAgent !== null ? (
                  <ContextMenuItem
                    key={currentDefaultAgent}
                    disabled
                    className="gap-2 opacity-60"
                  >
                    <span className="min-w-0 flex-1 truncate">
                      {`${AGENT_LABELS[currentDefaultAgent]} ${t("folderHeaderMenu.agentUnavailableSuffix")}`}
                    </span>
                    <Check className="h-3.5 w-3.5 shrink-0" />
                  </ContextMenuItem>
                ) : null}
              </>
            ) : (
              <ContextMenuItem disabled className="gap-2 opacity-60">
                <span className="min-w-0 flex-1 truncate">
                  {t("folderHeaderMenu.loadingAgents")}
                </span>
              </ContextMenuItem>
            )}
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSub>
          <ContextMenuSubTrigger>
            <Palette className="h-4 w-4" />
            {t("folderHeaderMenu.changeColor")}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent className="min-w-[12rem] p-2">
            <ContextMenuItem
              onSelect={() =>
                onChangeColor(folderId, FOLDER_THEME_COLOR_INHERIT)
              }
              className="gap-2"
            >
              <span
                aria-hidden
                className="h-[1.125rem] w-[1.125rem] shrink-0 rounded-[0.25rem] border border-border"
                style={{ backgroundColor: THEME_COLOR_PREVIEW[appThemeColor] }}
              />
              <span className="min-w-0 flex-1 truncate">
                {t("folderHeaderMenu.useThemeColor")}
              </span>
              {themeColor === FOLDER_THEME_COLOR_INHERIT ? (
                <Check className="h-3.5 w-3.5 shrink-0" />
              ) : null}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <div className="grid grid-cols-6 gap-1">
              {THEME_COLORS.map((color) => {
                const active = color === themeColor
                return (
                  <button
                    key={color}
                    type="button"
                    title={color}
                    aria-label={color}
                    onClick={() => onChangeColor(folderId, color)}
                    className={cn(
                      "h-[1.125rem] w-[1.125rem] cursor-pointer rounded-[0.25rem]",
                      "outline-none ring-offset-1 ring-offset-popover",
                      "transition-[box-shadow,transform] duration-100 hover:scale-110",
                      active && "ring-2 ring-foreground/60"
                    )}
                    style={{ backgroundColor: THEME_COLOR_PREVIEW[color] }}
                  />
                )
              })}
            </div>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSeparator />
        <ContextMenuItem
          variant="destructive"
          onSelect={() => onRemoveFromWorkspace(folderId)}
        >
          <XCircle className="h-4 w-4" />
          {t("folderHeaderMenu.removeFromWorkspace")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
})

export interface SidebarConversationListHandle {
  scrollToActive: () => void
  expandAll: () => void
  collapseAll: () => void
}

export interface SidebarConversationListProps {
  showCompleted?: boolean
  sortMode?: SidebarSortMode
}

export function SidebarConversationList({
  ref,
  showCompleted = true,
  sortMode = "created",
}: SidebarConversationListProps & {
  ref?: Ref<SidebarConversationListHandle>
}) {
  const t = useTranslations("Folder.sidebar")
  const tCommon = useTranslations("Folder.common")
  const tFolderDropdown = useTranslations("Folder.folderNameDropdown")
  const tFileTree = useTranslations("Folder.fileTreeTab")
  const { resolvedTheme } = useTheme()
  const { themeColor: appThemeColor } = useThemeColor()
  const { createTerminalInDirectory } = useTerminalContext()
  useZoomLevel()
  const {
    folders,
    allFolders,
    conversations,
    conversationsLoading: loading,
    conversationsError: error,
    refreshConversations,
    updateConversationLocal,
    removeFolderFromWorkspace,
    reorderFolders,
    openFolder,
    refreshFolder,
  } = useAppWorkspace()
  const refreshing = loading
  const { activeFolder } = useActiveFolder()

  const {
    openTab,
    closeConversationTab,
    closeTabsByFolder,
    openNewConversationTab,
    activeTabId,
    tabs,
  } = useTabContext()
  const { addTask, updateTask } = useTaskContext()

  const folderIndex = useMemo(() => {
    const map = new Map<
      number,
      {
        name: string
        path: string
        color: string
        defaultAgentType: AgentType | null
      }
    >()
    for (const f of allFolders)
      map.set(f.id, {
        name: f.name,
        path: f.path,
        color: f.color,
        defaultAgentType: f.default_agent_type,
      })
    return map
  }, [allFolders])

  // `tabs` gets a fresh array reference on every `conversations` change (the tab
  // context re-derives titles/status), so these two derivations would otherwise
  // hand a new object / Set to every FolderGroupItem on each status event and
  // defeat its memo. Reuse the previous reference when the content is unchanged
  // (render-phase ref cache; idempotent under StrictMode's double invoke).
  const selectedConvRef = useRef<{ id: number; agentType: string } | null>(null)
  const selectedConversation = useMemo(() => {
    const activeTab = tabs.find((tab) => tab.id === activeTabId)
    const next =
      !activeTab || activeTab.conversationId == null
        ? null
        : { id: activeTab.conversationId, agentType: activeTab.agentType }
    const reused = reuseSelected(selectedConvRef.current, next)
    selectedConvRef.current = reused
    return reused
  }, [tabs, activeTabId])

  const openTabKeysRef = useRef<Set<string>>(new Set())
  const openTabKeys = useMemo(() => {
    const next = new Set<string>()
    for (const tab of tabs) {
      if (tab.conversationId != null) {
        next.add(`${tab.agentType}:${tab.conversationId}`)
      }
    }
    const reused = reuseSet(openTabKeysRef.current, next)
    openTabKeysRef.current = reused
    return reused
  }, [tabs])

  const [importing, setImporting] = useState(false)
  const { sortedTypes: availableAgents, fresh: availableAgentsFresh } =
    useSortedAvailableAgents()
  const [folderExpanded, setFolderExpanded] = useState<Record<number, boolean>>(
    {}
  )
  const [removeConfirm, setRemoveConfirm] = useState<{
    folderId: number
    folderName: string
  } | null>(null)
  const [manageState, setManageState] = useState<{
    folderId: number
    folderName: string
  } | null>(null)
  const [cloneOpen, setCloneOpen] = useState(false)
  const [browserOpen, setBrowserOpen] = useState(false)
  const [dragging, setDragging] = useState<number | null>(null)
  const [reordering, setReordering] = useState(false)
  const [dragOrder, setDragOrder] = useState<number[] | null>(null)
  const pendingOrderRef = useRef<number[] | null>(null)

  // Floating sticky folder header. `stickyFolderId` is the ONLY new render
  // state and changes solely when the scroll crosses into a different folder —
  // never on a status event or the per-minute `now` tick — so the card/header
  // memo budget is untouched. The per-frame handoff translateY is written
  // straight to the overlay node (no re-render); see `recomputeSticky`.
  const [stickyFolderId, setStickyFolderId] = useState<number | null>(null)
  const overlayRef = useRef<HTMLDivElement>(null)
  const stickyRafRef = useRef<number | null>(null)
  // Read by the imperative scroll path without re-subscribing virtua's listener.
  const draggingRef = useRef<number | null>(dragging)
  draggingRef.current = dragging

  // Custom pointer-based folder reorder (replaces motion `Reorder`, which can't
  // coexist with virtualization — see the perf plan). Refs are read by the
  // window pointer listeners so the public callbacks stay referentially stable
  // (the `FolderHeader` memo depends on a stable `onGripPointerDown`).
  const dragSurfaceRef = useRef<HTMLDivElement>(null)
  const dragPointerRef = useRef<{
    folderId: number
    pointerId: number
    startX: number
    startY: number
    lastY: number
    started: boolean
  } | null>(null)
  const dragCleanupRef = useRef<(() => void) | null>(null)
  const autoscrollRef = useRef<number | null>(null)
  // Snapshots read by the imperative drag listeners without re-subscribing them.
  const orderedFolderIdsRef = useRef<number[]>([])
  const reorderingRef = useRef(false)

  useEffect(() => {
    // Hydrate from localStorage after mount to keep SSR/CSR markup consistent.

    setFolderExpanded(loadFolderExpanded())
  }, [])

  const handleChangeFolderColor = useCallback(
    async (folderId: number, color: FolderThemeColor) => {
      try {
        await updateFolderColor(folderId, color)
        await refreshFolder(folderId)
      } catch (err) {
        const msg = toErrorMessage(err)
        toast.error(t("toasts.changeFolderColorFailed", { message: msg }))
      }
    },
    [refreshFolder, t]
  )

  const handleChangeFolderDefaultAgent = useCallback(
    async (folderId: number, agentType: AgentType | null) => {
      try {
        await updateFolderDefaultAgent(folderId, agentType)
        await refreshFolder(folderId)
      } catch (err) {
        const msg = toErrorMessage(err)
        toast.error(
          t("toasts.changeFolderDefaultAgentFailed", { message: msg })
        )
      }
    },
    [refreshFolder, t]
  )

  const handleOpenFolderInSystemExplorer = useCallback(
    (folderId: number) => {
      const folder = folderIndex.get(folderId)
      if (!folder) return
      void revealItemInDir(folder.path).catch(() => {
        toast.error(tFileTree("toasts.openDirectoryFailed"))
      })
    },
    [folderIndex, tFileTree]
  )

  const handleOpenFolderInTerminal = useCallback(
    async (folderId: number) => {
      const folder = folderIndex.get(folderId)
      if (!folder) return
      const title = tFileTree("terminalTitle", { name: folder.name })
      const id = await createTerminalInDirectory(folder.path, title)
      if (!id) {
        toast.error(tFileTree("toasts.openBuiltinTerminalFailed"))
      }
    },
    [folderIndex, createTerminalInDirectory, tFileTree]
  )

  // virtua binds to the real OverlayScrollbars viewport element (surfaced via
  // the ScrollArea `onViewportRef` bridge once OS has initialized). We keep both
  // a ref (for the Virtualizer `scrollRef` prop) and a state flag so the
  // Virtualizer only mounts after the viewport exists.
  const viewportRef = useRef<HTMLElement | null>(null)
  const [viewportEl, setViewportEl] = useState<HTMLElement | null>(null)
  const handleViewportRef = useCallback((element: HTMLElement | null) => {
    viewportRef.current = element
    setViewportEl(element)
  }, [])
  const virtualizerRef = useRef<VirtualizerHandle>(null)
  const scrollToActiveRef = useRef<() => void>(() => {})
  const pendingScrollRef = useRef(false)

  // Single "now" shared by every relative time label, refreshed once a minute.
  // Threading one value through all rows (instead of each row calling
  // `Date.now()` during render) keeps `timeLabel` referentially stable within a
  // render tick, so a single status event re-renders only the affected card.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const interval = setInterval(() => setNow(Date.now()), 60_000)
    return () => clearInterval(interval)
  }, [])

  const filteredConversations = useMemo(() => {
    if (showCompleted) return conversations
    return conversations.filter((c) => c.status !== "completed")
  }, [conversations, showCompleted])

  // Maps each open worktree child folder → its (open) root folder. A child is
  // only redirected when its parent is also open, so a worktree whose root was
  // closed/removed falls back to standing on its own (its conversations stay
  // reachable). The merge is display-only: it never rewrites `conversation.folder_id`.
  const childToParent = useMemo(() => {
    const openIds = new Set(folders.map((f) => f.id))
    const map = new Map<number, number>()
    for (const f of folders) {
      if (f.parent_id != null && openIds.has(f.parent_id)) {
        map.set(f.id, f.parent_id)
      }
    }
    return map
  }, [folders])

  // Hold the previous grouping so unchanged folders keep their bucket array
  // reference across renders (lets memoized FolderGroupItems bail out). Updating
  // the ref inside the memo factory is a deliberate cache, idempotent under
  // StrictMode's double invoke.
  const byFolderRef = useRef<Map<number, DbConversationSummary[]>>(new Map())
  const byFolder = useMemo(() => {
    const grouped = groupByFolderWithReuse(
      filteredConversations,
      sortMode,
      byFolderRef.current,
      childToParent
    )
    byFolderRef.current = grouped
    return grouped
  }, [filteredConversations, sortMode, childToParent])

  const folderTotalCounts = useMemo(() => {
    const map = new Map<number, number>()
    for (const conv of conversations) {
      const groupId = childToParent.get(conv.folder_id) ?? conv.folder_id
      map.set(groupId, (map.get(groupId) ?? 0) + 1)
    }
    return map
  }, [conversations, childToParent])

  const orderedFolderIds = useMemo(() => {
    const folderIdSet = new Set(folders.map((f) => f.id))
    // Worktree child folders are merged into their parent group, so they never
    // get their own header row.
    const isMergedChild = (id: number) => childToParent.has(id)
    // During drag we honour the optimistic order so sibling folders shift live
    // as the user hovers over slots. We still filter/append against the source
    // of truth so newly-added or -removed folders don't disappear mid-drag.
    if (dragOrder) {
      const seen = new Set<number>()
      const ids: number[] = []
      for (const id of dragOrder) {
        if (folderIdSet.has(id) && !seen.has(id) && !isMergedChild(id)) {
          seen.add(id)
          ids.push(id)
        }
      }
      for (const f of folders) {
        if (!seen.has(f.id) && !isMergedChild(f.id)) {
          seen.add(f.id)
          ids.push(f.id)
        }
      }
      return ids
    }

    const seen = new Set<number>()
    const ids: number[] = []
    for (const f of folders) {
      if (!seen.has(f.id) && !isMergedChild(f.id)) {
        seen.add(f.id)
        ids.push(f.id)
      }
    }
    return ids
  }, [folders, dragOrder, childToParent])

  const darkMode = resolvedTheme === "dark"

  // Flat row model for windowing. Deliberately excludes `now` (see buildRows):
  // the per-minute label tick must not rebuild rows and break the card memo.
  const rows = useMemo(
    () =>
      buildRows(orderedFolderIds, byFolder, folderExpanded, folderTotalCounts),
    [orderedFolderIds, byFolder, folderExpanded, folderTotalCounts]
  )

  // Latest snapshots for the imperative scroll/drag code paths, refreshed every
  // render so the window listeners and scrollToActive read current values
  // without being torn down and re-subscribed.
  const rowsRef = useRef<SidebarRow[]>(rows)
  rowsRef.current = rows
  orderedFolderIdsRef.current = orderedFolderIds
  reorderingRef.current = reordering

  // Sticky-overlay lookup tables, rebuilt only when the flat rows change
  // (folder add/remove/expand, not status events). Consumed exclusively by the
  // imperative scroll handler via refs — never passed to a memoized child — so
  // they have zero effect on the card/header memo path.
  const ownerHeaderIndex = useMemo(() => buildOwnerHeaderIndex(rows), [rows])
  const headerFlatIndices = useMemo(() => folderHeaderFlatIndices(rows), [rows])
  const ownerHeaderIndexRef = useRef(ownerHeaderIndex)
  ownerHeaderIndexRef.current = ownerHeaderIndex
  const headerFlatIndicesRef = useRef(headerFlatIndices)
  headerFlatIndicesRef.current = headerFlatIndices

  useImperativeHandle(ref, () => ({
    scrollToActive() {
      scrollToActiveRef.current()
    },
    expandAll() {
      setFolderExpanded((prev) => {
        const next: Record<number, boolean> = { ...prev }
        for (const id of orderedFolderIds) next[id] = true
        saveFolderExpanded(next)
        return next
      })
    },
    collapseAll() {
      setFolderExpanded((prev) => {
        const next: Record<number, boolean> = { ...prev }
        for (const id of orderedFolderIds) next[id] = false
        saveFolderExpanded(next)
        return next
      })
    },
  }))

  useEffect(() => {
    scrollToActiveRef.current = () => {
      if (!selectedConversation) return
      const targetId = selectedConversation.id
      const targetAgent = selectedConversation.agentType
      const conv = conversations.find(
        (c) => c.id === targetId && c.agent_type === targetAgent
      )
      if (!conv) return
      // A worktree conversation is rendered under its parent group, so the row's
      // visibility is gated by the parent's expansion — expand the display group,
      // not the (never-rendered) child folder id.
      const displayFolderId =
        childToParent.get(conv.folder_id) ?? conv.folder_id
      if (!(folderExpanded[displayFolderId] ?? true)) {
        // Expand first; the row only exists in the flat model once expanded, so
        // defer the actual scroll to the next render (this effect re-runs on the
        // folderExpanded change with the rebuilt rows available via rowsRef).
        setFolderExpanded((prev) => {
          const next = { ...prev, [displayFolderId]: true }
          saveFolderExpanded(next)
          return next
        })
        pendingScrollRef.current = true
        return
      }
      // Off-screen virtualized rows are not in the DOM, so resolve the flat row
      // index and let virtua scroll to it.
      const index = flatIndexOfConversation(
        rowsRef.current,
        targetId,
        targetAgent
      )
      if (index < 0) return
      virtualizerRef.current?.scrollToIndex(index, {
        align: "center",
        smooth: true,
      })
    }

    if (pendingScrollRef.current) {
      pendingScrollRef.current = false
      scrollToActiveRef.current()
    }
  }, [selectedConversation, conversations, folderExpanded, childToParent])

  const toggleFolder = useCallback((folderId: number) => {
    setFolderExpanded((prev) => {
      const next = { ...prev, [folderId]: !(prev[folderId] ?? true) }
      saveFolderExpanded(next)
      return next
    })
  }, [])

  // ── Sticky folder header overlay ──────────────────────────────────────────
  // Resolve the folder currently scrolled through and the iOS handoff offset
  // from the live virtua geometry. Imperative + ref-only so its identity stays
  // stable (passing it to `<Virtualizer onScroll>` must not re-subscribe the
  // listener) and so it never participates in the memoized render path.
  const recomputeSticky = useCallback(() => {
    const handle = virtualizerRef.current
    const currentRows = rowsRef.current
    const headers = headerFlatIndicesRef.current
    if (
      !handle ||
      draggingRef.current !== null ||
      currentRows.length === 0 ||
      headers.length === 0
    ) {
      setStickyFolderId((prev) => (prev === null ? prev : null))
      return
    }
    const scrollOffset = handle.scrollOffset
    const topIndex = Math.max(
      0,
      Math.min(currentRows.length - 1, handle.findItemIndex(scrollOffset))
    )
    const activeHeaderIndex = ownerHeaderIndexRef.current[topIndex]
    if (activeHeaderIndex < 0) {
      setStickyFolderId((prev) => (prev === null ? prev : null))
      return
    }
    const nextHeaderIndex = nextHeaderAfter(headers, activeHeaderIndex)
    const { visible, translateY } = computeStickyState({
      scrollOffset,
      activeHeaderOffset: handle.getItemOffset(activeHeaderIndex),
      nextHeaderOffset:
        nextHeaderIndex == null ? null : handle.getItemOffset(nextHeaderIndex),
      headerHeight: handle.getItemSize(activeHeaderIndex) || 32,
    })
    if (overlayRef.current) {
      overlayRef.current.style.transform = `translateY(${translateY}px)`
    }
    const activeRow = currentRows[activeHeaderIndex]
    const nextFolderId =
      visible && activeRow.kind === "folder" ? activeRow.folderId : null
    setStickyFolderId((prev) => (prev === nextFolderId ? prev : nextFolderId))
  }, [])

  // virtua fires onScroll synchronously per scroll event; coalesce to one
  // recompute per frame and keep the DOM write frame-aligned.
  const handleVirtuaScroll = useCallback(() => {
    if (stickyRafRef.current != null) return
    stickyRafRef.current = requestAnimationFrame(() => {
      stickyRafRef.current = null
      recomputeSticky()
    })
  }, [recomputeSticky])

  // Collapse from the overlay, then bring the now-collapsed header to the top so
  // the eye lands on the folder just folded (the in-list toggle leaves you mid
  // next folder otherwise). Deferred so virtua re-measures the shorter list
  // before scrolling. Header index is unchanged by its own collapse, but we
  // re-resolve it to stay correct regardless.
  const handleOverlayToggle = useCallback(
    (folderId: number) => {
      toggleFolder(folderId)
      requestAnimationFrame(() => {
        const idx = headerIndexForFolder(rowsRef.current, folderId)
        if (idx >= 0) {
          virtualizerRef.current?.scrollToIndex(idx, {
            align: "start",
            smooth: false,
          })
        }
      })
    },
    [toggleFolder]
  )

  // Recompute on anything that shifts geometry without firing a scroll event:
  // expand/collapse, reorder, data refresh, drag start/end, viewport ready, and
  // the overlay flip itself (so the freshly-mounted overlay node gets its
  // initial transform). `useLayoutEffect` avoids a one-frame stale overlay.
  useIsomorphicLayoutEffect(() => {
    recomputeSticky()
  }, [
    rows,
    folderExpanded,
    viewportEl,
    dragging,
    stickyFolderId,
    recomputeSticky,
  ])

  useEffect(
    () => () => {
      if (stickyRafRef.current != null) {
        cancelAnimationFrame(stickyRafRef.current)
      }
    },
    []
  )

  const handleRemoveFolder = useCallback(
    (folderId: number) => {
      const name = folderIndex.get(folderId)?.name ?? String(folderId)
      setRemoveConfirm({ folderId, folderName: name })
    },
    [folderIndex]
  )

  const handleManageConversations = useCallback(
    (folderId: number) => {
      const name = folderIndex.get(folderId)?.name ?? String(folderId)
      setManageState({ folderId, folderName: name })
    },
    [folderIndex]
  )

  const handleRemoveFolderConfirm = useCallback(async () => {
    if (!removeConfirm) return
    const { folderId, folderName } = removeConfirm
    try {
      closeTabsByFolder(folderId)
      await removeFolderFromWorkspace(folderId)
      toast.success(t("toasts.folderRemoved", { name: folderName }))
    } catch (e) {
      const msg = toErrorMessage(e)
      toast.error(t("toasts.removeFolderFailed", { message: msg }))
    } finally {
      setRemoveConfirm(null)
    }
  }, [removeConfirm, closeTabsByFolder, removeFolderFromWorkspace, t])

  // The card already holds the full summary, so it passes `folderId` back to
  // these callbacks. That removes the `conversations` closure dependency, which
  // is what keeps these references stable across status events — the linchpin
  // for the card `memo` actually bailing out (see Phase 1 of the perf plan).
  const handleSelect = useCallback(
    (id: number, agentType: string, folderId: number) => {
      openTab(folderId, id, agentType as Parameters<typeof openTab>[2], false)
    },
    [openTab]
  )

  const handleDoubleClick = useCallback(
    (id: number, agentType: string, folderId: number) => {
      openTab(folderId, id, agentType as Parameters<typeof openTab>[2], true)
    },
    [openTab]
  )

  const handleRename = useCallback(
    async (id: number, newTitle: string) => {
      await updateConversationTitle(id, newTitle)
      refreshConversations()
    },
    [refreshConversations]
  )

  const handleDelete = useCallback(
    async (id: number, agentType: string, folderId: number) => {
      await deleteConversation(id)
      // No-op if no matching tab is open (the context guards on its tab ref).
      closeConversationTab(
        folderId,
        id,
        agentType as Parameters<typeof openTab>[2]
      )
      refreshConversations()
    },
    [closeConversationTab, refreshConversations]
  )

  const handleStatusChange = useCallback(
    async (id: number, status: ConversationStatus) => {
      updateConversationLocal(id, { status })
      await updateConversationStatus(id, status)
    },
    [updateConversationLocal]
  )

  const handleNewConversation = useCallback(() => {
    if (!activeFolder) return
    openNewConversationTab(activeFolder.id, activeFolder.path)
  }, [activeFolder, openNewConversationTab])

  const handleNewConversationForFolder = useCallback(
    (folderId: number) => {
      const folder = folderIndex.get(folderId)
      if (!folder) return
      openNewConversationTab(folderId, folder.path)
    },
    [folderIndex, openNewConversationTab]
  )

  const handleImportForFolder = useCallback(
    async (folderId: number) => {
      if (importing) return
      setImporting(true)
      const taskId = `import-${folderId}-${Date.now()}`
      addTask(taskId, t("importLocalSessions"))
      updateTask(taskId, { status: "running" })
      try {
        const result = await importLocalConversations(folderId)
        updateTask(taskId, { status: "completed" })
        refreshConversations()
        if (result.imported > 0) {
          toast.success(
            t("toasts.importedSessions", {
              imported: result.imported,
              skipped: result.skipped,
            })
          )
        } else {
          toast.info(
            t("toasts.noNewSessionsFound", { skipped: result.skipped })
          )
        }
      } catch (e) {
        const msg = toErrorMessage(e)
        updateTask(taskId, { status: "failed", error: msg })
        toast.error(t("toasts.importFailed", { message: msg }))
      } finally {
        setImporting(false)
      }
    },
    [importing, addTask, updateTask, refreshConversations, t]
  )

  const persistReorder = useCallback(
    async (order: number[]) => {
      if (order.length === 0) return
      setReordering(true)
      try {
        await reorderFolders(order)
      } catch (e) {
        const msg = toErrorMessage(e)
        toast.error(t("toasts.reorderFoldersFailed", { message: msg }))
      } finally {
        setReordering(false)
      }
    },
    [reorderFolders, t]
  )

  const handleReorder = useCallback((nextIds: number[]) => {
    pendingOrderRef.current = nextIds
    setDragOrder(nextIds)
  }, [])

  const handleDragEnd = useCallback(async () => {
    setDragging(null)
    const order = pendingOrderRef.current
    pendingOrderRef.current = null
    if (!order) {
      setDragOrder(null)
      return
    }
    try {
      await persistReorder(order)
    } finally {
      // Clear the optimistic override once the workspace context's folders
      // have absorbed the new order (or on failure, the rollback in the
      // context restores the original order).
      setDragOrder(null)
    }
  }, [persistReorder])

  // ── Custom folder-drag gesture ────────────────────────────────────────────
  // Fixed height of one folder header row (Tailwind `h-[2rem]`); the drag
  // surface collapses every folder to just its header so the target slot is a
  // simple `floor(pointerY / FOLDER_ROW_HEIGHT)`.
  const FOLDER_ROW_HEIGHT = 32
  const DRAG_THRESHOLD_PX = 6
  const AUTOSCROLL_EDGE_PX = 28
  const AUTOSCROLL_STEP_PX = 12

  const stopAutoscroll = useCallback(() => {
    if (autoscrollRef.current != null) {
      cancelAnimationFrame(autoscrollRef.current)
      autoscrollRef.current = null
    }
  }, [])

  // Suppress exactly one trailing click after a real drag so the gesture never
  // also toggles a folder. The grip element that received `pointerdown` unmounts
  // when the drag surface takes over, so a per-element guard would be unreliable;
  // a one-shot capture listener is robust and self-cleans (the rAF drops it if
  // the browser synthesizes no click, leaving later legitimate clicks intact).
  const suppressNextClick = useCallback(() => {
    const onClick = (event: MouseEvent) => {
      event.preventDefault()
      event.stopPropagation()
    }
    window.addEventListener("click", onClick, { capture: true, once: true })
    requestAnimationFrame(() => {
      window.removeEventListener("click", onClick, true)
    })
  }, [])

  // Reorder the grabbed folder to the slot under the pointer (optimistically,
  // via the same `dragOrder` machinery the persisted reorder uses). Targeting is
  // intentionally gated on the collapsed drag surface existing: until it mounts,
  // the only available geometry is the *expanded* virtualized list, whose
  // scrollTop/row mix would map the pointer to a bogus far index (and clamp it
  // to the last folder). Skipping until the surface is up means a too-quick
  // release simply leaves the order untouched.
  const updateDragTarget = useCallback(
    (clientY: number) => {
      const state = dragPointerRef.current
      const surface = dragSurfaceRef.current
      if (!state || !surface) return
      const order = orderedFolderIdsRef.current
      // The surface's live rect already reflects scroll, so no scrollTop term.
      const targetIndex = pointerYToTargetIndex(
        clientY,
        surface.getBoundingClientRect().top,
        0,
        FOLDER_ROW_HEIGHT,
        order.length
      )
      const fromIndex = order.indexOf(state.folderId)
      if (fromIndex < 0 || fromIndex === targetIndex) return
      handleReorder(applyReorder(order, fromIndex, targetIndex))
    },
    [handleReorder]
  )

  // While the pointer rests near a viewport edge, scroll and keep retargeting so
  // off-screen folders remain reachable as drop targets.
  const maybeAutoscroll = useCallback(
    (clientY: number) => {
      const viewport = viewportRef.current
      if (!viewport) return
      const rect = viewport.getBoundingClientRect()
      const atTop = clientY < rect.top + AUTOSCROLL_EDGE_PX
      const atBottom = clientY > rect.bottom - AUTOSCROLL_EDGE_PX
      if (!atTop && !atBottom) {
        stopAutoscroll()
        return
      }
      if (autoscrollRef.current != null) return
      const step = () => {
        const v = viewportRef.current
        const state = dragPointerRef.current
        if (!v || !state) {
          stopAutoscroll()
          return
        }
        const r = v.getBoundingClientRect()
        const dir = state.lastY < r.top + AUTOSCROLL_EDGE_PX ? -1 : 1
        v.scrollTop += dir * AUTOSCROLL_STEP_PX
        updateDragTarget(state.lastY)
        autoscrollRef.current = requestAnimationFrame(step)
      }
      autoscrollRef.current = requestAnimationFrame(step)
    },
    [stopAutoscroll, updateDragTarget]
  )

  const teardownDragListeners = useCallback(() => {
    dragCleanupRef.current?.()
    dragCleanupRef.current = null
    stopAutoscroll()
  }, [stopAutoscroll])

  const cancelDrag = useCallback(() => {
    teardownDragListeners()
    dragPointerRef.current = null
    pendingOrderRef.current = null
    setDragging(null)
    setDragOrder(null)
  }, [teardownDragListeners])

  const finishDrag = useCallback(() => {
    teardownDragListeners()
    const state = dragPointerRef.current
    dragPointerRef.current = null
    if (state?.started) {
      // A real drag occurred → commit the optimistic order and swallow the
      // trailing click so it doesn't also toggle a folder. A pointerup that
      // never crossed the threshold falls through to the normal toggle click.
      suppressNextClick()
      void handleDragEnd()
    }
  }, [teardownDragListeners, handleDragEnd, suppressNextClick])

  const onDragPointerMove = useCallback(
    (event: PointerEvent) => {
      const state = dragPointerRef.current
      if (!state || event.pointerId !== state.pointerId) return
      state.lastY = event.clientY
      if (!state.started) {
        const moved = Math.hypot(
          event.clientX - state.startX,
          event.clientY - state.startY
        )
        if (moved < DRAG_THRESHOLD_PX) return
        state.started = true
        setDragging(state.folderId)
        setDragOrder(orderedFolderIdsRef.current.slice())
      }
      updateDragTarget(event.clientY)
      maybeAutoscroll(event.clientY)
    },
    [updateDragTarget, maybeAutoscroll]
  )

  const onDragPointerUp = useCallback(
    (event: PointerEvent) => {
      const state = dragPointerRef.current
      if (state && event.pointerId !== state.pointerId) return
      finishDrag()
    },
    [finishDrag]
  )

  // Pointer cancellation (touch interruption, browser takeover) aborts the drag
  // rather than committing a possibly-incomplete reorder.
  const onDragPointerCancel = useCallback(
    (event: PointerEvent) => {
      const state = dragPointerRef.current
      if (state && event.pointerId !== state.pointerId) return
      cancelDrag()
    },
    [cancelDrag]
  )

  const onDragKeyDown = useCallback(
    (event: KeyboardEvent) => {
      if (event.key === "Escape") cancelDrag()
    },
    [cancelDrag]
  )

  const beginFolderDrag = useCallback(
    (folderId: number, event: React.PointerEvent) => {
      if (event.button !== 0) return
      if (reorderingRef.current) return
      if (dragPointerRef.current) return
      dragPointerRef.current = {
        folderId,
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        lastY: event.clientY,
        started: false,
      }
      window.addEventListener("pointermove", onDragPointerMove)
      window.addEventListener("pointerup", onDragPointerUp)
      window.addEventListener("pointercancel", onDragPointerCancel)
      window.addEventListener("keydown", onDragKeyDown)
      dragCleanupRef.current = () => {
        window.removeEventListener("pointermove", onDragPointerMove)
        window.removeEventListener("pointerup", onDragPointerUp)
        window.removeEventListener("pointercancel", onDragPointerCancel)
        window.removeEventListener("keydown", onDragKeyDown)
      }
    },
    [onDragPointerMove, onDragPointerUp, onDragPointerCancel, onDragKeyDown]
  )

  // Safety net: drop listeners / stop autoscroll if the list unmounts mid-drag.
  useEffect(() => () => teardownDragListeners(), [teardownDragListeners])

  const handleOpenFolderAction = useCallback(async () => {
    // Native Tauri dialog only when running on local desktop (no active
    // remote workspace). Inside a remote workspace window the path lives
    // on the remote host, so we route to the in-app server-side browser
    // instead — the native dialog would pick a local path the remote
    // server can't open.
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      try {
        const result = await openFileDialog({
          directory: true,
          multiple: false,
        })
        if (!result) return
        const selected = Array.isArray(result) ? result[0] : result
        await openFolder(selected)
      } catch (err) {
        console.error("[SidebarConversationList] failed to open folder:", err)
      }
    } else {
      setBrowserOpen(true)
    }
  }, [openFolder])

  const handleBrowserSelect = useCallback(
    (path: string) => {
      openFolder(path).catch((err) => {
        console.error("[SidebarConversationList] failed to open folder:", err)
      })
    },
    [openFolder]
  )

  const handleProjectBoot = useCallback(() => {
    openProjectBootWindow().catch((err) => {
      console.error(
        "[SidebarConversationList] failed to open project boot:",
        err
      )
    })
  }, [])

  const showEmptyWorkspaceActions =
    folders.length === 0 && conversations.length === 0

  const folderThemeColor = (folderId: number): FolderThemeColor =>
    normalizeFolderThemeColor(folderIndex.get(folderId)?.color)

  // A per-row theme wrapper replaces the old per-folder-group wrapper: it scopes
  // the folder's accent color (and dark-mode flip) to that single virtual row.
  const themeWrap = (folderId: number, child: React.ReactNode) => {
    const themeColor = folderThemeColor(folderId)
    return (
      <div
        className={cn(
          darkMode && themeColor !== FOLDER_THEME_COLOR_INHERIT && "dark"
        )}
        data-theme={
          themeColor === FOLDER_THEME_COLOR_INHERIT ? undefined : themeColor
        }
      >
        {child}
      </div>
    )
  }

  const folderHeaderElement = (
    folderId: number,
    opts: {
      dragging: boolean
      collapsed?: boolean
      grip: boolean
      onToggle?: (folderId: number) => void
      suppressed?: boolean
    }
  ) => {
    const folderEntry = folderIndex.get(folderId)
    return (
      <FolderHeader
        folderId={folderId}
        folderName={folderEntry?.name ?? String(folderId)}
        folderPath={folderEntry?.path ?? ""}
        count={byFolder.get(folderId)?.length ?? 0}
        expanded={opts.collapsed ? false : (folderExpanded[folderId] ?? true)}
        importing={importing}
        themeColor={folderThemeColor(folderId)}
        appThemeColor={appThemeColor}
        currentDefaultAgent={folderEntry?.defaultAgentType ?? null}
        availableAgents={availableAgents}
        availableAgentsFresh={availableAgentsFresh}
        onToggle={opts.onToggle ?? toggleFolder}
        onRemoveFromWorkspace={handleRemoveFolder}
        onNewConversation={handleNewConversationForFolder}
        onImport={handleImportForFolder}
        onManageConversations={handleManageConversations}
        onChangeColor={handleChangeFolderColor}
        onSetDefaultAgent={handleChangeFolderDefaultAgent}
        onOpenInSystemExplorer={handleOpenFolderInSystemExplorer}
        onOpenInTerminal={handleOpenFolderInTerminal}
        isDragging={opts.dragging}
        onGripPointerDown={opts.grip ? beginFolderDrag : undefined}
        suppressed={opts.suppressed ?? false}
      />
    )
  }

  const renderRow = (row: SidebarRow) => {
    if (row.kind === "folder") {
      return themeWrap(
        row.folderId,
        folderHeaderElement(row.folderId, {
          dragging: dragging === row.folderId,
          grip: true,
          // While this folder's sticky overlay is showing, the overlay is the
          // accessible control; make the (occluded) in-list copy inert so it is
          // not a duplicate tab stop / announcement.
          suppressed: stickyFolderId === row.folderId,
        })
      )
    }
    if (row.kind === "empty") {
      return themeWrap(
        row.folderId,
        <div
          className="py-[0.375rem] text-[0.75rem] text-muted-foreground/70"
          style={{ paddingLeft: "calc(var(--conv-rail-axis) + 0.875rem)" }}
        >
          {row.totalConversationCount === 0
            ? t("emptyFolderHint")
            : t("noUnfinishedConversations")}
        </div>
      )
    }
    const conv = row.conversation
    // Worktree child folders render under their parent group, so theme the row
    // by the display group (parent) for a unified look.
    const groupId = childToParent.get(conv.folder_id) ?? conv.folder_id
    return themeWrap(
      groupId,
      <SidebarConversationCard
        conversation={conv}
        isSelected={
          selectedConversation?.agentType === conv.agent_type &&
          selectedConversation?.id === conv.id
        }
        isOpenInTab={openTabKeys.has(`${conv.agent_type}:${conv.id}`)}
        timeLabel={formatRelative(
          sortMode === "updated" ? conv.updated_at : conv.created_at,
          now
        )}
        onSelect={handleSelect}
        onDoubleClick={handleDoubleClick}
        onRename={handleRename}
        onDelete={handleDelete}
        onStatusChange={handleStatusChange}
        onNewConversation={handleNewConversationForFolder}
      />
    )
  }

  const rowKey = (row: SidebarRow): string => {
    if (row.kind === "folder") return `folder-${row.folderId}`
    if (row.kind === "empty") return `empty-${row.folderId}`
    return `conv-${row.conversation.agent_type}-${row.conversation.id}`
  }

  return (
    <div className="relative flex flex-col flex-1 min-h-0">
      {(loading || refreshing) && (
        // z-20 keeps the refresh spinner above the sticky header overlay (z-10),
        // which lives in a later sibling and would otherwise paint over it.
        <div className="absolute top-0 left-0 right-0 flex items-center justify-center py-1 z-20 pointer-events-none">
          <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        </div>
      )}

      {loading && !refreshing ? (
        <div className="px-3 space-y-1.5 overflow-hidden">
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-14 w-full rounded-md" />
          ))}
        </div>
      ) : error ? (
        <div className="flex-1 flex items-center justify-center px-3">
          <p className="text-destructive text-xs">
            {t("error", { message: error })}
          </p>
        </div>
      ) : showEmptyWorkspaceActions ? (
        <div className="flex-1 flex flex-col items-center justify-center px-3 gap-2">
          <Button
            variant="outline"
            size="sm"
            className="w-full max-w-[14rem] justify-start"
            onClick={handleOpenFolderAction}
          >
            <FolderOpen className="h-3.5 w-3.5 mr-1.5" />
            {tFolderDropdown("openFolder")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="w-full max-w-[14rem] justify-start"
            onClick={() => setCloneOpen(true)}
          >
            <GitBranch className="h-3.5 w-3.5 mr-1.5" />
            {tFolderDropdown("cloneRepository")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="w-full max-w-[14rem] justify-start"
            onClick={handleProjectBoot}
          >
            <Rocket className="h-3.5 w-3.5 mr-1.5" />
            {tFolderDropdown("projectBoot")}
          </Button>
        </div>
      ) : (
        <ContextMenu>
          <ContextMenuTrigger asChild>
            <div className="flex-1 min-h-0 relative">
              <ScrollArea
                onViewportRef={handleViewportRef}
                className={cn(
                  "h-full min-h-0 px-1.5 pb-1.5",
                  "[overflow-anchor:none]",
                  "[--conv-rail-axis:0.875rem]"
                )}
              >
                {dragging !== null ? (
                  // Drag surface: every folder collapsed to its header so any
                  // folder (even one that was virtualized off-screen) is a valid
                  // drop target. Non-virtualized — folder counts are small.
                  <div ref={dragSurfaceRef} className="flex flex-col">
                    {orderedFolderIds.map((folderId) => (
                      <div key={folderId}>
                        {themeWrap(
                          folderId,
                          folderHeaderElement(folderId, {
                            dragging: dragging === folderId,
                            collapsed: true,
                            grip: false,
                          })
                        )}
                      </div>
                    ))}
                  </div>
                ) : viewportEl ? (
                  <Virtualizer
                    ref={virtualizerRef}
                    scrollRef={viewportRef}
                    data={rows}
                    itemSize={32}
                    bufferSize={400}
                    onScroll={handleVirtuaScroll}
                  >
                    {(row: SidebarRow) => (
                      <div key={rowKey(row)}>{renderRow(row)}</div>
                    )}
                  </Virtualizer>
                ) : (
                  <div className="flex flex-col gap-1.5 pt-1">
                    {Array.from({ length: 8 }).map((_, i) => (
                      <Skeleton
                        key={i}
                        className="h-[2rem] w-full rounded-md"
                      />
                    ))}
                  </div>
                )}
              </ScrollArea>
              {/*
                Floating sticky folder header. Rendered AFTER ScrollArea so any
                `[data-folder-id]` lookup still resolves the real in-list header
                first (the real one stays mounted within virtua's buffer while
                this overlay also shows). It is a real, accessible control: once
                scrolled past, the in-list header is unmounted by virtua, so the
                overlay is the keyboard/AT path to toggle/act on that folder.
                `grip:false` — reordering is driven from the in-list header,
                whose geometry the custom drag gesture relies on. `bg-sidebar`
                lives inside themeWrap so it picks up the folder's themed
                background and occludes the rows scrolling beneath it.
              */}
              {stickyFolderId !== null && (
                <div
                  ref={overlayRef}
                  className={cn(
                    "pointer-events-none absolute left-0 right-0 top-0 z-10",
                    "px-1.5 [--conv-rail-axis:0.875rem]"
                  )}
                  style={{ willChange: "transform" }}
                >
                  {themeWrap(
                    stickyFolderId,
                    <div className="pointer-events-auto bg-sidebar">
                      {folderHeaderElement(stickyFolderId, {
                        dragging: false,
                        grip: false,
                        onToggle: handleOverlayToggle,
                      })}
                    </div>
                  )}
                </div>
              )}
            </div>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              onSelect={handleNewConversation}
              disabled={!activeFolder}
            >
              <Plus className="h-4 w-4" />
              {t("newConversation")}
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem onSelect={handleOpenFolderAction}>
              <FolderOpen className="h-4 w-4" />
              {tFolderDropdown("openFolder")}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => setCloneOpen(true)}>
              <GitBranch className="h-4 w-4" />
              {tFolderDropdown("cloneRepository")}
            </ContextMenuItem>
            <ContextMenuItem onSelect={handleProjectBoot}>
              <Rocket className="h-4 w-4" />
              {tFolderDropdown("projectBoot")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      )}

      <AlertDialog
        open={removeConfirm !== null}
        onOpenChange={(open) => !open && setRemoveConfirm(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("removeFolderConfirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("removeFolderConfirmDescription", {
                name: removeConfirm?.folderName ?? "",
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tCommon("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleRemoveFolderConfirm}>
              {tCommon("confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {manageState && (
        <ConversationManageDialog
          open
          onOpenChange={(o) => !o && setManageState(null)}
          folderId={manageState.folderId}
          folderName={manageState.folderName}
        />
      )}

      <CloneDialog open={cloneOpen} onOpenChange={setCloneOpen} />
      <DirectoryBrowserDialog
        open={browserOpen}
        onOpenChange={setBrowserOpen}
        onSelect={handleBrowserSelect}
      />
    </div>
  )
}
