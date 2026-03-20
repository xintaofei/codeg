"use client"

import {
  type ReactElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { ChevronsDownUp, ChevronsUpDown } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  CommitFileAdditions,
  CommitFileChanges,
  CommitFileDeletions,
  CommitFileIcon,
  CommitFileInfo,
  CommitFilePath,
  CommitFileStatus,
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
import { Skeleton } from "@/components/ui/skeleton"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useFolderContext } from "@/contexts/folder-context"
import { useWorkspaceContext } from "@/contexts/workspace-context"
import {
  deleteFileTreeEntry,
  gitDiff,
  gitAddFiles,
  gitRollbackFile,
  gitStatus,
  openCommitWindow,
  startFileTreeWatch,
  stopFileTreeWatch,
} from "@/lib/tauri"
import { disposeTauriListener } from "@/lib/tauri-listener"
import type { FileTreeChangedEvent, GitStatusEntry } from "@/lib/types"
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

interface WorkingTreeChange {
  path: string
  status: string
  additions: number
  deletions: number
}

interface GitActionTarget {
  kind: "file" | "dir"
  path: string
  name: string
}

type DirectoryGitAction =
  | "add"
  | "rollback"
  | "delete-tracked"
  | "delete-untracked"

interface DirectoryGitCandidateEntry {
  path: string
  status: string
}

type ChangeTreeDirNode = {
  kind: "dir"
  name: string
  path: string
  children: ChangeTreeNode[]
  fileCount: number
}

type ChangeTreeFileNode = {
  kind: "file"
  name: string
  path: string
  change: WorkingTreeChange
}

type ChangeTreeNode = ChangeTreeDirNode | ChangeTreeFileNode

interface MutableChangeTreeDirNode {
  kind: "dir"
  name: string
  path: string
  children: Map<string, MutableChangeTreeDirNode | ChangeTreeFileNode>
}

const TRACKED_ROOT_PATH = "__working_tree_tracked_root__"
const UNTRACKED_ROOT_PATH = "__working_tree_untracked_root__"
const UNTRACKED_STATUS = "??"

type GitFileState =
  | "untracked"
  | "modified"
  | "staged"
  | "conflicted"
  | "deleted"
  | "renamed"

function classifyGitFileState(status: string): GitFileState | null {
  const code = status.trim().toUpperCase()
  if (!code) return null
  if (code === UNTRACKED_STATUS) return "untracked"
  if (code.includes("U")) return "conflicted"
  if (code.includes("R") || code.includes("C")) return "renamed"
  if (code.includes("D")) return "deleted"
  if (code.includes("M") || code.includes("T")) return "modified"
  if (code.includes("A")) return "staged"
  return null
}

function normalizePathSegments(path: string): string[] {
  const normalized = path.replace(/\\/g, "/").replace(/^\/+|\/+$/g, "")
  if (!normalized) return []
  return normalized.split("/").filter(Boolean)
}

function normalizeGitStatusPath(path: string): string {
  const normalized = path.trim()
  const renameSeparator = " -> "
  const renameIndex = normalized.lastIndexOf(renameSeparator)
  if (renameIndex < 0) return normalized
  return normalized.slice(renameIndex + renameSeparator.length).trim()
}

function normalizeComparePath(path: string): string {
  return path.replace(/\\/g, "/").replace(/\/+$/, "")
}

function isPathInDirectory(path: string, directoryPath: string): boolean {
  const normalizedPath = normalizeComparePath(path)
  const normalizedDir = normalizeComparePath(directoryPath)
  if (!normalizedDir) return normalizedPath.length > 0
  return (
    normalizedPath === normalizedDir ||
    normalizedPath.startsWith(`${normalizedDir}/`)
  )
}

function scopeGitStatusEntriesForDirectory(
  entries: GitStatusEntry[],
  directoryPath: string
): DirectoryGitCandidateEntry[] {
  const normalizedDirPath = normalizeComparePath(directoryPath)
  const scopedEntries: DirectoryGitCandidateEntry[] = []
  const dedupByPath = new Set<string>()

  for (const entry of entries) {
    const normalizedPath = normalizeComparePath(
      normalizeGitStatusPath(entry.file)
    )
    if (!normalizedPath) continue
    if (!isPathInDirectory(normalizedPath, normalizedDirPath)) continue
    if (normalizedPath === normalizedDirPath) continue
    if (dedupByPath.has(normalizedPath)) continue
    dedupByPath.add(normalizedPath)
    scopedEntries.push({ path: normalizedPath, status: entry.status })
  }

  return scopedEntries.sort((left, right) =>
    left.path.localeCompare(right.path, undefined, { sensitivity: "base" })
  )
}

function isDeleteAction(action: DirectoryGitAction): boolean {
  return action === "delete-tracked" || action === "delete-untracked"
}

function filterDirectoryGitCandidates(
  entries: DirectoryGitCandidateEntry[],
  action: DirectoryGitAction
): DirectoryGitCandidateEntry[] {
  if (action === "add") {
    return entries.filter((entry) => entry.status.trim().length > 0)
  }

  if (action === "delete-tracked") {
    return entries.filter((entry) => {
      const fileState = classifyGitFileState(entry.status)
      return fileState !== null && fileState !== "untracked"
    })
  }

  if (action === "delete-untracked") {
    return entries.filter((entry) => {
      const fileState = classifyGitFileState(entry.status)
      return fileState === "untracked"
    })
  }

  return entries.filter((entry) => {
    const fileState = classifyGitFileState(entry.status)
    return fileState !== "untracked"
  })
}

