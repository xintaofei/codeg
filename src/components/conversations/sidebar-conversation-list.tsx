"use client"

import {
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type Ref,
} from "react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { toast } from "sonner"
import { Reorder, useDragControls, type DragControls } from "motion/react"
import type { OverlayScrollbarsComponentRef } from "overlayscrollbars-react"
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

function parseTimestamp(value: string): number {
  const timestamp = Date.parse(value)
  return Number.isNaN(timestamp) ? 0 : timestamp
}

function compareByUpdatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const updatedDiff =
    parseTimestamp(right.updated_at) - parseTimestamp(left.updated_at)
  if (updatedDiff !== 0) return updatedDiff

  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff

  return right.id - left.id
}

function compareByCreatedAtDesc(
  left: DbConversationSummary,
  right: DbConversationSummary
): number {
  const createdDiff =
    parseTimestamp(right.created_at) - parseTimestamp(left.created_at)
  if (createdDiff !== 0) return createdDiff

  const updatedDiff =
    parseTimestamp(right.updated_at) - parseTimestamp(left.updated_at)
  if (updatedDiff !== 0) return updatedDiff

  return right.id - left.id
}

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

function formatRelative(iso: string): string {
  const ts = parseTimestamp(iso)
  if (!ts) return ""
  const diff = Math.max(0, Date.now() - ts)
  const m = Math.floor(diff / 60000)
  if (m < 1) return "now"
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h`
  const d = Math.floor(h / 24)
  if (d < 30) return `${d}d`
  const mo = Math.floor(d / 30)
  if (mo < 12) return `${mo}mo`
  const y = Math.floor(mo / 12)
  return `${y}y`
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
  dragControls,
  t,
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
  dragControls: DragControls
  t: ReturnType<typeof useTranslations>
}) {
  const showStaleDefault =
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
        <div className={cn("relative h-[2rem]", isDragging && "opacity-60")}>
          <div
            onPointerDown={(e) => {
              if (e.button !== 0) return
              dragControls.start(e)
            }}
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
              className={cn(
                "relative flex h-full min-w-0 flex-1 items-center pr-[0.5rem] outline-none",
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
                  {active ? <Check className="h-3.5 w-3.5 shrink-0" /> : null}
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

interface FolderGroupItemProps {
  folderId: number
  folderName: string
  folderPath: string
  conversations: DbConversationSummary[]
  totalConversationCount: number
  expanded: boolean
  importing: boolean
  reordering: boolean
  dragging: boolean
  sortMode: SidebarSortMode
  selectedConversation: { id: number; agentType: string } | null
  openTabKeys: Set<string>
  themeColor: FolderThemeColor
  appThemeColor: ThemeColor
  currentDefaultAgent: AgentType | null
  availableAgents: AgentType[]
  darkMode: boolean
  onToggle: (folderId: number) => void
  onRemoveFromWorkspace: (folderId: number) => void
  onNewConversationForFolder: (folderId: number) => void
  onImport: (folderId: number) => void
  onManageConversations: (folderId: number) => void
  onChangeColor: (folderId: number, color: FolderThemeColor) => void
  onSetDefaultAgent: (folderId: number, agentType: AgentType | null) => void
  onOpenInSystemExplorer: (folderId: number) => void
  onOpenInTerminal: (folderId: number) => void
  onSelect: (id: number, agentType: string) => void
  onDoubleClick: (id: number, agentType: string) => void
  onRename: (id: number, newTitle: string) => Promise<void>
  onDelete: (id: number, agentType: string) => Promise<void>
  onStatusChange: (id: number, status: ConversationStatus) => Promise<void>
  onNewConversation: () => void
  onDragStart: (folderId: number) => void
  onDragEnd: () => void
  stackIndex: number
  t: ReturnType<typeof useTranslations>
}

const DRAGGING_Z_INDEX = 10_000

function FolderGroupItem({
  folderId,
  folderName,
  folderPath,
  conversations,
  totalConversationCount,
  expanded,
  importing,
  reordering,
  dragging,
  sortMode,
  selectedConversation,
  openTabKeys,
  themeColor,
  appThemeColor,
  currentDefaultAgent,
  availableAgents,
  darkMode,
  onToggle,
  onRemoveFromWorkspace,
  onNewConversationForFolder,
  onImport,
  onManageConversations,
  onChangeColor,
  onSetDefaultAgent,
  onOpenInSystemExplorer,
  onOpenInTerminal,
  onSelect,
  onDoubleClick,
  onRename,
  onDelete,
  onStatusChange,
  onNewConversation,
  onDragStart,
  onDragEnd,
  stackIndex,
  t,
}: FolderGroupItemProps) {
  const justDraggedRef = useRef(false)
  const dragControls = useDragControls()

  const handleToggle = useCallback(
    (id: number) => {
      if (justDraggedRef.current) {
        justDraggedRef.current = false
        return
      }
      onToggle(id)
    },
    [onToggle]
  )

  const handleDragStart = useCallback(() => {
    justDraggedRef.current = true
    onDragStart(folderId)
  }, [folderId, onDragStart])

  // Wrap Reorder.Item in a plain div that owns the zIndex. Framer's Reorder.Item
  // internally overrides `style.zIndex` (forces 1 while dragging, "unset" at rest),
  // so any zIndex set directly on the Item is discarded. `isolation: isolate`
  // forces a real stacking context on each wrapper so earlier folders' sticky
  // headers always paint above later folders' conversation rows when scrolled.
  return (
    <div
      className={cn(
        "relative",
        darkMode && themeColor !== FOLDER_THEME_COLOR_INHERIT && "dark"
      )}
      data-theme={
        themeColor === FOLDER_THEME_COLOR_INHERIT ? undefined : themeColor
      }
      style={{
        isolation: "isolate",
        zIndex: dragging ? DRAGGING_Z_INDEX : stackIndex,
      }}
    >
      <Reorder.Item
        as="div"
        value={folderId}
        drag={reordering ? false : "y"}
        dragListener={false}
        dragControls={dragControls}
        dragMomentum={false}
        layout="position"
        onDragStart={handleDragStart}
        onDragEnd={onDragEnd}
      >
        <div
          className={cn(
            "sticky top-0 z-20 bg-sidebar",
            dragging && "shadow-sm"
          )}
        >
          <FolderHeader
            folderId={folderId}
            folderName={folderName}
            folderPath={folderPath}
            count={conversations.length}
            expanded={expanded}
            importing={importing}
            themeColor={themeColor}
            appThemeColor={appThemeColor}
            currentDefaultAgent={currentDefaultAgent}
            availableAgents={availableAgents}
            onToggle={handleToggle}
            onRemoveFromWorkspace={onRemoveFromWorkspace}
            onNewConversation={onNewConversationForFolder}
            onImport={onImport}
            onManageConversations={onManageConversations}
            onChangeColor={onChangeColor}
            onSetDefaultAgent={onSetDefaultAgent}
            onOpenInSystemExplorer={onOpenInSystemExplorer}
            onOpenInTerminal={onOpenInTerminal}
            isDragging={dragging}
            dragControls={dragControls}
            t={t}
          />
        </div>
        {expanded &&
          (conversations.length === 0 ? (
            <div
              className="py-[0.375rem] text-[0.75rem] text-muted-foreground/70"
              style={{
                paddingLeft: "calc(var(--conv-rail-axis) + 0.875rem)",
              }}
            >
              {totalConversationCount === 0
                ? t("emptyFolderHint")
                : t("noUnfinishedConversations")}
            </div>
          ) : (
            conversations.map((conv) => (
              <SidebarConversationCard
                key={`conv-${conv.agent_type}-${conv.id}`}
                conversation={conv}
                isSelected={
                  selectedConversation?.agentType === conv.agent_type &&
                  selectedConversation?.id === conv.id
                }
                isOpenInTab={openTabKeys.has(`${conv.agent_type}:${conv.id}`)}
                timeLabel={formatRelative(
                  sortMode === "updated" ? conv.updated_at : conv.created_at
                )}
                onSelect={onSelect}
                onDoubleClick={onDoubleClick}
                onRename={onRename}
                onDelete={onDelete}
                onStatusChange={onStatusChange}
                onNewConversation={onNewConversation}
              />
            ))
          ))}
      </Reorder.Item>
    </div>
  )
}

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

  const selectedConversation = useMemo(() => {
    const activeTab = tabs.find((tab) => tab.id === activeTabId)
    if (!activeTab || activeTab.conversationId == null) return null
    return {
      id: activeTab.conversationId,
      agentType: activeTab.agentType,
    }
  }, [tabs, activeTabId])

  const openTabKeys = useMemo(() => {
    const set = new Set<string>()
    for (const tab of tabs) {
      if (tab.conversationId != null) {
        set.add(`${tab.agentType}:${tab.conversationId}`)
      }
    }
    return set
  }, [tabs])

  const [importing, setImporting] = useState(false)
  const { sortedTypes: availableAgents } = useSortedAvailableAgents()
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

  const scrollRootRef = useRef<OverlayScrollbarsComponentRef>(null)
  const scrollToActiveRef = useRef<() => void>(() => {})
  const pendingScrollRef = useRef(false)

  const filteredConversations = useMemo(() => {
    if (showCompleted) return conversations
    return conversations.filter((c) => c.status !== "completed")
  }, [conversations, showCompleted])

  const byFolder = useMemo(() => {
    const map = new Map<number, DbConversationSummary[]>()
    for (const conv of filteredConversations) {
      const list = map.get(conv.folder_id)
      if (list) list.push(conv)
      else map.set(conv.folder_id, [conv])
    }
    const comparator =
      sortMode === "updated" ? compareByUpdatedAtDesc : compareByCreatedAtDesc
    for (const list of map.values()) list.sort(comparator)
    return map
  }, [filteredConversations, sortMode])

  const folderTotalCounts = useMemo(() => {
    const map = new Map<number, number>()
    for (const conv of conversations) {
      map.set(conv.folder_id, (map.get(conv.folder_id) ?? 0) + 1)
    }
    return map
  }, [conversations])

  const orderedFolderIds = useMemo(() => {
    const folderIdSet = new Set(folders.map((f) => f.id))
    // During drag we honour the optimistic order so sibling folders shift live
    // as the user hovers over slots. We still filter/append against the source
    // of truth so newly-added or -removed folders don't disappear mid-drag.
    if (dragOrder) {
      const seen = new Set<number>()
      const ids: number[] = []
      for (const id of dragOrder) {
        if (folderIdSet.has(id) && !seen.has(id)) {
          seen.add(id)
          ids.push(id)
        }
      }
      for (const f of folders) {
        if (!seen.has(f.id)) {
          seen.add(f.id)
          ids.push(f.id)
        }
      }
      return ids
    }

    const seen = new Set<number>()
    const ids: number[] = []
    for (const f of folders) {
      if (!seen.has(f.id)) {
        seen.add(f.id)
        ids.push(f.id)
      }
    }
    return ids
  }, [folders, dragOrder])

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
      if (!(folderExpanded[conv.folder_id] ?? true)) {
        setFolderExpanded((prev) => {
          const next = { ...prev, [conv.folder_id]: true }
          saveFolderExpanded(next)
          return next
        })
        pendingScrollRef.current = true
        return
      }
      const root = scrollRootRef.current?.getElement()
      if (!root) return
      const selector = `[data-conv-key="${targetAgent}:${targetId}"]`
      const el = root.querySelector(selector)
      if (el instanceof HTMLElement) {
        el.scrollIntoView({ block: "center", behavior: "smooth" })
      }
    }

    if (pendingScrollRef.current) {
      pendingScrollRef.current = false
      scrollToActiveRef.current()
    }
  }, [selectedConversation, conversations, folderExpanded])

  const toggleFolder = useCallback((folderId: number) => {
    setFolderExpanded((prev) => {
      const next = { ...prev, [folderId]: !(prev[folderId] ?? true) }
      saveFolderExpanded(next)
      return next
    })
  }, [])

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

  const handleSelect = useCallback(
    (id: number, agentType: string) => {
      const conv = conversations.find(
        (c) => c.id === id && c.agent_type === agentType
      )
      if (!conv) return
      openTab(
        conv.folder_id,
        id,
        agentType as Parameters<typeof openTab>[2],
        false
      )
    },
    [openTab, conversations]
  )

  const handleDoubleClick = useCallback(
    (id: number, agentType: string) => {
      const conv = conversations.find(
        (c) => c.id === id && c.agent_type === agentType
      )
      if (!conv) return
      openTab(
        conv.folder_id,
        id,
        agentType as Parameters<typeof openTab>[2],
        true
      )
    },
    [openTab, conversations]
  )

  const handleRename = useCallback(
    async (id: number, newTitle: string) => {
      await updateConversationTitle(id, newTitle)
      refreshConversations()
    },
    [refreshConversations]
  )

  const handleDelete = useCallback(
    async (id: number, agentType: string) => {
      const conv = conversations.find(
        (c) => c.id === id && c.agent_type === agentType
      )
      await deleteConversation(id)
      if (conv) {
        closeConversationTab(
          conv.folder_id,
          id,
          agentType as Parameters<typeof openTab>[2]
        )
      }
      refreshConversations()
    },
    [closeConversationTab, refreshConversations, conversations]
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

  const handleDragStart = useCallback((folderId: number) => {
    setDragging(folderId)
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

  return (
    <div className="relative flex flex-col flex-1 min-h-0">
      {(loading || refreshing) && (
        <div className="absolute top-0 left-0 right-0 flex items-center justify-center py-1 z-10 pointer-events-none">
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
                ref={scrollRootRef}
                className={cn(
                  "h-full min-h-0 px-1.5 pb-1.5",
                  "[overflow-anchor:none]"
                )}
              >
                <Reorder.Group
                  as="div"
                  axis="y"
                  values={orderedFolderIds}
                  onReorder={handleReorder}
                  className="flex flex-col"
                  style={
                    {
                      "--conv-rail-axis": "0.875rem",
                    } as React.CSSProperties
                  }
                >
                  {orderedFolderIds.map((folderId, index) => {
                    const folderEntry = folderIndex.get(folderId)
                    const themeColor = normalizeFolderThemeColor(
                      folderEntry?.color
                    )
                    const folderName = folderEntry?.name ?? String(folderId)
                    const folderPath = folderEntry?.path ?? ""
                    const currentDefaultAgent =
                      folderEntry?.defaultAgentType ?? null
                    const convs = byFolder.get(folderId) ?? []
                    const expanded = folderExpanded[folderId] ?? true
                    const convsWithKey = convs.map((conv) => ({
                      ...conv,
                    }))
                    // Earlier folders get a higher stacking index so their
                    // sticky headers paint above later folders' conversation
                    // cards when scrolled. Framer's `layout` prop sets
                    // `will-change: transform`, which would otherwise trap
                    // each sticky inside its own Reorder.Item.
                    const stackIndex = orderedFolderIds.length - index
                    return (
                      <FolderGroupItem
                        key={folderId}
                        folderId={folderId}
                        folderName={folderName}
                        folderPath={folderPath}
                        conversations={convsWithKey}
                        totalConversationCount={
                          folderTotalCounts.get(folderId) ?? 0
                        }
                        expanded={expanded}
                        importing={importing}
                        reordering={reordering}
                        dragging={dragging === folderId}
                        sortMode={sortMode}
                        selectedConversation={selectedConversation}
                        openTabKeys={openTabKeys}
                        themeColor={themeColor}
                        appThemeColor={appThemeColor}
                        currentDefaultAgent={currentDefaultAgent}
                        availableAgents={availableAgents}
                        darkMode={resolvedTheme === "dark"}
                        onToggle={toggleFolder}
                        onRemoveFromWorkspace={handleRemoveFolder}
                        onNewConversationForFolder={
                          handleNewConversationForFolder
                        }
                        onImport={handleImportForFolder}
                        onManageConversations={handleManageConversations}
                        onChangeColor={handleChangeFolderColor}
                        onSetDefaultAgent={handleChangeFolderDefaultAgent}
                        onOpenInSystemExplorer={
                          handleOpenFolderInSystemExplorer
                        }
                        onOpenInTerminal={handleOpenFolderInTerminal}
                        onSelect={handleSelect}
                        onDoubleClick={handleDoubleClick}
                        onRename={handleRename}
                        onDelete={handleDelete}
                        onStatusChange={handleStatusChange}
                        onNewConversation={handleNewConversation}
                        onDragStart={handleDragStart}
                        onDragEnd={handleDragEnd}
                        stackIndex={stackIndex}
                        t={t}
                      />
                    )
                  })}
                </Reorder.Group>
              </ScrollArea>
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
