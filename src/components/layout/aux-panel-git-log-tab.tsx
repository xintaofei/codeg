"use client"

import {
  type ReactElement,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useState,
} from "react"
import { useTranslations } from "next-intl"
import {
  ChevronDown,
  ChevronRight,
  ChevronsDownUp,
  ChevronsUpDown,
  CircleHelp,
  CloudCheck,
  CloudOff,
  GitBranch,
  GitBranchPlus,
  GitCompare,
  RefreshCw,
  RotateCcw,
  Upload,
} from "lucide-react"
import {
  Commit,
  CommitActions,
  CommitContent,
  CommitCopyButton,
  CommitFileAdditions,
  CommitFileChanges,
  CommitFileDeletions,
  CommitFileIcon,
  CommitFileInfo,
  CommitFilePath,
  CommitFiles,
  CommitFileStatus,
  CommitHash,
  CommitHeader,
  CommitInfo,
  CommitMessage,
  CommitMetadata,
  CommitTimestamp,
} from "@/components/ai-elements/commit"
import {
  FileTree,
  FileTreeFile,
  FileTreeFolder,
} from "@/components/ai-elements/file-tree"
import { Button } from "@/components/ui/button"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { Skeleton } from "@/components/ui/skeleton"
import { AuxPanelNoFolderEmpty } from "@/components/layout/aux-panel-no-folder-empty"
import { subscribe } from "@/lib/platform"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useWorkspaceActions } from "@/contexts/workspace-context"
import { useWorkspaceStateStore } from "@/hooks/use-workspace-state-store"
import {
  getGitBranch,
  gitCommitBranches,
  gitListAllBranches,
  gitLog,
  gitNewBranch,
  gitReset,
  openPushWindow,
} from "@/lib/api"
import type {
  GitBranchList,
  GitLogEntry,
  GitLogFileChange,
  GitResetMode,
} from "@/lib/types"
import { toast } from "sonner"
import { isNotAGitRepoError, toErrorMessage } from "@/lib/app-error"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  buildBranchTree,
  buildRemoteBranchSections,
  containsBranch,
  expandedKeysForBranch,
  localBranchItems,
  sectionKey,
  type BranchTreeLeaf,
} from "@/lib/branch-tree"
import {
  BranchTreeCollapsible,
  branchRowPaddingLeft,
} from "@/components/layout/branch-tree-collapsible"
import { useBranchTreeExpansion } from "@/hooks/use-branch-tree-expansion"

// Local section open by default; Remote stays collapsed until expanded or seeded.
const SIDEBAR_DEFAULT_EXPANDED = [sectionKey("local")]

const emitEvent = async (event: string, payload?: unknown) => {
  try {
    const { emit } = await import("@tauri-apps/api/event")
    await emit(event, payload)
  } catch {
    // not in Tauri
  }
}

function formatRelativeTime(
  dateStr: string,
  t: (
    key:
      | "time.monthsAgo"
      | "time.daysAgo"
      | "time.hoursAgo"
      | "time.minsAgo"
      | "time.justNow",
    values?: { count: number }
  ) => string
): string {
  const date = new Date(dateStr)
  if (Number.isNaN(date.getTime())) return dateStr

  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffMin = Math.floor(diffMs / 60_000)
  const diffHour = Math.floor(diffMin / 60)
  const diffDay = Math.floor(diffHour / 24)

  if (diffDay > 30) {
    const diffMonth = Math.floor(diffDay / 30)
    return t("time.monthsAgo", { count: diffMonth })
  }
  if (diffDay > 0) return t("time.daysAgo", { count: diffDay })
  if (diffHour > 0) return t("time.hoursAgo", { count: diffHour })
  if (diffMin > 0) return t("time.minsAgo", { count: diffMin })
  return t("time.justNow", { count: 0 })
}

function parseDate(dateStr: string): Date | null {
  const date = new Date(dateStr)
  return Number.isNaN(date.getTime()) ? null : date
}

function filterRecordByCommitHashes<T>(
  record: Record<string, T>,
  hashes: Set<string>
): Record<string, T> {
  const next: Record<string, T> = {}
  for (const [key, value] of Object.entries(record)) {
    if (hashes.has(key)) {
      next[key] = value
    }
  }
  return next
}

function mapFileStatus(
  status: string
): "added" | "modified" | "deleted" | "renamed" {
  switch (status.toUpperCase().charAt(0)) {
    case "A":
      return "added"
    case "D":
      return "deleted"
    case "R":
      return "renamed"
    default:
      return "modified"
  }
}

function getPushStatusMeta(
  pushed: boolean | null,
  labels: {
    pushed: string
    notPushed: string
    unknown: string
  }
): {
  label: string
  icon: typeof CloudCheck
  className: string
} {
  if (pushed === true) {
    return {
      label: labels.pushed,
      icon: CloudCheck,
      className: "text-emerald-500",
    }
  }

  if (pushed === false) {
    return {
      label: labels.notPushed,
      icon: CloudOff,
      className: "text-amber-500",
    }
  }

  return {
    label: labels.unknown,
    icon: CircleHelp,
    className: "text-muted-foreground",
  }
}

type CommitFileTreeDirNode = {
  kind: "dir"
  name: string
  path: string
  children: CommitFileTreeNode[]
  fileCount: number
}

type CommitFileTreeFileNode = {
  kind: "file"
  name: string
  path: string
  change: GitLogFileChange
}

type CommitFileTreeNode = CommitFileTreeDirNode | CommitFileTreeFileNode

interface CommitBranchTarget {
  fullHash: string
  shortHash: string
}

interface CommitResetTarget {
  fullHash: string
  shortHash: string
  message: string
}

interface MutableCommitFileTreeDirNode {
  kind: "dir"
  name: string
  path: string
  children: Map<string, MutableCommitFileTreeDirNode | CommitFileTreeFileNode>
}