function normalizeDiffPath(rawPath: string): string | null {
  const trimmed = rawPath.trim().replace(/^"|"$/g, "")
  if (!trimmed || trimmed === "/dev/null") return null
  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) {
    return trimmed.slice(2).replace(/\\/g, "/")
  }
  return trimmed.replace(/\\/g, "/")
}

function parsePathFromDiffGitLine(line: string): string | null {
  if (!line.startsWith("diff --git ")) return null
  const match = line.match(/^diff --git\s+(.+?)\s+(.+)$/)
  if (!match) return null
  return normalizeDiffPath(match[2]) ?? normalizeDiffPath(match[1])
}

function parseDiffStatsMap(
  diffText: string
): Map<string, { additions: number; deletions: number }> {
  const stats = new Map<string, { additions: number; deletions: number }>()
  let currentPath: string | null = null

  for (const line of diffText.split("\n")) {
    const nextPath = parsePathFromDiffGitLine(line)
    if (nextPath) {
      currentPath = nextPath
      if (!stats.has(currentPath)) {
        stats.set(currentPath, { additions: 0, deletions: 0 })
      }
      continue
    }

    if (!currentPath) continue
    const current = stats.get(currentPath)
    if (!current) continue

    if (line.startsWith("+") && !line.startsWith("+++")) {
      current.additions += 1
    } else if (line.startsWith("-") && !line.startsWith("---")) {
      current.deletions += 1
    }
  }

  return stats
}

function toSortedTreeNodes(dir: MutableChangeTreeDirNode): ChangeTreeNode[] {
  return Array.from(dir.children.values())
    .map<ChangeTreeNode>((node) => {
      if (node.kind === "file") return node
      return {
        kind: "dir",
        fileCount: 0,
        name: node.name,
        path: node.path,
        children: toSortedTreeNodes(node),
      }
    })
    .sort((left, right) => {
      if (left.kind !== right.kind) return left.kind === "dir" ? -1 : 1
      return left.name.localeCompare(right.name, undefined, {
        sensitivity: "base",
      })
    })
}

