"use client"

import {
  type ReactElement,
  type UIEvent,
  useCallback,
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
import { useFolderContext } from "@/contexts/folder-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import {
  getGitBranch,
  gitCommitBranches,
  gitListAllBranches,
  gitLog,
  gitNewBranch,
} from "@/lib/tauri"
import type { GitBranchList, GitLogEntry, GitLogFileChange } from "@/lib/types"
import { toast } from "sonner"
import { toErrorMessage } from "@/lib/app-error"

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
  const [localOpen, setLocalOpen] = useState(true)
  const [remoteOpen, setRemoteOpen] = useState(false)
  const groupedRemoteBranches = useMemo(() => {
    const groups: Record<string, string[]> = {}
    for (const b of branchList.remote) {
      const slashIndex = b.indexOf("/")
      const remoteName = slashIndex > 0 ? b.substring(0, slashIndex) : "origin"
      if (!groups[remoteName]) groups[remoteName] = []
      groups[remoteName].push(b)
    }
    return groups
  }, [branchList.remote])
  const remoteNames = Object.keys(groupedRemoteBranches)
  const hasMultipleRemotes = remoteNames.length > 1

  const handleSelect = (branch: string) => {
    onBranchChange(branch)
    setPopoverOpen(false)
  }

  function renderBranchItem(branch: string, displayName?: string, indent = 0) {
    const isCurrent = branch === selectedBranch
    return (
      <button
        key={branch}
        type="button"
        className={`flex w-full items-center gap-2 rounded-lg py-1.5 text-xs hover:bg-accent hover:text-accent-foreground select-none outline-hidden ${isCurrent ? "bg-accent/50" : ""}`}
        style={{ paddingLeft: `${(indent + 1) * 0.5 + 0.5}rem` }}
        onClick={() => handleSelect(branch)}
      >
        <span className="truncate">{displayName ?? branch}</span>
        {branch === currentBranch && (
          <span className="ml-auto pr-2 text-[10px] text-muted-foreground">
            {t("current")}
          </span>
        )}
      </button>
    )
  }

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
              <Collapsible open={localOpen} onOpenChange={setLocalOpen}>
                <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-xs font-medium hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("localBranches")}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  {branchList.local.map((branch) =>
                    renderBranchItem(branch, undefined, 1)
                  )}
                </CollapsibleContent>
              </Collapsible>
            )}
            {branchList.remote.length > 0 && (
              <Collapsible open={remoteOpen} onOpenChange={setRemoteOpen}>
                <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-xs font-medium hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("remoteBranches")}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  {hasMultipleRemotes
                    ? remoteNames.map((remoteName) => (
                        <Collapsible key={remoteName}>
                          <CollapsibleTrigger className="flex w-full items-center gap-2 rounded-lg py-1.5 pl-5 text-xs hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                            <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                            {remoteName} (
                            {groupedRemoteBranches[remoteName].length})
                          </CollapsibleTrigger>
                          <CollapsibleContent>
                            {groupedRemoteBranches[remoteName].map((branch) =>
                              renderBranchItem(
                                branch,
                                branch.substring(remoteName.length + 1),
                                3
                              )
                            )}
                          </CollapsibleContent>
                        </Collapsible>
                      ))
                    : branchList.remote.map((branch) => {
                        const slashIndex = branch.indexOf("/")
                        const shortName =
                          slashIndex > 0
                            ? branch.substring(slashIndex + 1)
                            : branch
                        return renderBranchItem(branch, shortName, 1)
                      })}
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
  const { folder } = useFolderContext()
  const { openCommitDiff, openFilePreview } = useWorkspaceContext()
  const [entries, setEntries] = useState<GitLogEntry[]>([])
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)
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

  // Fetch branches on mount
  useEffect(() => {
    void refreshBranches()
  }, [refreshBranches])

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
      try {
        const log = await gitLog(folder.path, 100, branch ?? undefined)
        setEntries(log)
        if (inline) {
          const commitHashes = new Set(log.map((entry) => entry.full_hash))
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
        setError(toErrorMessage(e))
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
        description: error instanceof Error ? error.message : String(error),
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

  useEffect(() => {
    void fetchLog()
  }, [fetchLog])

  const handleScroll = useCallback((e: UIEvent<HTMLDivElement>) => {
    const nextScrolled = e.currentTarget.scrollTop > 0
    setScrolled((prev) => (prev === nextScrolled ? prev : nextScrolled))
  }, [])

  if (loading) {
    return (
      <div className="flex flex-col h-full overflow-y-auto p-2">
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
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex flex-col h-full overflow-y-auto p-2">
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
      </div>
    )
  }

  if (entries.length === 0) {
    return (
      <div className="flex flex-col h-full overflow-y-auto p-2">
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
    )
  }

  return (
    <div className="flex flex-col h-full">
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            onScroll={handleScroll}
            className="flex-1 min-h-0 overflow-y-auto p-2 space-y-2"
          >
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
                  </ContextMenuContent>
                </ContextMenu>
              )
            })}
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            onSelect={() => {
              void fetchLog()
            }}
          >
            {tCommon("refresh")}
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
    </div>
  )
}