function normalizePathSegments(path: string): string[] {
  const normalized = path.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "")
  if (!normalized) return []
  return normalized.split("/").filter(Boolean)
}

function toSortedTreeNodes(
  dir: MutableCommitFileTreeDirNode
): CommitFileTreeNode[] {
  return Array.from(dir.children.values())
    .map<CommitFileTreeNode>((node) => {
      if (node.kind === "file") return node
      return {
        kind: "dir" as const,
        fileCount: 0,
        name: node.name,
        path: node.path,
        children: toSortedTreeNodes(node),
      }
    })
    .sort((a, b) => {
      if (a.kind !== b.kind) return a.kind === "dir" ? -1 : 1
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
    })
}

function compressAndAnnotateDir(
  node: CommitFileTreeDirNode
): CommitFileTreeDirNode {
  let compressedChildren: CommitFileTreeNode[] = node.children.map((child) => {
    if (child.kind === "file") return child
    return compressAndAnnotateDir(child)
  })

  let fileCount = compressedChildren.reduce((count, child) => {
    if (child.kind === "file") return count + 1
    return count + child.fileCount
  }, 0)

  let nextNode: CommitFileTreeDirNode = {
    ...node,
    children: compressedChildren,
    fileCount,
  }

  // Merge "dir/dir/dir" chains where each directory only has one directory child.
  while (
    nextNode.children.length === 1 &&
    nextNode.children[0].kind === "dir"
  ) {
    const onlyChild = nextNode.children[0]
    nextNode = {
      kind: "dir",
      name: `${nextNode.name}/${onlyChild.name}`,
      path: onlyChild.path,
      children: onlyChild.children,
      fileCount: onlyChild.fileCount,
    }
  }

  compressedChildren = nextNode.children
  fileCount = compressedChildren.reduce((count, child) => {
    if (child.kind === "file") return count + 1
    return count + child.fileCount
  }, 0)

  return {
    ...nextNode,
    children: compressedChildren,
    fileCount,
  }
}

function buildCommitFileTree(files: GitLogFileChange[]): CommitFileTreeNode[] {
  const root: MutableCommitFileTreeDirNode = {
    kind: "dir",
    name: "",
    path: "",
    children: new Map(),
  }

  for (const change of files) {
    const segments = normalizePathSegments(change.path)
    if (segments.length === 0) continue

    let current = root
    for (const [index, segment] of segments.entries()) {
      const nodePath = segments.slice(0, index + 1).join("/")
      const isLeaf = index === segments.length - 1

      if (isLeaf) {
        current.children.set(`file:${nodePath}`, {
          kind: "file",
          name: segment,
          path: nodePath,
          change,
        })
        continue
      }

      const dirKey = `dir:${nodePath}`
      const existing = current.children.get(dirKey)
      if (existing && existing.kind === "dir") {
        current = existing
        continue
      }

      const nextDir: MutableCommitFileTreeDirNode = {
        kind: "dir",
        name: segment,
        path: nodePath,
        children: new Map(),
      }
      current.children.set(dirKey, nextDir)
      current = nextDir
    }
  }

  const sortedNodes = toSortedTreeNodes(root)
  return sortedNodes.map((node) => {
    if (node.kind === "file") return node
    return compressAndAnnotateDir(node)
  })
}

function collectExpandedDirectoryPaths(
  nodes: CommitFileTreeNode[],
  expanded = new Set<string>()
): Set<string> {
  for (const node of nodes) {
    if (node.kind !== "dir") continue
    expanded.add(node.path)
    collectExpandedDirectoryPaths(node.children, expanded)
  }
  return expanded
}

function CommitFilesTree({
  commitHash,
  files,
  folderName,
  onOpenCommitDiff,
  onOpenFilePreview,
}: {
  commitHash: string
  files: GitLogFileChange[]
  folderName: string
  onOpenCommitDiff: (
    commit: string,
    path?: string,
    description?: string
  ) => void
  onOpenFilePreview: (path: string) => void
}) {
  const t = useTranslations("Folder.gitLogTab")
  const tCommon = useTranslations("Folder.common")
  const rootPath = "__commit_file_tree_root__"
  const treeNodes = useMemo(() => buildCommitFileTree(files), [files])
  const allDirectoryPaths = useMemo(() => {
    const paths = collectExpandedDirectoryPaths(treeNodes)
    paths.add(rootPath)
    return paths
  }, [treeNodes])
  const [expandedPaths, setExpandedPaths] =
    useState<Set<string>>(allDirectoryPaths)

  useEffect(() => {
    setExpandedPaths(allDirectoryPaths)
  }, [allDirectoryPaths])

  const canExpandAll = useMemo(() => {
    if (allDirectoryPaths.size === 0) return false
    for (const path of allDirectoryPaths) {
      if (!expandedPaths.has(path)) return true
    }
    return false
  }, [allDirectoryPaths, expandedPaths])

  const canCollapseAll = expandedPaths.size > 0

  const toggleExpanded = useCallback(() => {
    if (canExpandAll) {
      setExpandedPaths(new Set(allDirectoryPaths))
      return
    }
    setExpandedPaths(new Set())
  }, [allDirectoryPaths, canExpandAll])

  const renderNode = (node: CommitFileTreeNode): ReactElement => {
    if (node.kind === "dir") {
      return (
        <FileTreeFolder
          key={node.path}
          path={node.path}
          name={node.name}
          suffix={`(${node.fileCount})`}
          suffixClassName="text-muted-foreground/45"
          title={node.path}
        >
          {node.children.map(renderNode)}
        </FileTreeFolder>
      )
    }

    const file = node.change
    return (
      <ContextMenu key={`${commitHash}:${file.path}`}>
        <ContextMenuTrigger>
          <FileTreeFile
            className="w-full min-w-0 cursor-pointer"
            name={node.name}
            onClick={() => {
              void onOpenCommitDiff(commitHash, file.path)
            }}
            path={node.path}
            title={file.path}
          >
            <>
              <span className="size-4 shrink-0" />
              <CommitFileInfo className="flex-1 min-w-0 gap-1.5">
                <CommitFileStatus status={mapFileStatus(file.status)}>
                  {file.status}
                </CommitFileStatus>
                <CommitFileIcon />
                <CommitFilePath title={file.path}>{node.name}</CommitFilePath>
              </CommitFileInfo>
              <CommitFileChanges>
                <CommitFileAdditions count={file.additions} />
                <CommitFileDeletions count={file.deletions} />
              </CommitFileChanges>
            </>
          </FileTreeFile>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            onSelect={() => {
              void onOpenCommitDiff(commitHash, file.path)
            }}
          >
            {tCommon("viewDiff")}
          </ContextMenuItem>
          <ContextMenuItem
            onSelect={() => {
              void onOpenFilePreview(file.path)
            }}
          >
            {tCommon("openFile")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    )
  }

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] text-muted-foreground">{t("filesTitle")}</p>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon"
            className="size-5"
            onClick={toggleExpanded}
            disabled={!canExpandAll && !canCollapseAll}
            title={canExpandAll ? t("expandAllFiles") : t("collapseAllFiles")}
            aria-label={
              canExpandAll ? t("expandAllFiles") : t("collapseAllFiles")
            }
          >
            {canExpandAll ? (
              <ChevronsUpDown className="size-3.5" />
            ) : (
              <ChevronsDownUp className="size-3.5" />
            )}
          </Button>
        </div>
      </div>
      <CommitFiles>
        <FileTree
          className="max-h-[32rem] overflow-auto rounded-md border-border/60 bg-transparent text-xs [&>div]:p-1"
          expanded={expandedPaths}
          onExpandedChange={setExpandedPaths}
        >
          <FileTreeFolder
            path={rootPath}
            name={folderName}
            suffix={`(${files.length})`}
            suffixClassName="text-muted-foreground/45"
            title={folderName}
          >
            {treeNodes.map(renderNode)}
          </FileTreeFolder>
        </FileTree>
      </CommitFiles>
    </div>
  )
}