function compressAndAnnotateDir(node: ChangeTreeDirNode): ChangeTreeDirNode {
  let compressedChildren = node.children.map((child) => {
    if (child.kind === "file") return child
    return compressAndAnnotateDir(child)
  })

  let fileCount = compressedChildren.reduce((count, child) => {
    if (child.kind === "file") return count + 1
    return count + child.fileCount
  }, 0)

  let nextNode: ChangeTreeDirNode = {
    ...node,
    children: compressedChildren,
    fileCount,
  }

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

function buildChangeFileTree(changes: WorkingTreeChange[]): ChangeTreeNode[] {
  const root: MutableChangeTreeDirNode = {
    kind: "dir",
    name: "",
    path: "",
    children: new Map(),
  }

  for (const change of changes) {
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

      const nextDir: MutableChangeTreeDirNode = {
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
  nodes: ChangeTreeNode[],
  expanded = new Set<string>()
): Set<string> {
  for (const node of nodes) {
    if (node.kind !== "dir") continue
    expanded.add(node.path)
    collectExpandedDirectoryPaths(node.children, expanded)
  }
  return expanded
}

function isUntrackedStatus(status: string): boolean {
  return status.trim().toUpperCase() === UNTRACKED_STATUS
}

function mapStatus(
  status: string
): "added" | "modified" | "deleted" | "renamed" {
  const normalized = status.trim().toUpperCase()
  if (normalized.includes("A")) return "added"
  if (normalized.includes("R") || normalized.includes("C")) return "renamed"
  if (normalized.includes("D")) return "deleted"
  return "modified"
}

function canOpenFile(status: string): boolean {
  return !status.trim().toUpperCase().includes("D")
}

function shouldRefreshFromEvent(event: FileTreeChangedEvent): boolean {
  const shouldRefreshGitStatus = event.refresh_git_status ?? true
  if (!shouldRefreshGitStatus) return false
  if (event.kind === "access") return false
  return true
}

function toWorkingTreeChanges(
  entries: GitStatusEntry[],
  diffText: string
): WorkingTreeChange[] {
  const stats = parseDiffStatsMap(diffText)

  return entries
    .map((entry) => {
      const path = normalizeGitStatusPath(entry.file)
      if (!path) return null
      const diffStat = stats.get(path)
      return {
        path,
        status: entry.status.trim() || "M",
        additions: diffStat?.additions ?? 0,
        deletions: diffStat?.deletions ?? 0,
      }
    })
    .filter((change): change is WorkingTreeChange => change !== null)
    .sort((left, right) =>
      left.path.localeCompare(right.path, undefined, { sensitivity: "base" })
    )
}

export function GitChangesTab() {
  const t = useTranslations("Folder.gitChangesTab")
  const tCommon = useTranslations("Folder.common")
  const { folder } = useFolderContext()
  const { activeTab } = useAuxPanelContext()
  const { openFilePreview, openWorkingTreeDiff } = useWorkspaceContext()

  const [changes, setChanges] = useState<WorkingTreeChange[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const [expandedTrackedPaths, setExpandedTrackedPaths] = useState<Set<string>>(
    new Set()
  )
  const [expandedUntrackedPaths, setExpandedUntrackedPaths] = useState<
    Set<string>
  >(new Set())
  const [rollbackTarget, setRollbackTarget] = useState<GitActionTarget | null>(
    null
  )
  const [rollingBack, setRollingBack] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<GitActionTarget | null>(null)
  const [deleting, setDeleting] = useState(false)
  const [directoryGitActionType, setDirectoryGitActionType] =
    useState<DirectoryGitAction | null>(null)
  const [directoryGitActionTarget, setDirectoryGitActionTarget] =
    useState<GitActionTarget | null>(null)
  const [directoryGitCandidates, setDirectoryGitCandidates] = useState<
    DirectoryGitCandidateEntry[]
  >([])
  const [directoryGitSelectedPaths, setDirectoryGitSelectedPaths] = useState<
    Set<string>
  >(new Set())
  const [directoryGitLoading, setDirectoryGitLoading] = useState(false)
  const [directoryGitSubmitting, setDirectoryGitSubmitting] = useState(false)
  const [directoryGitError, setDirectoryGitError] = useState<string | null>(
    null
  )

  const hasHydratedTrackedPaths = useRef(false)
  const hasHydratedUntrackedPaths = useRef(false)
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const isChangesTabActive = activeTab === "changes"

  const folderName = useMemo(() => {
    const path = folder?.path ?? ""
    const parts = path.split(/[\\/]/).filter(Boolean)
    return (parts[parts.length - 1] ?? path) || t("workspace")
  }, [folder?.path, t])

  const trackedChanges = useMemo(
    () => changes.filter((change) => !isUntrackedStatus(change.status)),
    [changes]
  )
  const untrackedChanges = useMemo(
    () => changes.filter((change) => isUntrackedStatus(change.status)),
    [changes]
  )

  const trackedTreeNodes = useMemo(
    () => buildChangeFileTree(trackedChanges),
    [trackedChanges]
  )
  const untrackedTreeNodes = useMemo(
    () => buildChangeFileTree(untrackedChanges),
    [untrackedChanges]
  )

  const allTrackedDirectoryPaths = useMemo(() => {
    const paths = collectExpandedDirectoryPaths(trackedTreeNodes)
    paths.add(TRACKED_ROOT_PATH)
    return paths
  }, [trackedTreeNodes])
  const allUntrackedDirectoryPaths = useMemo(() => {
    const paths = collectExpandedDirectoryPaths(untrackedTreeNodes)
    paths.add(UNTRACKED_ROOT_PATH)
    return paths
  }, [untrackedTreeNodes])

  useEffect(() => {
    hasHydratedTrackedPaths.current = false
    hasHydratedUntrackedPaths.current = false
    setExpandedTrackedPaths(new Set())
    setExpandedUntrackedPaths(new Set())
  }, [folder?.path])

  useEffect(() => {
    setExpandedTrackedPaths((prev) => {
      if (!hasHydratedTrackedPaths.current) {
        if (trackedChanges.length === 0) return prev
        hasHydratedTrackedPaths.current = true
        return new Set(allTrackedDirectoryPaths)
      }

      const next = new Set<string>()
      for (const path of prev) {
        if (allTrackedDirectoryPaths.has(path)) next.add(path)
      }
      return next
    })
  }, [allTrackedDirectoryPaths, trackedChanges.length])

  useEffect(() => {
    setExpandedUntrackedPaths((prev) => {
      if (!hasHydratedUntrackedPaths.current) {
        if (untrackedChanges.length === 0) return prev
        hasHydratedUntrackedPaths.current = true
        return new Set()
      }

      const next = new Set<string>()
      for (const path of prev) {
        if (allUntrackedDirectoryPaths.has(path)) next.add(path)
      }
      return next
    })
  }, [allUntrackedDirectoryPaths, untrackedChanges.length])

  const fetchChanges = useCallback(
    async (options?: { inline?: boolean }) => {
      if (!folder?.path) {
        setLoading(false)
        setError(null)
        setChanges([])
        return
      }

      const inline = options?.inline ?? false
      if (!inline) {
        setLoading(true)
      }
      setError(null)

      try {
        const statusEntries = await gitStatus(folder.path)
        const hasTrackedEntries = statusEntries.some(
          (entry) => !isUntrackedStatus(entry.status)
        )
        const diffText = hasTrackedEntries
          ? await gitDiff(folder.path).catch(() => "")
          : ""
        setChanges(toWorkingTreeChanges(statusEntries, diffText))
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        if (!inline) {
          setLoading(false)
        }
      }
    },
    [folder?.path]
  )

  useEffect(() => {
    if (!isChangesTabActive) return
    void fetchChanges()
  }, [fetchChanges, isChangesTabActive])

  useEffect(() => {
    const rootPath = folder?.path
    if (!rootPath || !isChangesTabActive) return

    let unlisten: UnlistenFn | null = null
    const normalizedRootPath = normalizeComparePath(rootPath)

    const scheduleRefresh = () => {
      if (refreshTimerRef.current) {
        clearTimeout(refreshTimerRef.current)
      }
      refreshTimerRef.current = setTimeout(() => {
        void fetchChanges({ inline: true })
      }, 220)
    }

    const setup = async () => {
      try {
        await startFileTreeWatch(rootPath)
      } catch {
        // ignore watch startup errors
      }

      try {
        unlisten = await listen<FileTreeChangedEvent>(
          "folder://file-tree-changed",
          (event) => {
            if (
              normalizeComparePath(event.payload.root_path) !==
              normalizedRootPath
            ) {
              return
            }
            if (!shouldRefreshFromEvent(event.payload)) return
            scheduleRefresh()
          }
        )
      } catch {
        // ignore listen errors
      }
    }

    void setup()

    return () => {
      if (refreshTimerRef.current) {
        clearTimeout(refreshTimerRef.current)
        refreshTimerRef.current = null
      }
      disposeTauriListener(unlisten, "AuxPanelGitChanges.fileTreeChanged")
      void stopFileTreeWatch(rootPath)
    }
  }, [fetchChanges, folder?.path, isChangesTabActive])

  const trackedCanExpand = useMemo(() => {
    if (trackedTreeNodes.length === 0) return false
    for (const path of allTrackedDirectoryPaths) {
      if (!expandedTrackedPaths.has(path)) return true
    }
    return false
  }, [allTrackedDirectoryPaths, expandedTrackedPaths, trackedTreeNodes.length])

  const trackedCanCollapse = useMemo(
    () => trackedTreeNodes.length > 0 && expandedTrackedPaths.size > 0,
    [expandedTrackedPaths.size, trackedTreeNodes.length]
  )

  const untrackedCanExpand = useMemo(() => {
    if (untrackedTreeNodes.length === 0) return false
    for (const path of allUntrackedDirectoryPaths) {
      if (!expandedUntrackedPaths.has(path)) return true
    }
    return false
  }, [
    allUntrackedDirectoryPaths,
    expandedUntrackedPaths,
    untrackedTreeNodes.length,
  ])

  const untrackedCanCollapse = useMemo(
    () => untrackedTreeNodes.length > 0 && expandedUntrackedPaths.size > 0,
    [expandedUntrackedPaths.size, untrackedTreeNodes.length]
  )

  const toggleTrackedExpanded = useCallback(() => {
    if (trackedCanExpand) {
      setExpandedTrackedPaths(new Set(allTrackedDirectoryPaths))
      return
    }
    setExpandedTrackedPaths(new Set())
  }, [allTrackedDirectoryPaths, trackedCanExpand])

  const toggleUntrackedExpanded = useCallback(() => {
    if (untrackedCanExpand) {
      setExpandedUntrackedPaths(new Set(allUntrackedDirectoryPaths))
      return
    }
    setExpandedUntrackedPaths(new Set())
  }, [allUntrackedDirectoryPaths, untrackedCanExpand])

  const handleOpenCommitWindow = useCallback(() => {
    if (!folder) return
    openCommitWindow(folder.id).catch((error) => {
      const message = error instanceof Error ? error.message : String(error)
      toast.error(t("toasts.openCommitWindowFailed"), {
        description: message,
      })
    })
  }, [folder, t])

  const resetDirectoryGitActionDialog = useCallback(() => {
    setDirectoryGitActionType(null)
    setDirectoryGitActionTarget(null)
    setDirectoryGitCandidates([])
    setDirectoryGitSelectedPaths(new Set())
    setDirectoryGitError(null)
    setDirectoryGitLoading(false)
    setDirectoryGitSubmitting(false)
  }, [])

  const openDirectoryGitActionDialog = useCallback(
    async (action: DirectoryGitAction, target: GitActionTarget) => {
      if (!folder?.path) return

      setDirectoryGitActionType(action)
      setDirectoryGitActionTarget(target)
      setDirectoryGitCandidates([])
      setDirectoryGitSelectedPaths(new Set())
      setDirectoryGitError(null)
      setDirectoryGitLoading(true)

      try {
        const statusEntries = await gitStatus(folder.path)
        const scopedEntries = scopeGitStatusEntriesForDirectory(
          statusEntries,
          target.path
        )
        const candidates = filterDirectoryGitCandidates(scopedEntries, action)

        if (candidates.length === 0) {
          resetDirectoryGitActionDialog()
          toast.info(
            action === "add"
              ? t("toasts.noAddableFilesInDir")
              : isDeleteAction(action)
                ? t("toasts.noDeletableFilesInDir")
                : t("toasts.noRollbackFilesInDir")
          )
          return
        }

        setDirectoryGitCandidates(candidates)
        setDirectoryGitSelectedPaths(
          new Set(candidates.map((entry) => entry.path))
        )
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        setDirectoryGitError(message)
      } finally {
        setDirectoryGitLoading(false)
      }
    },
    [folder?.path, resetDirectoryGitActionDialog, t]
  )

  const handleRequestRollback = useCallback(
    (target: GitActionTarget) => {
      if (target.kind === "dir") {
        void openDirectoryGitActionDialog("rollback", target)
        return
      }
      setRollbackTarget(target)
    },
    [openDirectoryGitActionDialog]
  )

  const handleAddToVcs = useCallback(
    async (target: GitActionTarget) => {
      if (target.kind === "dir") {
        await openDirectoryGitActionDialog("add", target)
        return
      }

      if (!folder?.path) return
      try {
        await gitAddFiles(folder.path, [target.path])
        toast.success(t("toasts.addedToVcs", { name: target.name }))
        await fetchChanges({ inline: true })
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        toast.error(t("toasts.addToVcsFailed"), { description: message })
      }
    },
    [fetchChanges, folder?.path, openDirectoryGitActionDialog, t]
  )

  const handleRollbackConfirm = useCallback(async () => {
    if (!folder?.path || !rollbackTarget) return

    setRollingBack(true)
    try {
      await gitRollbackFile(folder.path, rollbackTarget.path)
      toast.success(t("toasts.rolledBack", { name: rollbackTarget.name }))
      setRollbackTarget(null)
      await fetchChanges({ inline: true })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      toast.error(t("toasts.rollbackFailed"), { description: message })
    } finally {
      setRollingBack(false)
    }
  }, [fetchChanges, folder?.path, rollbackTarget, t])

  const handleRequestDelete = useCallback(
    (target: GitActionTarget, scope: "tracked" | "untracked") => {
      if (target.kind === "dir") {
        void openDirectoryGitActionDialog(
          scope === "tracked" ? "delete-tracked" : "delete-untracked",
          target
        )
        return
      }
      setDeleteTarget(target)
    },
    [openDirectoryGitActionDialog]
  )

  const handleDeleteConfirm = useCallback(async () => {
    if (!folder?.path || !deleteTarget) return

    setDeleting(true)
    try {
      await deleteFileTreeEntry(folder.path, deleteTarget.path)
      toast.success(t("toasts.deleted", { name: deleteTarget.name }))
      setDeleteTarget(null)
      await fetchChanges({ inline: true })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      toast.error(t("toasts.deleteFailed"), { description: message })
    } finally {
      setDeleting(false)
    }
  }, [deleteTarget, fetchChanges, folder?.path, t])

  const directoryGitAllFilePaths = useMemo(
    () => directoryGitCandidates.map((entry) => entry.path),
    [directoryGitCandidates]
  )

  const directoryGitAllSelected = useMemo(
    () =>
      directoryGitAllFilePaths.length > 0 &&
      directoryGitAllFilePaths.every((path) =>
        directoryGitSelectedPaths.has(path)
      ),
    [directoryGitAllFilePaths, directoryGitSelectedPaths]
  )

  const handleToggleDirectoryGitSelectAll = useCallback(() => {
    setDirectoryGitSelectedPaths((prev) => {
      const next = new Set(prev)
      const allSelected =
        directoryGitAllFilePaths.length > 0 &&
        directoryGitAllFilePaths.every((path) => next.has(path))
      if (allSelected) {
        return new Set<string>()
      }
      return new Set(directoryGitAllFilePaths)
    })
  }, [directoryGitAllFilePaths])

  const handleToggleDirectoryGitFile = useCallback((path: string) => {
    setDirectoryGitSelectedPaths((prev) => {
      const next = new Set(prev)
      if (next.has(path)) {
        next.delete(path)
      } else {
        next.add(path)
      }
      return next
    })
  }, [])

  const handleDirectoryGitActionConfirm = useCallback(async () => {
    if (!folder?.path || !directoryGitActionType) return
    if (directoryGitSelectedPaths.size === 0) return

    const selectedPaths = Array.from(directoryGitSelectedPaths)
    setDirectoryGitSubmitting(true)
    setDirectoryGitError(null)

    try {
      if (directoryGitActionType === "add") {
        await gitAddFiles(folder.path, selectedPaths)
        toast.success(
          t("toasts.addedFilesToVcs", {
            count: selectedPaths.length,
          })
        )
      } else if (isDeleteAction(directoryGitActionType)) {
        for (const filePath of selectedPaths) {
          await deleteFileTreeEntry(folder.path, filePath)
        }
        toast.success(
          t("toasts.deletedFiles", {
            count: selectedPaths.length,
          })
        )
      } else {
        for (const filePath of selectedPaths) {
          await gitRollbackFile(folder.path, filePath)
        }
        toast.success(
          t("toasts.rolledBackFiles", {
            count: selectedPaths.length,
          })
        )
      }

      resetDirectoryGitActionDialog()
      await fetchChanges({ inline: true })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setDirectoryGitError(message)
      toast.error(
        directoryGitActionType === "add"
          ? t("toasts.addToVcsFailed")
          : isDeleteAction(directoryGitActionType)
            ? t("toasts.deleteFailed")
            : t("toasts.rollbackFailed"),
        {
          description: message,
        }
      )
    } finally {
      setDirectoryGitSubmitting(false)
    }
  }, [
    directoryGitActionType,
    directoryGitSelectedPaths,
    fetchChanges,
    folder?.path,
    resetDirectoryGitActionDialog,
    t,
  ])

  useEffect(() => {
    setRollbackTarget(null)
    resetDirectoryGitActionDialog()
  }, [folder?.path, resetDirectoryGitActionDialog])

  const renderTrackedNode = useCallback(
    function renderNode(node: ChangeTreeNode): ReactElement {
      if (node.kind === "dir") {
        const target: GitActionTarget = {
          kind: "dir",
          path: node.path,
          name: node.name,
        }

        return (
          <ContextMenu key={`tracked:${node.path}`}>
            <ContextMenuTrigger>
              <FileTreeFolder
                path={node.path}
                name={node.name}
                suffix={`(${node.fileCount})`}
                suffixClassName="text-muted-foreground/45"
                title={node.path}
              >
                {node.children.map(renderNode)}
              </FileTreeFolder>
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem
                onSelect={() => {
                  handleOpenCommitWindow()
                }}
              >
                {t("actions.commitCode")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  void openWorkingTreeDiff(node.path, { mode: "overview" })
                }}
              >
                {tCommon("viewDiff")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  handleRequestRollback(target)
                }}
                variant="destructive"
              >
                {t("actions.rollback")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  void handleAddToVcs(target)
                }}
              >
                {t("actions.addToVcs")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  handleRequestDelete(target, "tracked")
                }}
                variant="destructive"
              >
                {t("actions.delete")}
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        )
      }

      const file = node.change
      const canOpenCurrentFile = canOpenFile(file.status)
      const target: GitActionTarget = {
        kind: "file",
        path: file.path,
        name: node.name,
      }

      return (
        <ContextMenu key={`tracked:${file.path}`}>
          <ContextMenuTrigger>
            <FileTreeFile
              className="w-full min-w-0 cursor-pointer"
              name={node.name}
              onClick={() => {
                void openWorkingTreeDiff(file.path)
              }}
              path={node.path}
              title={file.path}
            >
              <>
                <span className="size-4 shrink-0" />
                <CommitFileInfo className="flex-1 min-w-0 gap-1.5">
                  <CommitFileStatus status={mapStatus(file.status)}>
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
                handleOpenCommitWindow()
              }}
            >
              {t("actions.commitCode")}
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!canOpenCurrentFile}
              onSelect={() => {
                if (!canOpenCurrentFile) return
                void openFilePreview(file.path)
              }}
            >
              {tCommon("openFile")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                void openWorkingTreeDiff(file.path)
              }}
            >
              {tCommon("viewDiff")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                handleRequestRollback(target)
              }}
              variant="destructive"
            >
              {t("actions.rollback")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                void handleAddToVcs(target)
              }}
            >
              {t("actions.addToVcs")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                handleRequestDelete(target, "tracked")
              }}
              variant="destructive"
            >
              {t("actions.delete")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      )
    },
    [
      handleOpenCommitWindow,
      handleAddToVcs,
      handleRequestDelete,
      handleRequestRollback,
      openFilePreview,
      openWorkingTreeDiff,
      t,
      tCommon,
    ]
  )

  const renderUntrackedNode = useCallback(
    function renderNode(node: ChangeTreeNode): ReactElement {
      if (node.kind === "dir") {
        const target: GitActionTarget = {
          kind: "dir",
          path: node.path,
          name: node.name,
        }

        return (
          <ContextMenu key={`untracked:${node.path}`}>
            <ContextMenuTrigger>
              <FileTreeFolder
                path={node.path}
                name={node.name}
                suffix={`(${node.fileCount})`}
                suffixClassName="text-muted-foreground/45"
                title={node.path}
              >
                {node.children.map(renderNode)}
              </FileTreeFolder>
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem
                onSelect={() => {
                  handleOpenCommitWindow()
                }}
              >
                {t("actions.commitCode")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  void openWorkingTreeDiff(node.path, { mode: "overview" })
                }}
              >
                {tCommon("viewDiff")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  handleRequestRollback(target)
                }}
                variant="destructive"
              >
                {t("actions.rollback")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  void handleAddToVcs(target)
                }}
              >
                {t("actions.addToVcs")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => {
                  handleRequestDelete(target, "untracked")
                }}
                variant="destructive"
              >
                {t("actions.delete")}
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        )
      }

      const file = node.change
      const target: GitActionTarget = {
        kind: "file",
        path: file.path,
        name: node.name,
      }

      return (
        <ContextMenu key={`untracked:${file.path}`}>
          <ContextMenuTrigger>
            <FileTreeFile
              className="w-full min-w-0 cursor-pointer"
              name={node.name}
              onClick={() => {
                void openWorkingTreeDiff(file.path)
              }}
              path={node.path}
              title={file.path}
            >
              <>
                <span className="size-4 shrink-0" />
                <CommitFileInfo className="flex-1 min-w-0 gap-1.5">
                  <CommitFileIcon />
                  <CommitFilePath title={file.path}>{node.name}</CommitFilePath>
                </CommitFileInfo>
              </>
            </FileTreeFile>
          </ContextMenuTrigger>
          <ContextMenuContent>
            <ContextMenuItem
              onSelect={() => {
                handleOpenCommitWindow()
              }}
            >
              {t("actions.commitCode")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                void openFilePreview(file.path)
              }}
            >
              {tCommon("openFile")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                void openWorkingTreeDiff(file.path)
              }}
            >
              {tCommon("viewDiff")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                handleRequestRollback(target)
              }}
              variant="destructive"
            >
              {t("actions.rollback")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                void handleAddToVcs(target)
              }}
            >
              {t("actions.addToVcs")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => {
                handleRequestDelete(target, "untracked")
              }}
              variant="destructive"
            >
              {t("actions.delete")}
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenu>
      )
    },
    [
      handleOpenCommitWindow,
      handleAddToVcs,
      handleRequestDelete,
      handleRequestRollback,
      openFilePreview,
      openWorkingTreeDiff,
      t,
      tCommon,
    ]
  )

  if (loading) {
    return (
      <div className="p-2 space-y-2">
        <Skeleton className="h-6 w-full" />
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    )
  }

  if (error) {
    return (
      <div className="p-2 text-xs text-destructive">
        <p>{error}</p>
      </div>
    )
  }

  return (
    <>
      <div className="h-full min-h-0 overflow-y-auto">
        {trackedChanges.length === 0 && untrackedChanges.length === 0 ? (
          <div className="flex items-center justify-center h-full p-4">
            <p className="text-xs text-muted-foreground text-center">
              {t("noChanges")}
            </p>
          </div>
        ) : (
          <div className="space-y-2 pb-2">
            {trackedChanges.length > 0 && (
              <section className="space-y-1">
                <div className="flex items-center justify-between px-2 py-1 text-[11px] text-muted-foreground">
                  <span>
                    {t("trackedChanges", { count: trackedChanges.length })}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-5"
                    onClick={toggleTrackedExpanded}
                    disabled={!trackedCanExpand && !trackedCanCollapse}
                    title={
                      trackedCanExpand
                        ? t("expandTracked")
                        : t("collapseTracked")
                    }
                    aria-label={
                      trackedCanExpand
                        ? t("expandTracked")
                        : t("collapseTracked")
                    }
                  >
                    {trackedCanExpand ? (
                      <ChevronsUpDown className="size-3.5" />
                    ) : (
                      <ChevronsDownUp className="size-3.5" />
                    )}
                  </Button>
                </div>
                <FileTree
                  className="rounded-none border-0 bg-transparent text-xs [&>div]:p-0"
                  expanded={expandedTrackedPaths}
                  onExpandedChange={setExpandedTrackedPaths}
                >
                  <ContextMenu>
                    <ContextMenuTrigger>
                      <FileTreeFolder
                        path={TRACKED_ROOT_PATH}
                        name={folderName}
                        suffix={`(${trackedChanges.length})`}
                        suffixClassName="text-muted-foreground/45"
                        title={folderName}
                      >
                        {trackedTreeNodes.map(renderTrackedNode)}
                      </FileTreeFolder>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem
                        onSelect={() => {
                          handleOpenCommitWindow()
                        }}
                      >
                        {t("actions.commitCode")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          void openWorkingTreeDiff(".", {
                            mode: "overview",
                          })
                        }}
                      >
                        {tCommon("viewDiff")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          handleRequestRollback({
                            kind: "dir",
                            path: "",
                            name: folderName,
                          })
                        }}
                        variant="destructive"
                      >
                        {t("actions.rollback")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          void handleAddToVcs({
                            kind: "dir",
                            path: "",
                            name: folderName,
                          })
                        }}
                      >
                        {t("actions.addToVcs")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          handleRequestDelete(
                            {
                              kind: "dir",
                              path: "",
                              name: folderName,
                            },
                            "tracked"
                          )
                        }}
                        variant="destructive"
                      >
                        {t("actions.delete")}
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                </FileTree>
              </section>
            )}

            {untrackedChanges.length > 0 && (
              <section className="space-y-1">
                <div className="flex items-center justify-between px-2 py-1 text-[11px] text-muted-foreground">
                  <span>
                    {t("untrackedFiles", { count: untrackedChanges.length })}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-5"
                    onClick={toggleUntrackedExpanded}
                    disabled={!untrackedCanExpand && !untrackedCanCollapse}
                    title={
                      untrackedCanExpand
                        ? t("expandUntracked")
                        : t("collapseUntracked")
                    }
                    aria-label={
                      untrackedCanExpand
                        ? t("expandUntracked")
                        : t("collapseUntracked")
                    }
                  >
                    {untrackedCanExpand ? (
                      <ChevronsUpDown className="size-3.5" />
                    ) : (
                      <ChevronsDownUp className="size-3.5" />
                    )}
                  </Button>
                </div>
                <FileTree
                  className="rounded-none border-0 bg-transparent text-xs [&>div]:p-0"
                  expanded={expandedUntrackedPaths}
                  onExpandedChange={setExpandedUntrackedPaths}
                >
                  <ContextMenu>
                    <ContextMenuTrigger>
                      <FileTreeFolder
                        path={UNTRACKED_ROOT_PATH}
                        name={folderName}
                        suffix={`(${untrackedChanges.length})`}
                        suffixClassName="text-muted-foreground/45"
                        title={folderName}
                      >
                        {untrackedTreeNodes.map(renderUntrackedNode)}
                      </FileTreeFolder>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem
                        onSelect={() => {
                          handleOpenCommitWindow()
                        }}
                      >
                        {t("actions.commitCode")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          void openWorkingTreeDiff(".", {
                            mode: "overview",
                          })
                        }}
                      >
                        {tCommon("viewDiff")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          handleRequestRollback({
                            kind: "dir",
                            path: "",
                            name: folderName,
                          })
                        }}
                        variant="destructive"
                      >
                        {t("actions.rollback")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          void handleAddToVcs({
                            kind: "dir",
                            path: "",
                            name: folderName,
                          })
                        }}
                      >
                        {t("actions.addToVcs")}
                      </ContextMenuItem>
                      <ContextMenuItem
                        onSelect={() => {
                          handleRequestDelete(
                            {
                              kind: "dir",
                              path: "",
                              name: folderName,
                            },
                            "untracked"
                          )
                        }}
                        variant="destructive"
                      >
                        {t("actions.delete")}
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                </FileTree>
              </section>
            )}
          </div>
        )}
      </div>

      <Dialog
        open={Boolean(directoryGitActionType && directoryGitActionTarget)}
        onOpenChange={(open) => {
          if (open) return
          resetDirectoryGitActionDialog()
        }}
      >
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {directoryGitActionType === "add"
                ? t("actions.addToVcs")
                : directoryGitActionType &&
                    isDeleteAction(directoryGitActionType)
                  ? t("actions.delete")
                  : t("actions.rollback")}
            </DialogTitle>
            <DialogDescription>
              {directoryGitActionTarget
                ? directoryGitActionType === "add"
                  ? t("directoryDialog.descriptionAdd", {
                      path: directoryGitActionTarget.path,
                    })
                  : directoryGitActionType &&
                      isDeleteAction(directoryGitActionType)
                    ? t("directoryDialog.descriptionDelete", {
                        path: directoryGitActionTarget.path,
                      })
                    : t("directoryDialog.descriptionRollback", {
                        path: directoryGitActionTarget.path,
                      })
                : t("directoryDialog.descriptionFallback")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3">
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="text-muted-foreground">
                {t("directoryDialog.selectionCount", {
                  selected: directoryGitSelectedPaths.size,
                  total: directoryGitAllFilePaths.length,
                })}
              </span>
              <Button
                type="button"
                size="xs"
                variant="outline"
                disabled={directoryGitLoading || directoryGitSubmitting}
                onClick={handleToggleDirectoryGitSelectAll}
              >
                {directoryGitAllSelected
                  ? t("directoryDialog.unselectAll")
                  : t("directoryDialog.selectAll")}
              </Button>
            </div>
            <div className="max-h-80 overflow-auto rounded-md border">
              {directoryGitLoading ? (
                <div className="py-8 text-center text-xs text-muted-foreground">
                  {t("directoryDialog.loadingCandidates")}
                </div>
              ) : directoryGitError ? (
                <div className="p-3 text-xs text-destructive">
                  {directoryGitError}
                </div>
              ) : directoryGitCandidates.length > 0 ? (
                <div className="divide-y">
                  {directoryGitCandidates.map((entry) => {
                    const selected = directoryGitSelectedPaths.has(entry.path)
                    return (
                      <button
                        key={entry.path}
                        type="button"
                        className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs hover:bg-muted/40"
                        onClick={() => {
                          handleToggleDirectoryGitFile(entry.path)
                        }}
                        disabled={directoryGitSubmitting}
                      >
                        <span
                          className={
                            selected
                              ? "flex h-4 w-4 shrink-0 items-center justify-center rounded border border-primary bg-primary text-[10px] text-primary-foreground"
                              : "flex h-4 w-4 shrink-0 items-center justify-center rounded border border-input"
                          }
                          aria-hidden
                        >
                          {selected ? "✓" : ""}
                        </span>
                        <span className="flex-1 truncate" title={entry.path}>
                          {entry.path}
                        </span>
                        {entry.status !== UNTRACKED_STATUS && (
                          <span className="shrink-0 text-muted-foreground">
                            {entry.status}
                          </span>
                        )}
                      </button>
                    )
                  })}
                </div>
              ) : (
                <div className="py-8 text-center text-xs text-muted-foreground">
                  {t("directoryDialog.noOperableFiles")}
                </div>
              )}
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={directoryGitSubmitting}
                onClick={resetDirectoryGitActionDialog}
              >
                {tCommon("cancel")}
              </Button>
              <Button
                type="button"
                variant={
                  directoryGitActionType === "rollback" ||
                  (directoryGitActionType &&
                    isDeleteAction(directoryGitActionType))
                    ? "destructive"
                    : "default"
                }
                disabled={
                  directoryGitLoading ||
                  directoryGitSubmitting ||
                  directoryGitSelectedPaths.size === 0
                }
                onClick={() => {
                  void handleDirectoryGitActionConfirm()
                }}
              >
                {directoryGitActionType === "add"
                  ? t("actions.addToVcs")
                  : directoryGitActionType &&
                      isDeleteAction(directoryGitActionType)
                    ? t("actions.delete")
                    : t("actions.rollback")}
              </Button>
            </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={Boolean(rollbackTarget)}
        onOpenChange={(open) => {
          if (open) return
          setRollbackTarget(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("rollbackConfirm.title")}</AlertDialogTitle>
            <AlertDialogDescription>
              {rollbackTarget
                ? t("rollbackConfirm.descriptionWithTarget", {
                    kind:
                      rollbackTarget.kind === "dir"
                        ? t("rollbackConfirm.kindDirectory")
                        : t("rollbackConfirm.kindFile"),
                    name: rollbackTarget.name,
                  })
                : t("rollbackConfirm.descriptionFallback")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={rollingBack}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={rollingBack}
              onClick={() => {
                void handleRollbackConfirm()
              }}
            >
              {t("actions.rollback")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => {
          if (open) return
          setDeleteTarget(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("deleteConfirm.title")}</AlertDialogTitle>
            <AlertDialogDescription>
              {deleteTarget
                ? t("deleteConfirm.descriptionWithTarget", {
                    kind:
                      deleteTarget.kind === "dir"
                        ? t("deleteConfirm.kindDirectory")
                        : t("deleteConfirm.kindFile"),
                    name: deleteTarget.name,
                  })
                : t("deleteConfirm.descriptionFallback")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleting}
              onClick={() => {
                void handleDeleteConfirm()
              }}
            >
              {t("actions.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