function BranchSelector({
  branchList,
  currentBranch,
  selectedBranch,
  onBranchChange,
  onRefresh,
  refreshing,
}: {
  branchList: GitBranchList
  currentBranch: string | null
  selectedBranch: string | null
  onBranchChange: (branch: string) => void
  onRefresh: () => void
  refreshing: boolean
}) {
  const t = useTranslations("Folder.gitLogTab.branchSelector")
  const [popoverOpen, setPopoverOpen] = useState(false)

  const localNodes = useMemo(
    () => buildBranchTree(localBranchItems(branchList.local), "local"),
    [branchList.local]
  )
  const remoteSections = useMemo(
    () => buildRemoteBranchSections(branchList.remote),
    [branchList.remote]
  )

  // Auto-expand the section + prefix groups leading to the selected (or current)
  // branch when the popover opens.
  const seedKeys = useMemo(() => {
    const target = selectedBranch ?? currentBranch
    if (!target) return []
    if (containsBranch(localNodes, target)) {
      return [sectionKey("local"), ...expandedKeysForBranch(localNodes, target)]
    }
    for (const section of remoteSections) {
      if (containsBranch(section.nodes, target)) {
        const keys = [
          sectionKey("remote"),
          ...expandedKeysForBranch(section.nodes, target),
        ]
        if (section.key) keys.push(section.key)
        return keys
      }
    }
    return []
  }, [selectedBranch, currentBranch, localNodes, remoteSections])

  const { expanded, isExpanded, toggle } = useBranchTreeExpansion(
    popoverOpen,
    seedKeys,
    SIDEBAR_DEFAULT_EXPANDED
  )

  const handleSelect = (branch: string) => {
    onBranchChange(branch)
    setPopoverOpen(false)
  }

  const renderLeaf = (leaf: BranchTreeLeaf, depth: number) => {
    const isSelected = leaf.fullName === selectedBranch
    return (
      <button
        key={leaf.key}
        type="button"
        className={`flex w-full items-center gap-2 rounded-lg py-1.5 text-xs hover:bg-accent hover:text-accent-foreground select-none outline-hidden ${isSelected ? "bg-accent/50" : ""}`}
        style={{ paddingLeft: branchRowPaddingLeft("sidebar", depth) }}
        onClick={() => handleSelect(leaf.fullName)}
        title={leaf.fullName}
      >
        <span className="min-w-0 flex-1 truncate text-left">{leaf.label}</span>
        {leaf.fullName === currentBranch && (
          <span className="shrink-0 pr-2 text-[10px] text-muted-foreground">
            {t("current")}
          </span>
        )}
      </button>
    )
  }

  const localSectionKey = sectionKey("local")
  const remoteSectionKey = sectionKey("remote")

  return (
    <div className="flex items-center gap-1">
      <Popover open={popoverOpen} onOpenChange={setPopoverOpen}>
        <PopoverTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            className="cursor-pointer flex-1 w-full text-xs bg-input/30 hover:bg-input/50 justify-start gap-1.5"
          >
            <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">
              {selectedBranch || t("selectBranchPlaceholder")}
            </span>
            <ChevronDown className="ml-auto h-3 w-3 shrink-0 opacity-50" />
          </Button>
        </PopoverTrigger>
        <PopoverContent
          className="w-64 p-1"
          side="bottom"
          align="start"
          sideOffset={4}
        >
          <div className="max-h-72 overflow-y-auto">
            {branchList.local.length > 0 && (
              <Collapsible
                open={isExpanded(localSectionKey)}
                onOpenChange={() => toggle(localSectionKey)}
              >
                <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-xs font-medium hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("localBranches")}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  <BranchTreeCollapsible
                    nodes={localNodes}
                    depth={0}
                    expanded={expanded}
                    onToggle={toggle}
                    renderLeaf={renderLeaf}
                    variant="sidebar"
                  />
                </CollapsibleContent>
              </Collapsible>
            )}
            {branchList.remote.length > 0 && (
              <Collapsible
                open={isExpanded(remoteSectionKey)}
                onOpenChange={() => toggle(remoteSectionKey)}
              >
                <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-xs font-medium hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("remoteBranches")}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  {remoteSections.map((section) =>
                    section.remoteName == null ? (
                      <BranchTreeCollapsible
                        key="remote-single"
                        nodes={section.nodes}
                        depth={0}
                        expanded={expanded}
                        onToggle={toggle}
                        renderLeaf={renderLeaf}
                        variant="sidebar"
                      />
                    ) : (
                      <Collapsible
                        key={section.key}
                        open={isExpanded(section.key)}
                        onOpenChange={() => toggle(section.key)}
                      >
                        <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg py-1.5 pl-5 text-xs hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                          <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                          {section.remoteName} ({section.count})
                        </CollapsibleTrigger>
                        <CollapsibleContent>
                          <BranchTreeCollapsible
                            nodes={section.nodes}
                            depth={1}
                            expanded={expanded}
                            onToggle={toggle}
                            renderLeaf={renderLeaf}
                            variant="sidebar"
                          />
                        </CollapsibleContent>
                      </Collapsible>
                    )
                  )}
                </CollapsibleContent>
              </Collapsible>
            )}
          </div>
        </PopoverContent>
      </Popover>
      <Button
        variant="outline"
        size="icon"
        className="h-8 w-8 shrink-0 rounded-full"
        onClick={onRefresh}
        disabled={refreshing}
        title={t("refreshCommitHistory")}
        aria-label={t("refreshCommitHistory")}
      >
        <RefreshCw className={`size-3.5 ${refreshing ? "animate-spin" : ""}`} />
      </Button>
    </div>
  )
}

export function GitLogTab() {
  const t = useTranslations("Folder.gitLogTab")
  const tCommon = useTranslations("Folder.common")
  // Defer the folder so a cross-folder conversation-tab switch commits first and
  // this tab's git-log refetch + commit-row render runs in a non-blocking
  // transition a frame later instead of janking the switch (see the file-tree /
  // changes tabs). Path-keyed effects ride the deferred folder.
  const { activeFolder } = useActiveFolder()
  const folder = useDeferredValue(activeFolder)
  // True while the deferred render lags a cross-folder switch. During that gap we
  // render the loading skeleton (below) instead of the PREVIOUS folder's commit
  // rows: besides keeping the heavy rebuild off the switch commit, unmounting
  // those rows closes any open row ContextMenu (which portals outside this
  // subtree) so a click can't route a commit diff / file opener to the NEW active
  // folder (openers default to it when folderId is omitted — see
  // resolveTargetFolder). Clears the instant the deferred render catches up.
  const folderStale = activeFolder?.id !== folder?.id
  const { openCommitDiff, openFilePreview } = useWorkspaceActions()
  const workspaceState = useWorkspaceStateStore(folder?.path ?? null)
  const isGitRepo = workspaceState.isGitRepo
  const [entries, setEntries] = useState<GitLogEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notAGitRepo, setNotAGitRepo] = useState(false)
  const [scrolled, setScrolled] = useState(false)
  const [openByCommit, setOpenByCommit] = useState<Record<string, boolean>>({})
  const [branchesByCommit, setBranchesByCommit] = useState<
    Record<string, string[]>
  >({})
  const [branchesLoading, setBranchesLoading] = useState<
    Record<string, boolean>
  >({})
  const [branchesError, setBranchesError] = useState<Record<string, string>>({})

  // Branch filter state
  const [branchList, setBranchList] = useState<GitBranchList>({
    local: [],
    remote: [],
    worktree_branches: [],
  })
  const [currentBranch, setCurrentBranch] = useState<string | null>(null)
  const [selectedBranch, setSelectedBranch] = useState<string | null>(null)
  const [newBranchTarget, setNewBranchTarget] =
    useState<CommitBranchTarget | null>(null)
  const [newBranchName, setNewBranchName] = useState("")
  const [creatingBranch, setCreatingBranch] = useState(false)
  const [resetTarget, setResetTarget] = useState<CommitResetTarget | null>(null)
  const [resetMode, setResetMode] = useState<GitResetMode>("mixed")
  const [resetting, setResetting] = useState(false)

  // Close the create-branch / reset dialogs the instant the ACTIVE folder
  // changes (keyed on the live id, not the deferred `folder`) so a dialog opened
  // under the previous folder can't create-branch / reset against the new one
  // after the deferred render settles and it re-mounts.
  useEffect(() => {
    setNewBranchTarget(null)
    setResetTarget(null)
  }, [activeFolder?.id])

  const hasBranches =
    branchList.local.length > 0 || branchList.remote.length > 0
  const pushStatusLabels = useMemo(
    () => ({
      pushed: t("pushStatus.pushed"),
      notPushed: t("pushStatus.notPushed"),
      unknown: t("pushStatus.unknown"),
    }),
    [t]
  )
  const folderName = useMemo(() => {
    const path = folder?.path ?? ""
    const parts = path.split(/[\\/]/).filter(Boolean)
    return (parts[parts.length - 1] ?? path) || t("workspace")
  }, [folder?.path, t])

  const handleBranchChange = useCallback((branch: string) => {
    setSelectedBranch(branch)
  }, [])

  const refreshBranches = useCallback(
    async (nextSelectedBranch?: string | null) => {
      if (!folder?.path) return
      try {
        const [allBranches, current] = await Promise.all([
          gitListAllBranches(folder.path),
          getGitBranch(folder.path),
        ])
        setBranchList(allBranches)
        setCurrentBranch(current)
        setSelectedBranch(nextSelectedBranch ?? current)
      } catch {
        // Silently ignore — branches dropdown won't appear
      }
    },
    [folder?.path]
  )

  // Fetch branches on mount and when git presence flips — the preflight
  // check in `git_list_all_branches` would short-circuit on non-git folders
  // anyway, but skipping the call saves an unnecessary round trip.
  useEffect(() => {
    if (!isGitRepo || notAGitRepo) return
    void refreshBranches()
  }, [isGitRepo, notAGitRepo, refreshBranches])

  const fetchCommitBranches = useCallback(
    async (fullHash: string) => {
      if (!folder?.path) return
      if (branchesByCommit[fullHash] || branchesLoading[fullHash]) return

      setBranchesLoading((prev) => ({ ...prev, [fullHash]: true }))
      setBranchesError((prev) => {
        if (!prev[fullHash]) return prev
        const next = { ...prev }
        delete next[fullHash]
        return next
      })

      try {
        const branches = await gitCommitBranches(folder.path, fullHash)
        setBranchesByCommit((prev) => ({ ...prev, [fullHash]: branches }))
      } catch (e) {
        setBranchesError((prev) => ({
          ...prev,
          [fullHash]: toErrorMessage(e),
        }))
      } finally {
        setBranchesLoading((prev) => ({ ...prev, [fullHash]: false }))
      }
    },
    [branchesByCommit, branchesLoading, folder?.path]
  )

  const fetchLog = useCallback(
    async (options?: { inline?: boolean; branch?: string | null }) => {
      const inline = options?.inline ?? false
      const branch = options?.branch ?? selectedBranch
      if (!folder?.path) return
      if (inline) {
        setRefreshing(true)
      } else {
        setLoading(true)
        setOpenByCommit({})
        setBranchesByCommit({})
        setBranchesLoading({})
        setBranchesError({})
      }
      setError(null)
      setNotAGitRepo(false)
      try {
        const result = await gitLog(folder.path, 100, branch ?? undefined)
        setEntries(result.entries)
        if (inline) {
          const commitHashes = new Set(
            result.entries.map((entry) => entry.full_hash)
          )
          setOpenByCommit((prev) =>
            filterRecordByCommitHashes(prev, commitHashes)
          )
          setBranchesByCommit((prev) =>
            filterRecordByCommitHashes(prev, commitHashes)
          )
          setBranchesLoading((prev) =>
            filterRecordByCommitHashes(prev, commitHashes)
          )
          setBranchesError((prev) =>
            filterRecordByCommitHashes(prev, commitHashes)
          )
        }
      } catch (e) {
        if (isNotAGitRepoError(e)) {
          setNotAGitRepo(true)
          // Workspace state will flip isGitRepo within the next watch flush;
          // clear entries so stale log data does not linger while we wait.
          setEntries([])
        } else {
          setError(toErrorMessage(e))
        }
      } finally {
        if (inline) {
          setRefreshing(false)
        } else {
          setLoading(false)
        }
      }
    },
    [folder?.path, selectedBranch]
  )

  useEffect(() => {
    setNotAGitRepo(false)
  }, [folder?.path])

  const handleRefresh = useCallback(() => {
    void fetchLog({ inline: true })
  }, [fetchLog])

  const handleOpenNewBranchDialog = useCallback((entry: GitLogEntry) => {
    setNewBranchName("")
    setNewBranchTarget({
      fullHash: entry.full_hash,
      shortHash: entry.hash,
    })
  }, [])

  const handleCreateBranchFromCommit = useCallback(async () => {
    const name = newBranchName.trim()
    if (!folder?.path || !newBranchTarget || !name || creatingBranch) return

    setCreatingBranch(true)
    try {
      await gitNewBranch(folder.path, name, newBranchTarget.fullHash)
      setNewBranchTarget(null)
      setNewBranchName("")
      await refreshBranches(name)
      toast.success(t("toasts.createdAndSwitchedNewBranch"), {
        description: t("toasts.newBranchFromCommit", {
          name,
          shortHash: newBranchTarget.shortHash,
        }),
      })
    } catch (error) {
      toast.error(t("toasts.createBranchFailed"), {
        description: toErrorMessage(error),
      })
    } finally {
      setCreatingBranch(false)
    }
  }, [
    creatingBranch,
    folder?.path,
    newBranchName,
    newBranchTarget,
    refreshBranches,
    t,
  ])

  const isResetAllowed = useMemo(() => {
    return (
      !!currentBranch && !!selectedBranch && currentBranch === selectedBranch
    )
  }, [currentBranch, selectedBranch])

  const handleOpenResetDialog = useCallback((entry: GitLogEntry) => {
    setResetMode("mixed")
    setResetTarget({
      fullHash: entry.full_hash,
      shortHash: entry.hash,
      message: entry.message,
    })
  }, [])

  const handleResetCurrentBranchToCommit = useCallback(async () => {
    if (
      !folder?.path ||
      !currentBranch ||
      !resetTarget ||
      !isResetAllowed ||
      resetting
    ) {
      return
    }

    setResetting(true)
    try {
      await gitReset(folder.path, resetTarget.fullHash, resetMode)
      await refreshBranches(currentBranch)
      await fetchLog({ inline: true })
      if (folder.id) {
        void emitEvent("folder://git-branch-changed", {
          folder_id: folder.id,
        })
      }
      toast.success(t("toasts.resetSuccess"), {
        description: t("toasts.resetSuccessDescription", {
          branch: currentBranch,
          shortHash: resetTarget.shortHash,
          mode: t(`dialogs.reset.modes.${resetMode}.label`),
        }),
      })
      setResetTarget(null)
      setResetMode("mixed")
    } catch (error) {
      toast.error(t("toasts.resetFailed"), {
        description: toErrorMessage(error),
      })
    } finally {
      setResetting(false)
    }
  }, [
    currentBranch,
    fetchLog,
    folder?.path,
    folder?.id,
    isResetAllowed,
    refreshBranches,
    resetMode,
    resetTarget,
    resetting,
    t,
  ])

  useEffect(() => {
    if (!folder?.path) return
    // Only fetch when workspaceState says we're in a git repo. When it flips
    // (user runs `git init` / deletes `.git` externally), this effect re-runs
    // and either re-fetches or clears the log to stay aligned with the other
    // workspace panels.
    if (!isGitRepo) {
      setNotAGitRepo(false)
      setEntries([])
      setError(null)
      setLoading(false)
      return
    }
    void fetchLog()
  }, [folder?.path, isGitRepo, fetchLog])

  // Refresh branches & log on branch change, commit, or push
  useEffect(() => {
    if (!folder) return

    const events = [
      "folder://git-branch-changed",
      "folder://git-commit-succeeded",
      "folder://git-push-succeeded",
    ] as const

    const unlistens: ((() => void) | null)[] = events.map(() => null)

    events.forEach((eventName, i) => {
      subscribe<{ folder_id: number }>(eventName, (payload) => {
        if (payload.folder_id !== folder.id) return
        void refreshBranches()
        void fetchLog({ inline: true })
      })
        .then((fn) => {
          unlistens[i] = fn
        })
        .catch((err) => {
          console.error(`[GitLogTab] failed to listen ${eventName}:`, err)
        })
    })

    return () => {
      events.forEach((_eventName, i) => {
        unlistens[i]?.()
      })
    }
  }, [folder, refreshBranches, fetchLog])

  const handleScroll = useCallback((e: Event) => {
    const target = e.target as HTMLElement
    const nextScrolled = target.scrollTop > 0
    setScrolled((prev) => (prev === nextScrolled ? prev : nextScrolled))
  }, [])

  if (!folder) {
    return <AuxPanelNoFolderEmpty />
  }

  // `folderStale`: skeleton over the previous folder's commits while the deferred
  // render catches up to the switch (see the declaration above).
  if (loading || folderStale) {
    return (
      <ScrollArea className="h-full px-3 py-3">
        {hasBranches && (
          <BranchSelector
            branchList={branchList}
            currentBranch={currentBranch}
            selectedBranch={selectedBranch}
            onBranchChange={handleBranchChange}
            onRefresh={handleRefresh}
            refreshing={loading || refreshing}
          />
        )}
        <div className="space-y-3 pt-1">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="space-y-1">
              <Skeleton className="h-3 w-16" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-3 w-24" />
            </div>
          ))}
        </div>
      </ScrollArea>
    )
  }

  if (!isGitRepo || notAGitRepo) {
    return (
      <ScrollArea className="h-full px-3 py-3">
        <div className="flex flex-col items-center justify-center min-h-full gap-1 p-6 text-center">
          <GitBranch className="size-5 text-muted-foreground/60" aria-hidden />
          <p className="text-sm font-medium">{t("notAGitRepoTitle")}</p>
          <p className="text-xs text-muted-foreground">
            {t("notAGitRepoHint")}
          </p>
          {isGitRepo && (
            <Button
              variant="ghost"
              size="xs"
              className="mt-2"
              onClick={() => {
                void fetchLog()
              }}
            >
              {t("retry")}
            </Button>
          )}
        </div>
      </ScrollArea>
    )
  }

  if (error) {
    return (
      <ScrollArea className="h-full px-3 py-3">
        {hasBranches && (
          <BranchSelector
            branchList={branchList}
            currentBranch={currentBranch}
            selectedBranch={selectedBranch}
            onBranchChange={handleBranchChange}
            onRefresh={handleRefresh}
            refreshing={loading || refreshing}
          />
        )}
        <div className="pt-1 text-xs text-destructive">
          <p>{error}</p>
          <Button
            variant="ghost"
            size="xs"
            className="mt-2"
            onClick={() => {
              void fetchLog()
            }}
          >
            {t("retry")}
          </Button>
        </div>
      </ScrollArea>
    )
  }

  if (entries.length === 0) {
    return (
      <ScrollArea className="h-full px-3 py-3">
        <div className="flex flex-col min-h-full">
          {hasBranches && (
            <BranchSelector
              branchList={branchList}
              currentBranch={currentBranch}
              selectedBranch={selectedBranch}
              onBranchChange={handleBranchChange}
              onRefresh={handleRefresh}
              refreshing={loading || refreshing}
            />
          )}
          <div className="flex items-center justify-center flex-1 p-4">
            <p className="text-xs text-muted-foreground text-center">
              {t("noCommitsFound")}
            </p>
          </div>
        </div>
      </ScrollArea>
    )
  }

  return (
    <div className="flex flex-col h-full">
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <ScrollArea
            onScroll={handleScroll}
            className="flex-1 min-h-0 px-3 py-3"
          >
            <div className="space-y-3">
              {hasBranches && (
                <div
                  className={`sticky top-0 z-10 rounded-full bg-sidebar/85 supports-[backdrop-filter]:bg-sidebar/70 backdrop-blur ${scrolled ? "p-2 shadow-md" : "p-0"}`}
                >
                  <BranchSelector
                    branchList={branchList}
                    currentBranch={currentBranch}
                    selectedBranch={selectedBranch}
                    onBranchChange={handleBranchChange}
                    onRefresh={handleRefresh}
                    refreshing={loading || refreshing}
                  />
                </div>
              )}
              {entries.map((entry) => {
                const commitKey = entry.full_hash
                const commitDate = parseDate(entry.date)
                const pushStatus = getPushStatusMeta(
                  entry.pushed,
                  pushStatusLabels
                )
                const PushStatusIcon = pushStatus.icon
                const commitBranches = branchesByCommit[commitKey]
                const isBranchLoading = !!branchesLoading[commitKey]
                const branchError = branchesError[commitKey]
                const isOpen = !!openByCommit[commitKey]

                return (
                  <ContextMenu key={entry.full_hash}>
                    <ContextMenuTrigger asChild>
                      <div>
                        <Commit
                          // 开背景图时卡片改磨砂表面（与提问卡片等 frosted 卡一致，透出
                          // 下方背景图），而非纯色 bg-background 黑白块。ws-surface-card 组合式
                          // 无 base 规则，关图沿用 bg-background，零回归。
                          className="ws-surface-card"
                          onOpenChange={(open) => {
                            setOpenByCommit((prev) => ({
                              ...prev,
                              [commitKey]: open,
                            }))
                            if (open) {
                              void fetchCommitBranches(commitKey)
                            }
                          }}
                          open={isOpen}
                        >
                          <CommitHeader>
                            <CommitInfo className="min-w-0">
                              <CommitMessage className="line-clamp-1 leading-snug">
                                {entry.message}
                              </CommitMessage>
                              <CommitMetadata className="mt-1 min-w-0 flex items-center gap-1.5">
                                <span
                                  className="inline-flex shrink-0"
                                  title={pushStatus.label}
                                  aria-label={pushStatus.label}
                                >
                                  <PushStatusIcon
                                    className={pushStatus.className}
                                    size={12}
                                  />
                                </span>
                                <span className="truncate">{entry.author}</span>
                                <CommitTimestamp
                                  className="shrink-0"
                                  date={commitDate ?? new Date()}
                                >
                                  {formatRelativeTime(entry.date, t)}
                                </CommitTimestamp>
                                <CommitHash className="text-primary/70">
                                  {entry.hash}
                                </CommitHash>
                              </CommitMetadata>
                            </CommitInfo>
                            <CommitActions className="shrink-0">
                              <Button
                                variant="ghost"
                                size="icon"
                                className="h-6 w-6 text-muted-foreground hover:text-foreground"
                                onClick={() => {
                                  void openCommitDiff(
                                    entry.full_hash,
                                    undefined,
                                    entry.message
                                  )
                                }}
                                title={tCommon("viewDiff")}
                                aria-label={t("viewCommitDiffAria", {
                                  hash: entry.hash,
                                })}
                              >
                                <GitCompare size={14} />
                              </Button>
                            </CommitActions>
                          </CommitHeader>
                          <CommitContent>
                            <div className="space-y-3">
                              <div className="grid grid-cols-[4rem_minmax(0,1fr)] items-center gap-x-2 gap-y-1 text-xs">
                                <span className="text-muted-foreground">
                                  {t("hash")}
                                </span>
                                <span className="group/hash flex items-center gap-1 min-w-0">
                                  <code
                                    className="block min-w-0 flex-1 truncate font-mono"
                                    title={entry.full_hash}
                                  >
                                    {entry.full_hash}
                                  </code>
                                  <CommitCopyButton
                                    aria-label={t("copyFullCommitHashAria", {
                                      hash: entry.full_hash,
                                    })}
                                    className="size-5 shrink-0 opacity-0 transition-opacity group-hover/hash:opacity-100 group-focus-within/hash:opacity-100"
                                    hash={entry.full_hash}
                                    title={t("copyHash")}
                                  />
                                </span>
                                <span className="text-muted-foreground">
                                  {t("author")}
                                </span>
                                <span className="min-w-0 flex items-center gap-1">
                                  <span className="min-w-0 truncate">
                                    {entry.author}
                                  </span>
                                  <span className="shrink-0 text-muted-foreground">
                                    ·
                                  </span>
                                  <time
                                    className="shrink-0"
                                    dateTime={commitDate?.toISOString()}
                                  >
                                    {commitDate
                                      ? commitDate.toLocaleString()
                                      : entry.date}
                                  </time>
                                </span>
                              </div>
                              <div className="group/msg relative rounded-lg border border-border/60 bg-muted/20 p-2.5">
                                <p className="text-xs whitespace-pre-wrap break-words pr-6">
                                  {entry.message}
                                </p>
                                <CommitCopyButton
                                  className="absolute top-1.5 right-1.5 size-5 opacity-0 transition-opacity group-hover/msg:opacity-100 group-focus-within/msg:opacity-100"
                                  hash={entry.message}
                                  title={t("copyMessage")}
                                />
                              </div>
                              {entry.files.length === 0 ? (
                                <div className="space-y-1">
                                  <p className="text-[11px] text-muted-foreground">
                                    {t("filesTitle")}
                                  </p>
                                  <p className="text-xs text-muted-foreground">
                                    {t("noFileChangeDetails")}
                                  </p>
                                </div>
                              ) : (
                                <CommitFilesTree
                                  commitHash={entry.full_hash}
                                  files={entry.files}
                                  folderName={folderName}
                                  onOpenCommitDiff={openCommitDiff}
                                  onOpenFilePreview={openFilePreview}
                                />
                              )}
                              <div className="pt-3 space-y-1">
                                <p className="text-[11px] text-muted-foreground">
                                  {t("branchesTitle")}
                                </p>
                                {isBranchLoading ? (
                                  <p className="text-xs text-muted-foreground">
                                    {t("loadingBranches")}
                                  </p>
                                ) : branchError ? (
                                  <p className="text-xs text-destructive">
                                    {branchError}
                                  </p>
                                ) : commitBranches &&
                                  commitBranches.length > 0 ? (
                                  <div className="flex flex-wrap gap-1">
                                    {commitBranches.map((branch) => (
                                      <span
                                        key={`${commitKey}-${branch}`}
                                        className="rounded-md border border-border px-1.5 py-0.5 text-[10px] text-muted-foreground"
                                        title={branch}
                                      >
                                        {branch}
                                      </span>
                                    ))}
                                  </div>
                                ) : (
                                  <p className="text-xs text-muted-foreground">
                                    {t("noContainingBranches")}
                                  </p>
                                )}
                              </div>
                            </div>
                          </CommitContent>
                        </Commit>
                      </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem
                        onSelect={() => {
                          handleOpenNewBranchDialog(entry)
                        }}
                      >
                        <GitBranchPlus className="h-3.5 w-3.5" />
                        {t("newBranch")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          void openCommitDiff(
                            entry.full_hash,
                            undefined,
                            entry.message
                          )
                        }}
                      >
                        <GitCompare className="h-3.5 w-3.5" />
                        {tCommon("viewDiff")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        disabled={!isResetAllowed}
                        onSelect={() => {
                          handleOpenResetDialog(entry)
                        }}
                      >
                        <RotateCcw className="size-3.5" />
                        {t("resetToHere")}
                      </ContextMenuItem>
                      {!isResetAllowed && (
                        <ContextMenuItem disabled>
                          {t("resetDisabledReasonNotCurrentBranchView")}
                        </ContextMenuItem>
                      )}
                      <ContextMenuItem
                        onSelect={() => {
                          void fetchLog()
                        }}
                      >
                        <RefreshCw className="size-3.5" />
                        {tCommon("refresh")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          if (!folder) return
                          openPushWindow(folder.id).catch((err) => {
                            const msg = toErrorMessage(err)
                            toast.error(t("toasts.openPushWindowFailed"), {
                              description: msg,
                            })
                          })
                        }}
                      >
                        <Upload className="size-3.5" />
                        {tCommon("push")}
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                )
              })}
            </div>
          </ScrollArea>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            onSelect={() => {
              void fetchLog()
            }}
          >
            <RefreshCw className="size-3.5" />
            {tCommon("refresh")}
          </ContextMenuItem>
          <ContextMenuItem
            onSelect={() => {
              if (!folder) return
              openPushWindow(folder.id).catch((err) => {
                const msg = toErrorMessage(err)
                toast.error(t("toasts.openPushWindowFailed"), {
                  description: msg,
                })
              })
            }}
          >
            <Upload className="size-3.5" />
            {tCommon("push")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <Dialog
        open={newBranchTarget !== null}
        onOpenChange={(open) => {
          if (!open && !creatingBranch) {
            setNewBranchTarget(null)
            setNewBranchName("")
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("dialogs.newBranchTitle")}</DialogTitle>
            <DialogDescription>
              {t("dialogs.newBranchDescription", {
                shortHash: newBranchTarget?.shortHash ?? "-",
              })}
            </DialogDescription>
          </DialogHeader>
          <Input
            placeholder={t("dialogs.branchNamePlaceholder")}
            value={newBranchName}
            onChange={(event) => setNewBranchName(event.target.value)}
            onKeyDown={(event) => {
              if (
                event.nativeEvent.isComposing ||
                event.key === "Process" ||
                event.key !== "Enter"
              ) {
                return
              }
              void handleCreateBranchFromCommit()
            }}
            autoFocus
          />
          <DialogFooter>
            <Button
              variant="outline"
              disabled={creatingBranch}
              onClick={() => {
                setNewBranchTarget(null)
                setNewBranchName("")
              }}
            >
              {tCommon("cancel")}
            </Button>
            <Button
              disabled={!newBranchName.trim() || creatingBranch}
              onClick={() => {
                void handleCreateBranchFromCommit()
              }}
            >
              {tCommon("createAndSwitch")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={resetTarget !== null}
        onOpenChange={(open) => {
          if (!open && !resetting) {
            setResetTarget(null)
            setResetMode("mixed")
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("dialogs.reset.title")}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            <div className="grid grid-cols-[4.5rem_minmax(0,1fr)] items-start gap-x-2 gap-y-1 text-xs">
              <span className="text-muted-foreground">
                {t("dialogs.reset.branchLabel")}
              </span>
              <code className="block min-w-0 break-all font-mono">
                {currentBranch ?? "-"}
              </code>
              <span className="text-muted-foreground">
                {t("dialogs.reset.targetLabel")}
              </span>
              <code className="block min-w-0 break-all font-mono">
                {resetTarget?.shortHash ?? "-"}
              </code>
              <span className="text-muted-foreground">
                {t("dialogs.reset.messageLabel")}
              </span>
              <p className="max-h-32 min-w-0 overflow-y-auto whitespace-pre-wrap break-words">
                {resetTarget?.message || "-"}
              </p>
            </div>

            <div className="space-y-2">
              <p className="text-xs text-muted-foreground">
                {t("dialogs.reset.modeLabel")}
              </p>
              <RadioGroup
                value={resetMode}
                onValueChange={(value) => {
                  setResetMode(value as GitResetMode)
                }}
                className="space-y-2"
                disabled={resetting}
              >
                {(["soft", "mixed", "hard", "keep"] as const).map((mode) => {
                  const optionId = `git-reset-mode-${mode}`
                  return (
                    <label
                      key={mode}
                      htmlFor={optionId}
                      className="flex cursor-pointer items-start gap-2 rounded-md border border-border/60 p-2"
                    >
                      <RadioGroupItem
                        id={optionId}
                        value={mode}
                        className="mt-0.5"
                      />
                      <div className="min-w-0">
                        <p className="text-sm font-medium leading-tight">
                          {t(`dialogs.reset.modes.${mode}.label`)}
                        </p>
                        <p className="mt-0.5 whitespace-pre-line text-xs leading-relaxed text-muted-foreground">
                          {t(`dialogs.reset.modes.${mode}.description`)}
                        </p>
                      </div>
                    </label>
                  )
                })}
              </RadioGroup>
            </div>
          </div>
          <DialogFooter>
            <Button
              variant="outline"
              disabled={resetting}
              onClick={() => {
                setResetTarget(null)
                setResetMode("mixed")
              }}
            >
              {tCommon("cancel")}
            </Button>
            <Button
              disabled={resetting || !isResetAllowed || !resetTarget}
              onClick={() => {
                void handleResetCurrentBranchToCommit()
              }}
            >
              {t("dialogs.reset.confirmButton")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
