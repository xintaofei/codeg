"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { revealItemInDir } from "@/lib/platform"
import ignore from "ignore"
import { Check, ChevronRight } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useAuxPanelContext } from "@/contexts/aux-panel-context"
import { useTabContext } from "@/contexts/tab-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import {
  type FileWorkspaceTab,
  isImageFile,
  useWorkspaceContext,
} from "@/contexts/workspace-context"
import { useWorkspaceStateStore } from "@/hooks/use-workspace-state-store"
import { AuxPanelNoFolderEmpty } from "@/components/layout/aux-panel-no-folder-empty"
import { WorkspaceDegradedBanner } from "@/components/layout/workspace-degraded-banner"
import { WorkspaceUploadDialog } from "@/components/layout/workspace-upload-dialog"
import {
  createFileTreeEntry,
  deleteFileTreeEntry,
  downloadWorkspaceDir,
  downloadWorkspaceFile,
  gitAddFiles,
  getFileTree,
  getGitBranch,
  gitListAllBranches,
  gitRollbackFile,
  gitStatus,
  readFileForEdit,
  readFilePreview,
  openCommitWindow,
  renameFileTreeEntry,
  saveFileCopy,
  WORKSPACE_DOWNLOAD_CANCELLED,
} from "@/lib/api"
import { isDesktop, isRemoteDesktopMode } from "@/lib/transport"
import { emitAttachFileToSession } from "@/lib/session-attachment-events"
import { ScrollArea } from "@/components/ui/scroll-area"
import type {
  FileEditContent,
  FileTreeNode,
  GitBranchList,
  GitStatusEntry,
} from "@/lib/types"
import {
  FileTree,
  FileTreeFolder,
  FileTreeFile,
} from "@/components/ai-elements/file-tree"
import { Button } from "@/components/ui/button"
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
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { Skeleton } from "@/components/ui/skeleton"
import { joinFsPath } from "@/lib/path-utils"
import { toErrorMessage } from "@/lib/app-error"
import { copyTextFromMenu } from "@/lib/utils"

function parentDir(filePath: string): string {
  const slashIndex = filePath.lastIndexOf("/")
  const backslashIndex = filePath.lastIndexOf("\\")
  const splitIndex = Math.max(slashIndex, backslashIndex)
  // No separator at all: the input is a leaf living at its root. For an
  // OS path that's a degenerate "C:" / "foo" — we can't navigate above
  // it, so the caller treated the result as the path itself. For a
  // workspace-relative path like "README.md" the answer is "workspace
  // root", encoded as empty string. The empty-string convention is the
  // safer default and matches what every caller currently expects.
  if (splitIndex < 0) return ""
  if (splitIndex === 0) return filePath.slice(0, 1)
  return filePath.slice(0, splitIndex)
}

function baseName(path: string): string {
  return path.split(/[/\\]/).pop() || path
}

async function copyPathToClipboard(
  absolutePath: string,
  messages: { success: string; failure: string }
) {
  // copyTextFromMenu defers the write until this context menu has closed, so
  // the execCommand clipboard fallback works in non-secure web contexts.
  const ok = await copyTextFromMenu(absolutePath)
  if (ok) {
    toast.success(messages.success)
  } else {
    toast.error(messages.failure)
  }
}

const FILE_TREE_ROOT_PATH = "__workspace_root__"
const GITIGNORE_MUTED_CLASS = "text-muted-foreground/55"

interface FileActionTarget {
  kind: "file" | "dir"
  path: string
  name: string
}

interface ExternalConflictPrompt {
  path: string
  diskContent: string
  unsavedContent: string
  signature: string
}

type GitFileState =
  | "untracked"
  | "modified"
  | "staged"
  | "conflicted"
  | "deleted"
  | "renamed"

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

function prefixFileTreeNodePaths(
  nodes: FileTreeNode[],
  prefix: string
): FileTreeNode[] {
  return nodes.map((node) => {
    const nextPath = prefix ? `${prefix}/${node.path}` : node.path
    if (node.kind === "file") {
      return {
        ...node,
        path: nextPath,
      }
    }
    return {
      ...node,
      path: nextPath,
      children: prefixFileTreeNodePaths(node.children, nextPath),
    }
  })
}

function applyLazyTreeOverrides(
  nodes: FileTreeNode[],
  overrides: ReadonlyMap<string, FileTreeNode[]>
): FileTreeNode[] {
  return nodes.map((node) => {
    if (node.kind === "file") return node
    const overrideChildren = overrides.get(node.path)
    const baseChildren = overrideChildren ?? node.children
    return {
      ...node,
      children: applyLazyTreeOverrides(baseChildren, overrides),
    }
  })
}

function findDirectoryChildren(
  nodes: FileTreeNode[],
  targetPath: string
): FileTreeNode[] | null {
  for (const node of nodes) {
    if (node.kind !== "dir") continue
    if (normalizeComparePath(node.path) === targetPath) {
      return node.children
    }
    const nested = findDirectoryChildren(node.children, targetPath)
    if (nested) return nested
  }
  return null
}

function classifyGitFileState(status: string): GitFileState | null {
  const code = status.trim().toUpperCase()
  if (!code) return null
  if (code === "??") return "untracked"
  if (code.includes("U")) return "conflicted"
  if (code.includes("R") || code.includes("C")) return "renamed"
  if (code.includes("D")) return "deleted"
  if (code.includes("M") || code.includes("T")) return "modified"
  if (code.includes("A")) return "staged"
  return null
}

function getGitFileStateClassName(status?: string): string {
  if (!status) return ""
  const state = classifyGitFileState(status)
  if (state === "untracked") return "text-red-500 dark:text-red-400"
  if (state === "modified") return "text-emerald-600 dark:text-emerald-400"
  if (state === "staged") return "text-emerald-500 dark:text-emerald-400"
  if (state === "conflicted") return "text-amber-500 dark:text-amber-400"
  if (state === "deleted") return "text-orange-500 dark:text-orange-400"
  if (state === "renamed") return "text-violet-500 dark:text-violet-400"
  return ""
}

function getParentPath(path: string): string | null {
  const splitIdx = path.lastIndexOf("/")
  if (splitIdx < 0) return null
  return path.slice(0, splitIdx)
}

function hasIgnoredAncestor(path: string, ignoredPaths: ReadonlySet<string>) {
  let current = path
  while (true) {
    const parent = getParentPath(current)
    if (!parent) return false
    if (ignoredPaths.has(parent)) return true
    current = parent
  }
}

type DirectoryGitAction = "add" | "rollback"

interface DirectoryGitCandidateEntry {
  path: string
  status: string
}

type DirectoryGitTreeNode = DirectoryGitTreeDirNode | DirectoryGitTreeFileNode

interface DirectoryGitTreeDirNode {
  kind: "dir"
  name: string
  path: string
  children: DirectoryGitTreeNode[]
  fileCount: number
}

interface DirectoryGitTreeFileNode {
  kind: "file"
  name: string
  path: string
  status: string
}

interface MutableDirectoryGitTreeDirNode {
  kind: "dir"
  name: string
  path: string
  children: Map<
    string,
    MutableDirectoryGitTreeDirNode | DirectoryGitTreeFileNode
  >
}

const DIRECTORY_GIT_TREE_ROOT_PATH = "__directory_git_tree_root__"

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

function filterDirectoryGitCandidates(
  entries: DirectoryGitCandidateEntry[],
  action: DirectoryGitAction
): DirectoryGitCandidateEntry[] {
  if (action === "add") {
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

function buildDirectoryGitTree(
  entries: DirectoryGitCandidateEntry[],
  directoryPath: string
): DirectoryGitTreeNode[] {
  const normalizedDirPath = normalizeComparePath(directoryPath)
  const root: MutableDirectoryGitTreeDirNode = {
    kind: "dir",
    name: "",
    path: "",
    children: new Map(),
  }

  for (const entry of entries) {
    let relativePath = normalizeComparePath(entry.path)
    if (normalizedDirPath && relativePath.startsWith(`${normalizedDirPath}/`)) {
      relativePath = relativePath.slice(normalizedDirPath.length + 1)
    }
    const segments = relativePath.split("/").filter(Boolean)
    if (segments.length === 0) continue

    let current = root
    for (const [index, segment] of segments.entries()) {
      const isLeaf = index === segments.length - 1
      const nestedPath = segments.slice(0, index + 1).join("/")
      const nodePath = normalizedDirPath
        ? `${normalizedDirPath}/${nestedPath}`
        : nestedPath

      if (isLeaf) {
        current.children.set(`file:${nodePath}`, {
          kind: "file",
          name: segment,
          path: nodePath,
          status: entry.status,
        })
        continue
      }

      const dirKey = `dir:${nodePath}`
      const existing = current.children.get(dirKey)
      if (existing && existing.kind === "dir") {
        current = existing
        continue
      }

      const nextDir: MutableDirectoryGitTreeDirNode = {
        kind: "dir",
        name: segment,
        path: nodePath,
        children: new Map(),
      }
      current.children.set(dirKey, nextDir)
      current = nextDir
    }
  }

  const toSortedTreeNodes = (
    dir: MutableDirectoryGitTreeDirNode
  ): DirectoryGitTreeNode[] => {
    return Array.from(dir.children.values())
      .map<DirectoryGitTreeNode>((node) => {
        if (node.kind === "file") return node
        return {
          kind: "dir" as const,
          name: node.name,
          path: node.path,
          children: toSortedTreeNodes(node),
          fileCount: 0,
        }
      })
      .sort((left, right) => {
        if (left.kind !== right.kind) return left.kind === "dir" ? -1 : 1
        return left.name.localeCompare(right.name, undefined, {
          sensitivity: "base",
        })
      })
  }

  const annotateDirectory = (
    node: DirectoryGitTreeDirNode
  ): DirectoryGitTreeDirNode => {
    const nextChildren = node.children.map((child) => {
      if (child.kind === "file") return child
      return annotateDirectory(child)
    })
    const fileCount = nextChildren.reduce((count, child) => {
      if (child.kind === "file") return count + 1
      return count + child.fileCount
    }, 0)
    return {
      ...node,
      children: nextChildren,
      fileCount,
    }
  }

  return toSortedTreeNodes(root).map((node) => {
    if (node.kind === "file") return node
    return annotateDirectory(node)
  })
}

function collectDirectoryGitTreeExpandedPaths(
  nodes: DirectoryGitTreeNode[],
  expanded = new Set<string>()
): Set<string> {
  for (const node of nodes) {
    if (node.kind !== "dir") continue
    expanded.add(node.path)
    collectDirectoryGitTreeExpandedPaths(node.children, expanded)
  }
  return expanded
}

function collectDirectoryGitTreeLeafPaths(
  node: DirectoryGitTreeNode
): string[] {
  if (node.kind === "file") return [node.path]
  return node.children.flatMap(collectDirectoryGitTreeLeafPaths)
}

interface RenderNodeProps {
  node: FileTreeNode
  expandedPaths: ReadonlySet<string>
  workspacePath: string
  activeSessionTabId: string | null
  gitEnabled: boolean
  webMode: boolean
  folderUploadSupported: boolean
  gitStatusByPath: ReadonlyMap<string, string>
  gitChangedDirPaths: ReadonlySet<string>
  untrackedDirPaths: ReadonlySet<string>
  gitignoreIgnoredPaths: ReadonlySet<string>
  ancestorGitignoreIgnored: boolean
  ancestorUntracked: boolean
  onOpenFilePreview: (path: string) => void
  onOpenFileDiff: (path: string) => void
  onOpenDirDiff: (path: string) => void
  onOpenCommitWindow: () => void
  onRequestCompareWithBranch: (target: FileActionTarget) => void
  onRequestRollback: (target: FileActionTarget) => void
  onOpenDirInTerminal: (dirPath: string, fileName: string) => Promise<void>
  onRequestAddToVcs: (target: FileActionTarget) => void
  onRequestRename: (target: FileActionTarget) => void
  onRequestCreate: (parentPath: string, kind: "file" | "dir") => void
  onRequestDelete: (target: FileActionTarget) => void
  onRequestUpload: (targetPath: string) => void
  onRequestDownloadFile: (target: FileActionTarget) => void
  onRequestDownloadDir: (target: FileActionTarget) => void
  onRefresh: () => void
}

function RenderNode({
  node,
  expandedPaths,
  workspacePath,
  activeSessionTabId,
  gitEnabled,
  webMode,
  folderUploadSupported,
  gitStatusByPath,
  gitChangedDirPaths,
  untrackedDirPaths,
  gitignoreIgnoredPaths,
  ancestorGitignoreIgnored,
  ancestorUntracked,
  onOpenFilePreview,
  onOpenFileDiff,
  onOpenDirDiff,
  onOpenCommitWindow,
  onRequestCompareWithBranch,
  onRequestRollback,
  onOpenDirInTerminal,
  onRequestAddToVcs,
  onRequestCreate,
  onRequestRename,
  onRequestDelete,
  onRequestUpload,
  onRequestDownloadFile,
  onRequestDownloadDir,
  onRefresh,
}: RenderNodeProps) {
  const t = useTranslations("Folder.fileTreeTab")
  const tCommon = useTranslations("Folder.common")
  const isGitignoreIgnored =
    ancestorGitignoreIgnored || gitignoreIgnoredPaths.has(node.path)

  const systemExplorerLabel =
    typeof navigator === "undefined"
      ? t("openInFileManager")
      : (() => {
          const platform =
            `${navigator.platform} ${navigator.userAgent}`.toLowerCase()
          if (platform.includes("mac")) return t("openInFinder")
          if (platform.includes("win")) return t("openInExplorer")
          return t("openInFileManager")
        })()

  if (node.kind === "file") {
    const gitStatusCode =
      gitStatusByPath.get(node.path) ?? (ancestorUntracked ? "??" : undefined)
    const absolutePath = joinFsPath(workspacePath, node.path)
    const dirPath = parentDir(absolutePath)
    const isGitMenuDisabled = !gitEnabled || isGitignoreIgnored

    const handleAttachToSession = () => {
      if (!activeSessionTabId) return
      emitAttachFileToSession({
        tabId: activeSessionTabId,
        path: absolutePath,
      })
    }

    const handleOpenInSystemExplorer = async () => {
      try {
        await revealItemInDir(absolutePath)
      } catch (error) {
        const message = toErrorMessage(error)
        toast.error(t("toasts.openDirectoryFailed"), { description: message })
      }
    }

    return (
      <ContextMenu>
        <ContextMenuTrigger>
          <FileTreeFile
            path={node.path}
            name={node.name}
            className={
              isGitignoreIgnored
                ? GITIGNORE_MUTED_CLASS
                : getGitFileStateClassName(gitStatusCode)
            }
          />
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={() => onOpenFilePreview(node.path)}>
            {tCommon("openFile")}
          </ContextMenuItem>
          <ContextMenuItem
            onSelect={() => void handleAttachToSession()}
            disabled={!activeSessionTabId}
          >
            {t("attachToCurrentSession")}
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>{t("new")}</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem
                onSelect={() => onRequestCreate(node.path, "file")}
              >
                {t("newFile")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => onRequestCreate(node.path, "dir")}
              >
                {t("newDirectory")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSub>
            <ContextMenuSubTrigger disabled={isGitMenuDisabled}>
              {t("git")}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem
                onSelect={() => onOpenCommitWindow()}
                disabled={isGitMenuDisabled}
              >
                {t("actions.commitCode")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => onRequestAddToVcs(node)}
                disabled={
                  isGitMenuDisabled ||
                  classifyGitFileState(gitStatusCode ?? "") !== "untracked"
                }
              >
                {t("actions.addToVcs")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => onOpenFileDiff(node.path)}
                disabled={isGitMenuDisabled}
              >
                {tCommon("viewDiff")}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => onRequestCompareWithBranch(node)}
                disabled={isGitMenuDisabled}
              >
                {t("compareWithBranch")}
              </ContextMenuItem>
              <ContextMenuItem
                variant="destructive"
                onSelect={() => onRequestRollback(node)}
                disabled={isGitMenuDisabled}
              >
                {t("actions.rollback")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuItem onSelect={() => onRequestRename(node)}>
            {tCommon("rename")}
          </ContextMenuItem>
          <ContextMenuItem onSelect={onRefresh}>
            {t("reloadFromDisk")}
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>{t("openIn")}</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem
                onSelect={() => void handleOpenInSystemExplorer()}
              >
                {systemExplorerLabel}
              </ContextMenuItem>
              <ContextMenuItem
                onSelect={() => void onOpenDirInTerminal(dirPath, node.name)}
              >
                {t("openInTerminal")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuItem
            onSelect={() =>
              void copyPathToClipboard(absolutePath, {
                success: t("toasts.pathCopied"),
                failure: t("toasts.copyPathFailed"),
              })
            }
          >
            {t("copyPath")}
          </ContextMenuItem>
          {webMode && (
            <>
              <ContextMenuItem
                onSelect={() => onRequestUpload(parentDir(node.path))}
              >
                {t("upload")}
              </ContextMenuItem>
              <ContextMenuItem onSelect={() => onRequestDownloadFile(node)}>
                {t("download")}
              </ContextMenuItem>
            </>
          )}
          <ContextMenuItem
            onSelect={() => onRequestDelete(node)}
            variant="destructive"
          >
            {tCommon("delete")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    )
  }

  const absolutePath = joinFsPath(workspacePath, node.path)
  const isThisDirUntracked =
    ancestorUntracked || untrackedDirPaths.has(node.path)
  const dirHasChanges =
    !isGitignoreIgnored &&
    (gitChangedDirPaths.has(node.path) || isThisDirUntracked)
  const isGitMenuDisabled = !gitEnabled || isGitignoreIgnored
  const shouldRenderChildren = expandedPaths.has(node.path)

  const handleAttachDirToSession = () => {
    if (!activeSessionTabId) return
    emitAttachFileToSession({
      tabId: activeSessionTabId,
      path: absolutePath,
    })
  }

  const handleOpenDirInSystemExplorer = async () => {
    try {
      await revealItemInDir(absolutePath)
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.openDirectoryFailed"), { description: message })
    }
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger>
        <FileTreeFolder
          path={node.path}
          name={node.name}
          nameClassName={
            isGitignoreIgnored
              ? GITIGNORE_MUTED_CLASS
              : dirHasChanges
                ? "text-emerald-600 dark:text-emerald-400"
                : undefined
          }
          iconClassName={isGitignoreIgnored ? GITIGNORE_MUTED_CLASS : undefined}
        >
          {shouldRenderChildren
            ? node.children.map((child) => (
                <RenderNode
                  key={child.path}
                  node={child}
                  expandedPaths={expandedPaths}
                  workspacePath={workspacePath}
                  activeSessionTabId={activeSessionTabId}
                  gitEnabled={gitEnabled}
                  webMode={webMode}
                  folderUploadSupported={folderUploadSupported}
                  gitStatusByPath={gitStatusByPath}
                  gitChangedDirPaths={gitChangedDirPaths}
                  untrackedDirPaths={untrackedDirPaths}
                  gitignoreIgnoredPaths={gitignoreIgnoredPaths}
                  ancestorGitignoreIgnored={isGitignoreIgnored}
                  ancestorUntracked={isThisDirUntracked}
                  onOpenFilePreview={onOpenFilePreview}
                  onOpenFileDiff={onOpenFileDiff}
                  onOpenDirDiff={onOpenDirDiff}
                  onOpenCommitWindow={onOpenCommitWindow}
                  onRequestCompareWithBranch={onRequestCompareWithBranch}
                  onRequestRollback={onRequestRollback}
                  onOpenDirInTerminal={onOpenDirInTerminal}
                  onRequestCreate={onRequestCreate}
                  onRequestAddToVcs={onRequestAddToVcs}
                  onRequestRename={onRequestRename}
                  onRequestDelete={onRequestDelete}
                  onRequestUpload={onRequestUpload}
                  onRequestDownloadFile={onRequestDownloadFile}
                  onRequestDownloadDir={onRequestDownloadDir}
                  onRefresh={onRefresh}
                />
              ))
            : null}
        </FileTreeFolder>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem
          onSelect={handleAttachDirToSession}
          disabled={!activeSessionTabId}
        >
          {t("attachToCurrentSession")}
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger>{t("new")}</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              onSelect={() => onRequestCreate(node.path, "file")}
            >
              {t("newFile")}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => onRequestCreate(node.path, "dir")}>
              {t("newDirectory")}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuSub>
          <ContextMenuSubTrigger disabled={isGitMenuDisabled}>
            {t("git")}
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              onSelect={() => onOpenCommitWindow()}
              disabled={isGitMenuDisabled}
            >
              {t("actions.commitCode")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => onRequestAddToVcs(node)}
              disabled={isGitMenuDisabled}
            >
              {t("actions.addToVcs")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => onOpenDirDiff(node.path)}
              disabled={isGitMenuDisabled}
            >
              {tCommon("viewDiff")}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => onRequestCompareWithBranch(node)}
              disabled={isGitMenuDisabled}
            >
              {t("compareWithBranch")}
            </ContextMenuItem>
            <ContextMenuItem
              variant="destructive"
              onSelect={() => onRequestRollback(node)}
              disabled={isGitMenuDisabled}
            >
              {t("actions.rollback")}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem onSelect={() => onRequestRename(node)}>
          {tCommon("rename")}
        </ContextMenuItem>
        <ContextMenuSub>
          <ContextMenuSubTrigger>{t("openIn")}</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            <ContextMenuItem
              onSelect={() => void handleOpenDirInSystemExplorer()}
            >
              {systemExplorerLabel}
            </ContextMenuItem>
            <ContextMenuItem
              onSelect={() => void onOpenDirInTerminal(absolutePath, node.name)}
            >
              {t("openInTerminal")}
            </ContextMenuItem>
          </ContextMenuSubContent>
        </ContextMenuSub>
        <ContextMenuItem
          onSelect={() =>
            void copyPathToClipboard(absolutePath, {
              success: t("toasts.pathCopied"),
              failure: t("toasts.copyPathFailed"),
            })
          }
        >
          {t("copyPath")}
        </ContextMenuItem>
        {webMode && (
          <>
            <ContextMenuItem onSelect={() => onRequestUpload(node.path)}>
              {t("upload")}
            </ContextMenuItem>
            <ContextMenuItem onSelect={() => onRequestDownloadDir(node)}>
              {t("downloadAsZip")}
            </ContextMenuItem>
          </>
        )}
        <ContextMenuItem onSelect={onRefresh}>
          {t("reloadFromDisk")}
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() => onRequestDelete(node)}
          variant="destructive"
        >
          {tCommon("delete")}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}

export function FileTreeTab() {
  const t = useTranslations("Folder.fileTreeTab")
  const tCommon = useTranslations("Folder.common")
  const { pendingRevealPath, consumePendingRevealPath } = useAuxPanelContext()
  const { activeFolder: folder } = useActiveFolder()
  const { tabs, activeTabId } = useTabContext()
  const { createTerminalInDirectory } = useTerminalContext()
  const {
    activeFileTab,
    activeFilePath,
    fileTabs,
    openBranchDiff,
    openExternalConflictDiff,
    openFilePreview,
    openWorkingTreeDiff,
    reloadOpenFileBackground,
    applyExternalReload,
    markTabsStale,
    rejectFileTab,
  } = useWorkspaceContext()
  const workspaceState = useWorkspaceStateStore(folder?.path ?? null)
  const [nodes, setNodes] = useState<FileTreeNode[]>([])
  const [gitStatusByPath, setGitStatusByPath] = useState<Map<string, string>>(
    new Map()
  )
  const [gitEnabled, setGitEnabled] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [renameTarget, setRenameTarget] = useState<FileActionTarget | null>(
    null
  )
  const [renameValue, setRenameValue] = useState("")
  const [renaming, setRenaming] = useState(false)
  const [createParentPath, setCreateParentPath] = useState<string | null>(null)
  const [createKind, setCreateKind] = useState<"file" | "dir">("file")
  const [createName, setCreateName] = useState("")
  const [creating, setCreating] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<FileActionTarget | null>(
    null
  )
  const [deleting, setDeleting] = useState(false)
  const [rollbackTarget, setRollbackTarget] = useState<FileActionTarget | null>(
    null
  )
  const [rollingBack, setRollingBack] = useState(false)
  const [compareTarget, setCompareTarget] = useState<FileActionTarget | null>(
    null
  )
  const [externalConflictPrompt, setExternalConflictPrompt] =
    useState<ExternalConflictPrompt | null>(null)
  const [savingExternalConflictCopy, setSavingExternalConflictCopy] =
    useState(false)
  const [directoryGitActionType, setDirectoryGitActionType] =
    useState<DirectoryGitAction | null>(null)
  const [directoryGitActionTarget, setDirectoryGitActionTarget] =
    useState<FileActionTarget | null>(null)
  const [directoryGitCandidates, setDirectoryGitCandidates] = useState<
    DirectoryGitCandidateEntry[]
  >([])
  const [directoryGitSelectedPaths, setDirectoryGitSelectedPaths] = useState<
    Set<string>
  >(new Set())
  const [directoryGitExpandedPaths, setDirectoryGitExpandedPaths] = useState<
    Set<string>
  >(new Set([DIRECTORY_GIT_TREE_ROOT_PATH]))
  const [directoryGitLoading, setDirectoryGitLoading] = useState(false)
  const [directoryGitSubmitting, setDirectoryGitSubmitting] = useState(false)
  const [directoryGitError, setDirectoryGitError] = useState<string | null>(
    null
  )
  const [compareBranchFilter, setCompareBranchFilter] = useState("")
  const [compareCurrentBranch, setCompareCurrentBranch] = useState<
    string | null
  >(null)
  const [compareBranchList, setCompareBranchList] = useState<GitBranchList>({
    local: [],
    remote: [],
    worktree_branches: [],
  })
  const [compareBranchLoading, setCompareBranchLoading] = useState(false)
  const [compareRecentOpen, setCompareRecentOpen] = useState(true)
  const [compareLocalOpen, setCompareLocalOpen] = useState(false)
  const [compareRemoteOpen, setCompareRemoteOpen] = useState(false)
  const [comparing, setComparing] = useState(false)
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(
    () => new Set([FILE_TREE_ROOT_PATH])
  )
  const [gitignoreIgnoredPaths, setGitignoreIgnoredPaths] = useState<
    Set<string>
  >(new Set())
  const activeFileTabRef = useRef(activeFileTab)
  const fileTabsRef = useRef<FileWorkspaceTab[]>(fileTabs)
  const filePathSetRef = useRef<Set<string>>(new Set())
  const previousExpandedPathsRef = useRef<Set<string>>(
    new Set([FILE_TREE_ROOT_PATH])
  )
  const lazyLoadedChildrenByPathRef = useRef<Map<string, FileTreeNode[]>>(
    new Map()
  )
  const lazyLoadingDirPathsRef = useRef<Set<string>>(new Set())
  const loadDirectoryChildrenRef = useRef<
    ((dirPath: string) => Promise<void>) | null
  >(null)
  const expandedPathsRef = useRef<Set<string>>(new Set([FILE_TREE_ROOT_PATH]))
  const workspaceTreeRef = useRef<FileTreeNode[]>([])
  const externalConflictSignatureByPathRef = useRef<Map<string, string>>(
    new Map()
  )

  useEffect(() => {
    activeFileTabRef.current = activeFileTab
  }, [activeFileTab])

  useEffect(() => {
    fileTabsRef.current = fileTabs
  }, [fileTabs])

  useEffect(() => {
    setExpandedPaths(new Set([FILE_TREE_ROOT_PATH]))
    previousExpandedPathsRef.current = new Set([FILE_TREE_ROOT_PATH])
    setGitignoreIgnoredPaths(new Set())
    setExternalConflictPrompt(null)
    setSavingExternalConflictCopy(false)
    lazyLoadedChildrenByPathRef.current.clear()
    lazyLoadingDirPathsRef.current.clear()
    externalConflictSignatureByPathRef.current.clear()
  }, [folder?.path])

  // Handle pending reveal path: expand all ancestor directories once tree is loaded
  const hasNodes = nodes.length > 0
  useEffect(() => {
    if (!pendingRevealPath || !hasNodes) return
    consumePendingRevealPath()
    setExpandedPaths((prev) => {
      const next = new Set(prev)
      next.add(FILE_TREE_ROOT_PATH)
      let idx = pendingRevealPath.indexOf("/")
      while (idx !== -1) {
        next.add(pendingRevealPath.slice(0, idx))
        idx = pendingRevealPath.indexOf("/", idx + 1)
      }
      next.add(pendingRevealPath)
      return next
    })
  }, [pendingRevealPath, consumePendingRevealPath, hasNodes])

  useEffect(() => {
    if (!activeFileTab || activeFileTab.kind !== "file") return
    if (!activeFileTab.path) return
    if (activeFileTab.loading || activeFileTab.isDirty) return
    const activeFilePath = activeFileTab.path
    externalConflictSignatureByPathRef.current.delete(activeFilePath)
    setExternalConflictPrompt((current) =>
      current &&
      normalizeComparePath(current.path) ===
        normalizeComparePath(activeFilePath)
        ? null
        : current
    )
  }, [activeFileTab])

  const activeSessionTabId = useMemo(() => {
    const activeTab = tabs.find((tab) => tab.id === activeTabId)
    if (!activeTab) return null
    if (activeTab.kind !== "conversation") {
      return null
    }
    return activeTab.id
  }, [tabs, activeTabId])

  const fetchTree = useCallback(
    async (options?: {
      skipTree?: boolean
      skipStatus?: boolean
      silent?: boolean
      maxDepth?: number
    }) => {
      void options
      if (!folder?.path) {
        setNodes([])
        setGitStatusByPath(new Map())
        setGitEnabled(false)
        setLoading(false)
        setError(null)
        return
      }

      // Drop the lazy-load override cache so the fresh snapshot is not
      // masked by stale children (e.g. after deletes / renames / rollbacks
      // or files the agent just created). Reading expanded paths via a ref
      // keeps fetchTree's identity stable across expand/collapse so
      // downstream memoization is not invalidated on every tree interaction.
      const pathsToReload = Array.from(expandedPathsRef.current).filter(
        (path) => path !== FILE_TREE_ROOT_PATH
      )
      lazyLoadedChildrenByPathRef.current.clear()
      await workspaceState.requestResync("manual_refresh")
      // Re-hydrate children for directories beyond WORKSPACE_TREE_MAX_DEPTH
      // that are still expanded — the backend snapshot does not include them.
      const loader = loadDirectoryChildrenRef.current
      if (loader) {
        for (const path of pathsToReload) {
          void loader(path)
        }
      }
    },
    [folder?.path, workspaceState]
  )

  // Tree updates are the only source that should cause a full setNodes.
  // applyLazyTreeOverrides rebuilds every directory node object, which forces
  // React to re-render the entire tree. Keeping this effect narrow avoids
  // wasted work on health / seq / error / git transitions that don't touch
  // the tree shape (e.g. the intermediate "resyncing" patch during a refresh).
  useEffect(() => {
    workspaceTreeRef.current = workspaceState.tree
    setNodes(
      applyLazyTreeOverrides(
        workspaceState.tree,
        lazyLoadedChildrenByPathRef.current
      )
    )
  }, [folder?.path, workspaceState.tree])

  useEffect(() => {
    const nextStatusByPath = new Map<string, string>()
    for (const entry of workspaceState.git) {
      nextStatusByPath.set(entry.path, entry.status)
    }
    setGitStatusByPath(nextStatusByPath)
    setGitEnabled(true)
  }, [workspaceState.git])

  useEffect(() => {
    setLoading(
      workspaceState.health === "resyncing" && workspaceState.seq === 0
    )
    setError(workspaceState.health === "degraded" ? workspaceState.error : null)
  }, [workspaceState.error, workspaceState.health, workspaceState.seq])

  const loadDirectoryChildren = useCallback(
    async (dirPath: string) => {
      const rootPath = folder?.path
      if (!rootPath) return
      const normalizedDirPath = normalizeComparePath(dirPath)
      if (!normalizedDirPath) return
      if (lazyLoadedChildrenByPathRef.current.has(normalizedDirPath)) return
      if (lazyLoadingDirPathsRef.current.has(normalizedDirPath)) return

      // Check the backend tree (source of truth), not the rendered `nodes`.
      // `nodes` carries stale lazy-cache overrides that don't invalidate
      // until a tree_replace delta arrives — but for directories beyond
      // WORKSPACE_TREE_MAX_DEPTH the backend never emits tree_replace for
      // changes inside them (their children are not in tree_snapshot, so
      // the refreshed tree compares equal to the old one). Checking
      // `nodes` would cause fetchTree's forced reload to short-circuit on
      // the stale override and miss deletions / creations in deep dirs.
      const existingChildren = findDirectoryChildren(
        workspaceTreeRef.current,
        normalizedDirPath
      )
      if (existingChildren && existingChildren.length > 0) {
        return
      }

      lazyLoadingDirPathsRef.current.add(normalizedDirPath)
      try {
        const subtree = await getFileTree(
          joinFsPath(rootPath, normalizedDirPath),
          1
        )
        const prefixed = prefixFileTreeNodePaths(subtree, normalizedDirPath)
        lazyLoadedChildrenByPathRef.current.set(normalizedDirPath, prefixed)
        setNodes((prev) =>
          applyLazyTreeOverrides(prev, lazyLoadedChildrenByPathRef.current)
        )
      } catch {
        // Ignore lazy load failures and keep current collapsed/empty state.
      } finally {
        lazyLoadingDirPathsRef.current.delete(normalizedDirPath)
      }
    },
    [folder?.path]
  )

  useEffect(() => {
    loadDirectoryChildrenRef.current = loadDirectoryChildren
  }, [loadDirectoryChildren])

  useEffect(() => {
    expandedPathsRef.current = expandedPaths
  }, [expandedPaths])

  // Subscribe to workspace envelopes to invalidate lazy-loaded overrides for
  // directories beyond WORKSPACE_TREE_MAX_DEPTH. Those directories are never
  // reflected in the backend's depth-2 tree_snapshot, so changes inside them
  // don't emit a tree_replace delta — the frontend has to target invalidation
  // by matching each `changed_paths` entry against its cached ancestors.
  // The backend already debounces raw FS events (300ms / 1.5s max), so we only
  // need a microtask hop here to merge paths that hit the same cached
  // ancestor within one envelope (or any synchronous burst of envelopes).
  const subscribeWorkspaceEnvelopes = workspaceState.subscribeEnvelopes
  useEffect(() => {
    if (!subscribeWorkspaceEnvelopes) return

    const pendingPaths = new Set<string>()
    let flushScheduled = false
    let disposed = false

    const flushPending = () => {
      flushScheduled = false
      if (disposed || pendingPaths.size === 0) return
      const paths = Array.from(pendingPaths)
      pendingPaths.clear()

      const loader = loadDirectoryChildrenRef.current
      const cache = lazyLoadedChildrenByPathRef.current
      const invalidated = new Set<string>()

      for (const changed of paths) {
        const normalized = normalizeComparePath(changed)
        if (!normalized) continue
        // When the changed path is itself a cached directory (FS events
        // that report the directory directly, e.g. a rename or a dir-level
        // notification), its own entry is stale — invalidate it.
        if (cache.has(normalized)) {
          invalidated.add(normalized)
        }
        // Independently of the above, walk up to the nearest cached
        // ancestor: the ancestor's children listing may also be stale
        // (a child was added, removed, or renamed). Without this, cases
        // where both a parent and child are cached leave the parent
        // holding a ghost reference to the old child.
        let cursor = normalized
        while (cursor.length > 0) {
          const slash = cursor.lastIndexOf("/")
          const parent = slash === -1 ? "" : cursor.slice(0, slash)
          if (parent.length === 0) break
          if (cache.has(parent)) {
            invalidated.add(parent)
            break
          }
          cursor = parent
        }
      }

      if (invalidated.size === 0) return
      for (const path of invalidated) {
        cache.delete(path)
      }
      if (!loader) return
      // Skip refetching directories that are no longer expanded — their
      // cleared cache will be re-hydrated on the next expansion via the
      // expandedPaths effect. This avoids spurious getFileTree traffic
      // for collapsed branches under bursty FS activity.
      const expanded = expandedPathsRef.current
      for (const path of invalidated) {
        if (!expanded.has(path)) continue
        void loader(path)
      }
    }

    const unsubscribe = subscribeWorkspaceEnvelopes(({ changed_paths }) => {
      if (!changed_paths || changed_paths.length === 0) return
      for (const path of changed_paths) {
        pendingPaths.add(path)
      }
      if (flushScheduled) return
      flushScheduled = true
      queueMicrotask(flushPending)
    })

    return () => {
      disposed = true
      unsubscribe()
      pendingPaths.clear()
    }
  }, [subscribeWorkspaceEnvelopes])

  useEffect(() => {
    const previousExpanded = previousExpandedPathsRef.current
    for (const path of expandedPaths) {
      if (path === FILE_TREE_ROOT_PATH) continue
      if (previousExpanded.has(path)) continue
      void loadDirectoryChildren(path)
    }
    previousExpandedPathsRef.current = new Set(expandedPaths)
  }, [expandedPaths, folder?.path, loadDirectoryChildren])

  const filePathSet = useMemo(() => {
    const paths = new Set<string>()
    const collect = (items: FileTreeNode[]) => {
      for (const item of items) {
        if (item.kind === "file") {
          paths.add(item.path)
        } else {
          collect(item.children)
        }
      }
    }
    collect(nodes)
    return paths
  }, [nodes])

  const dirChildrenByPath = useMemo(() => {
    const next = new Map<string, FileTreeNode[]>()
    next.set("", nodes)

    const collect = (items: FileTreeNode[]) => {
      for (const item of items) {
        if (item.kind !== "dir") continue
        next.set(item.path, item.children)
        collect(item.children)
      }
    }

    collect(nodes)
    return next
  }, [nodes])

  const expandedDirPaths = useMemo(() => {
    const dirs = new Set<string>([""])
    for (const path of expandedPaths) {
      if (path === FILE_TREE_ROOT_PATH) continue
      dirs.add(path)
    }
    return Array.from(dirs)
  }, [expandedPaths])

  useEffect(() => {
    filePathSetRef.current = filePathSet
  }, [filePathSet])

  useEffect(() => {
    if (!folder?.path) {
      setGitignoreIgnoredPaths(new Set())
      return
    }

    let canceled = false

    const loadIgnoredPaths = async () => {
      const nextIgnoredPaths = new Set<string>()
      const sortedDirs = [...expandedDirPaths].sort(
        (left, right) => left.length - right.length
      )

      for (const dirPath of sortedDirs) {
        if (hasIgnoredAncestor(dirPath, nextIgnoredPaths)) continue

        const children = dirChildrenByPath.get(dirPath)
        if (!children || children.length === 0) continue

        const gitignoreNode = children.find(
          (child) => child.kind === "file" && child.name === ".gitignore"
        )
        if (!gitignoreNode || gitignoreNode.kind !== "file") continue

        try {
          const result = await readFilePreview(folder.path, gitignoreNode.path)
          const matcher = ignore().add(result.content)

          // Collect all descendant nodes so multi-level patterns like
          // "public/vs" can be matched using relative paths.
          const descendants: FileTreeNode[] = []
          const collectDescendants = (parent: string) => {
            const items = dirChildrenByPath.get(parent)
            if (!items) return
            for (const item of items) {
              descendants.push(item)
              if (item.kind === "dir") collectDescendants(item.path)
            }
          }
          collectDescendants(dirPath)

          for (const desc of descendants) {
            if (hasIgnoredAncestor(desc.path, nextIgnoredPaths)) continue
            const relativePath =
              dirPath === "" ? desc.path : desc.path.slice(dirPath.length + 1)
            if (!relativePath) continue
            const ignored =
              desc.kind === "dir"
                ? matcher.ignores(`${relativePath}/`) ||
                  matcher.ignores(`${relativePath}/.codeg-ignore-probe`)
                : matcher.ignores(relativePath)
            if (ignored) {
              nextIgnoredPaths.add(desc.path)
            }
          }
        } catch {
          // Ignore parser/read failures for non-critical visual hints.
        }
      }

      if (!canceled) {
        setGitignoreIgnoredPaths(nextIgnoredPaths)
      }
    }

    void loadIgnoredPaths()

    return () => {
      canceled = true
    }
  }, [dirChildrenByPath, expandedDirPaths, folder?.path])

  const gitChangedDirPaths = useMemo(() => {
    const dirs = new Set<string>()
    for (const filePath of gitStatusByPath.keys()) {
      let current = filePath
      // Walk up the path collecting all parent directories
      while (true) {
        const slashIdx = current.lastIndexOf("/")
        const backslashIdx = current.lastIndexOf("\\")
        const splitIdx = Math.max(slashIdx, backslashIdx)
        if (splitIdx <= 0) break
        current = current.slice(0, splitIdx)
        dirs.add(current)
      }
    }
    return dirs
  }, [gitStatusByPath])

  // Directories that are entirely untracked (from git status -unormal)
  const untrackedDirPaths = useMemo(() => {
    const dirs = new Set<string>()
    for (const [path, status] of gitStatusByPath.entries()) {
      if (status.trim() === "??") {
        // Check if this path is a directory in the file tree
        if (dirChildrenByPath.has(path)) {
          dirs.add(path)
        }
      }
    }
    return dirs
  }, [gitStatusByPath, dirChildrenByPath])

  const handleTreeSelect = useCallback(
    (path: string) => {
      if (!filePathSet.has(path)) return
      void openFilePreview(path)
    },
    [filePathSet, openFilePreview]
  )

  const handleOpenDirInTerminal = useCallback(
    async (dirPath: string, fileName: string) => {
      const terminalTitle = t("terminalTitle", { name: baseName(fileName) })
      const terminalId = await createTerminalInDirectory(dirPath, terminalTitle)
      if (!terminalId) {
        toast.error(t("toasts.openBuiltinTerminalFailed"))
      }
    },
    [createTerminalInDirectory, t]
  )

  const handleOpenCommitWindow = useCallback(() => {
    if (!folder) return
    openCommitWindow(folder.id).catch((error) => {
      const message = toErrorMessage(error)
      toast.error(t("toasts.openCommitWindowFailed"), {
        description: message,
      })
    })
  }, [folder, t])

  const handleRequestCreate = useCallback(
    (parentPath: string, kind: "file" | "dir") => {
      setCreateParentPath(parentPath)
      setCreateKind(kind)
      setCreateName("")
    },
    []
  )

  const handleRequestRename = useCallback((target: FileActionTarget) => {
    setRenameTarget(target)
    setRenameValue(target.name)
  }, [])

  const handleRequestDelete = useCallback((target: FileActionTarget) => {
    setDeleteTarget(target)
  }, [])

  // ─── Web upload / download (issue #179) ───
  // In web mode the user has no native file dialog, so the file-tree
  // context menu opens `WorkspaceUploadDialog`, which owns the queue,
  // progress UI, and cancellation. We only track which directory the
  // user right-clicked from and whether the dialog is open.
  const [webMode, setWebMode] = useState(false)
  // `webkitdirectory` is non-standard. Chromium, Edge, Firefox, and
  // desktop Safari support it; iOS Safari does not, and historically
  // some embedded webviews lacked it too. Feature-detect at mount and
  // hide the "Select folder" affordance where the picker would silently
  // fall back to single-file selection — that would surprise the user
  // mid-flow and risk corrupting the relative-path contract.
  const [folderUploadSupported, setFolderUploadSupported] = useState(false)
  const [uploadDialogOpen, setUploadDialogOpen] = useState(false)
  const [uploadDialogTarget, setUploadDialogTarget] = useState("")
  useEffect(() => {
    // "webMode" here is a misnomer for "needs in-app upload/download
    // affordances because there's no native OS file picker for the
    // *destination/source* filesystem". That's true in pure-web mode
    // AND in remote-desktop mode (where the workspace lives on the
    // remote server, not on the local disk the OS dialog would target).
    setWebMode(!isDesktop() || isRemoteDesktopMode())
    setFolderUploadSupported(
      "webkitdirectory" in document.createElement("input")
    )
  }, [])

  const handleRequestUpload = useCallback((targetPath: string) => {
    setUploadDialogTarget(targetPath)
    setUploadDialogOpen(true)
  }, [])

  const handleUploadComplete = useCallback(() => {
    void fetchTree()
  }, [fetchTree])

  const handleRequestDownloadFile = useCallback(
    async (target: FileActionTarget) => {
      const folderPath = folder?.path
      if (!folderPath) return
      try {
        const result = await downloadWorkspaceFile(
          folderPath,
          target.path,
          target.name
        )
        // Remote-desktop downloads flow through a save-dialog; surface
        // the cancel-vs-saved outcome instead of silently doing nothing.
        if (result.status === "started") return
        if (result.status === WORKSPACE_DOWNLOAD_CANCELLED) return
        if (result.savedPath) {
          toast.success(t("toasts.downloadSaved", { name: target.name }), {
            description: result.savedPath,
          })
        }
      } catch (error) {
        const message = toErrorMessage(error)
        toast.error(t("toasts.downloadFailed", { name: target.name }), {
          description: message,
        })
      }
    },
    [folder?.path, t]
  )

  const handleRequestDownloadDir = useCallback(
    async (target: FileActionTarget) => {
      const folderPath = folder?.path
      if (!folderPath) return
      const name = target.name || baseName(folderPath) || "workspace"
      try {
        const result = await downloadWorkspaceDir(folderPath, target.path, name)
        if (result.status === "started") return
        if (result.status === WORKSPACE_DOWNLOAD_CANCELLED) return
        if (result.savedPath) {
          toast.success(t("toasts.downloadSaved", { name }), {
            description: result.savedPath,
          })
        }
      } catch (error) {
        const message = toErrorMessage(error)
        toast.error(t("toasts.downloadFailed", { name }), {
          description: message,
        })
      }
    },
    [folder?.path, t]
  )

  const resetDirectoryGitActionDialog = useCallback(() => {
    setDirectoryGitActionType(null)
    setDirectoryGitActionTarget(null)
    setDirectoryGitCandidates([])
    setDirectoryGitSelectedPaths(new Set())
    setDirectoryGitExpandedPaths(new Set([DIRECTORY_GIT_TREE_ROOT_PATH]))
    setDirectoryGitError(null)
    setDirectoryGitLoading(false)
    setDirectoryGitSubmitting(false)
  }, [])

  const openDirectoryGitActionDialog = useCallback(
    async (action: DirectoryGitAction, target: FileActionTarget) => {
      if (!folder?.path) return
      setDirectoryGitActionType(action)
      setDirectoryGitActionTarget(target)
      setDirectoryGitCandidates([])
      setDirectoryGitSelectedPaths(new Set())
      setDirectoryGitExpandedPaths(new Set([DIRECTORY_GIT_TREE_ROOT_PATH]))
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
              : t("toasts.noRollbackFilesInDir")
          )
          return
        }

        const treeNodes = buildDirectoryGitTree(candidates, target.path)
        const expanded = collectDirectoryGitTreeExpandedPaths(treeNodes)
        expanded.add(DIRECTORY_GIT_TREE_ROOT_PATH)

        setDirectoryGitCandidates(candidates)
        setDirectoryGitSelectedPaths(
          new Set(candidates.map((entry) => entry.path))
        )
        setDirectoryGitExpandedPaths(expanded)
      } catch (error) {
        const message = toErrorMessage(error)
        setDirectoryGitError(message)
      } finally {
        setDirectoryGitLoading(false)
      }
    },
    [folder?.path, resetDirectoryGitActionDialog, t]
  )

  const handleRequestRollback = useCallback(
    (target: FileActionTarget) => {
      if (target.kind === "dir") {
        void openDirectoryGitActionDialog("rollback", target)
        return
      }
      setRollbackTarget(target)
    },
    [openDirectoryGitActionDialog]
  )

  const handleAddToVcs = useCallback(
    async (target: FileActionTarget) => {
      if (target.kind === "dir") {
        await openDirectoryGitActionDialog("add", target)
        return
      }
      if (!folder?.path) return
      try {
        await gitAddFiles(folder.path, [target.path])
        toast.success(t("toasts.addedToVcs", { name: target.name }))
        await fetchTree()
      } catch (error) {
        const message = toErrorMessage(error)
        toast.error(t("toasts.addToVcsFailed"), { description: message })
      }
    },
    [fetchTree, folder?.path, openDirectoryGitActionDialog, t]
  )

  const loadCompareBranches = useCallback(async () => {
    if (!folder?.path) {
      setCompareBranchList({ local: [], remote: [], worktree_branches: [] })
      setCompareCurrentBranch(null)
      return
    }
    setCompareBranchLoading(true)
    try {
      const [branchesResult, currentBranchResult] = await Promise.allSettled([
        gitListAllBranches(folder.path),
        getGitBranch(folder.path),
      ])

      if (branchesResult.status === "fulfilled") {
        setCompareBranchList(branchesResult.value)
      } else {
        setCompareBranchList({ local: [], remote: [], worktree_branches: [] })
        const message =
          branchesResult.reason instanceof Error
            ? branchesResult.reason.message
            : String(branchesResult.reason)
        toast.error(t("toasts.loadBranchesFailed"), { description: message })
      }

      if (currentBranchResult.status === "fulfilled") {
        setCompareCurrentBranch(currentBranchResult.value)
      } else {
        setCompareCurrentBranch(null)
      }
    } catch (error) {
      setCompareBranchList({ local: [], remote: [], worktree_branches: [] })
      setCompareCurrentBranch(null)
      const message = toErrorMessage(error)
      toast.error(t("toasts.loadBranchesFailed"), { description: message })
    } finally {
      setCompareBranchLoading(false)
    }
  }, [folder?.path, t])

  const handleRequestCompareWithBranch = useCallback(
    (target: FileActionTarget) => {
      setCompareTarget(target)
      setCompareBranchFilter("")
      setCompareRecentOpen(true)
      setCompareLocalOpen(false)
      setCompareRemoteOpen(false)
      void loadCompareBranches()
    },
    [loadCompareBranches]
  )

  const compareFilterKeyword = useMemo(
    () => compareBranchFilter.trim().toLowerCase(),
    [compareBranchFilter]
  )

  const filteredCompareRecentBranches = useMemo(() => {
    if (!compareCurrentBranch) return []
    if (!compareFilterKeyword) return [compareCurrentBranch]
    return compareCurrentBranch.toLowerCase().includes(compareFilterKeyword)
      ? [compareCurrentBranch]
      : []
  }, [compareCurrentBranch, compareFilterKeyword])

  const filteredCompareBranches = useMemo(() => {
    if (!compareFilterKeyword) {
      return compareBranchList
    }

    return {
      local: compareBranchList.local.filter((branch) =>
        branch.toLowerCase().includes(compareFilterKeyword)
      ),
      remote: compareBranchList.remote.filter((branch) =>
        branch.toLowerCase().includes(compareFilterKeyword)
      ),
    }
  }, [compareBranchList, compareFilterKeyword])

  const groupedCompareRemoteBranches = useMemo(() => {
    const groups: Record<string, string[]> = {}
    for (const b of filteredCompareBranches.remote) {
      const slashIndex = b.indexOf("/")
      const remoteName = slashIndex > 0 ? b.substring(0, slashIndex) : "origin"
      if (!groups[remoteName]) groups[remoteName] = []
      groups[remoteName].push(b)
    }
    return groups
  }, [filteredCompareBranches.remote])
  const compareRemoteNames = Object.keys(groupedCompareRemoteBranches)
  const hasMultipleCompareRemotes = compareRemoteNames.length > 1

  const directoryGitTreeNodes = useMemo(() => {
    if (!directoryGitActionTarget) return []
    return buildDirectoryGitTree(
      directoryGitCandidates,
      directoryGitActionTarget.path
    )
  }, [directoryGitActionTarget, directoryGitCandidates])

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

  const directoryGitFilePathSet = useMemo(
    () => new Set(directoryGitAllFilePaths),
    [directoryGitAllFilePaths]
  )

  const directoryGitLeafPathsByDirPath = useMemo(() => {
    const next = new Map<string, string[]>()
    const collect = (node: DirectoryGitTreeNode) => {
      if (node.kind === "file") return
      next.set(node.path, collectDirectoryGitTreeLeafPaths(node))
      for (const child of node.children) {
        if (child.kind === "dir") collect(child)
      }
    }
    for (const node of directoryGitTreeNodes) {
      if (node.kind === "dir") collect(node)
    }
    return next
  }, [directoryGitTreeNodes])

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

  const handleToggleDirectoryGitSelectAll = useCallback(() => {
    setDirectoryGitSelectedPaths((prev) => {
      if (
        directoryGitAllFilePaths.length > 0 &&
        directoryGitAllFilePaths.every((path) => prev.has(path))
      ) {
        return new Set<string>()
      }
      return new Set(directoryGitAllFilePaths)
    })
  }, [directoryGitAllFilePaths])

  const handleToggleDirectoryGitDir = useCallback(
    (dirPath: string) => {
      const leafPaths = directoryGitLeafPathsByDirPath.get(dirPath) ?? []
      if (leafPaths.length === 0) return
      setDirectoryGitSelectedPaths((prev) => {
        const next = new Set(prev)
        const allSelected = leafPaths.every((path) => next.has(path))
        if (allSelected) {
          for (const path of leafPaths) next.delete(path)
        } else {
          for (const path of leafPaths) next.add(path)
        }
        return next
      })
    },
    [directoryGitLeafPathsByDirPath]
  )

  const handleDirectoryGitTreeSelect = useCallback(
    (path: string) => {
      if (path === DIRECTORY_GIT_TREE_ROOT_PATH) {
        handleToggleDirectoryGitSelectAll()
        return
      }

      if (directoryGitLeafPathsByDirPath.has(path)) {
        handleToggleDirectoryGitDir(path)
        return
      }

      if (directoryGitFilePathSet.has(path)) {
        handleToggleDirectoryGitFile(path)
      }
    },
    [
      directoryGitFilePathSet,
      directoryGitLeafPathsByDirPath,
      handleToggleDirectoryGitDir,
      handleToggleDirectoryGitFile,
      handleToggleDirectoryGitSelectAll,
    ]
  )

  const renderDirectoryGitTreeNode = useCallback(
    (node: DirectoryGitTreeNode): ReactNode => {
      if (node.kind === "dir") {
        const leafPaths = directoryGitLeafPathsByDirPath.get(node.path) ?? []
        const allSelected =
          leafPaths.length > 0 &&
          leafPaths.every((path) => directoryGitSelectedPaths.has(path))
        const partiallySelected =
          !allSelected &&
          leafPaths.some((path) => directoryGitSelectedPaths.has(path))
        return (
          <FileTreeFolder
            key={node.path}
            path={node.path}
            name={`${allSelected ? "[x]" : partiallySelected ? "[-]" : "[ ]"} ${node.name}`}
            suffix={`(${node.fileCount})`}
            suffixClassName="text-muted-foreground/45"
            title={node.path}
          >
            {node.children.map(renderDirectoryGitTreeNode)}
          </FileTreeFolder>
        )
      }

      const selected = directoryGitSelectedPaths.has(node.path)
      return (
        <FileTreeFile
          key={node.path}
          path={node.path}
          name={node.name}
          className="gap-1 px-1.5 py-1"
          title={node.path}
        >
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation()
              handleToggleDirectoryGitFile(node.path)
            }}
            className={
              selected
                ? "flex h-4 w-4 shrink-0 items-center justify-center rounded border border-primary bg-primary text-primary-foreground transition-colors"
                : "flex h-4 w-4 shrink-0 items-center justify-center rounded border border-input transition-colors"
            }
            aria-label={t("aria.selectPath", {
              action: selected ? t("actions.unselect") : t("actions.select"),
              path: node.path,
            })}
            disabled={directoryGitSubmitting}
          >
            {selected && <Check className="h-3 w-3" />}
          </button>
          <button
            type="button"
            className="flex-1 truncate text-left"
            onClick={(event) => {
              event.stopPropagation()
              handleToggleDirectoryGitFile(node.path)
            }}
            title={node.path}
            disabled={directoryGitSubmitting}
          >
            {node.name}
          </button>
          <span className="w-8 shrink-0 text-right text-[10px] font-medium text-muted-foreground">
            {node.status}
          </span>
        </FileTreeFile>
      )
    },
    [
      directoryGitLeafPathsByDirPath,
      directoryGitSelectedPaths,
      directoryGitSubmitting,
      handleToggleDirectoryGitFile,
      t,
    ]
  )

  const handleCreateConfirm = useCallback(async () => {
    if (!folder?.path || createParentPath === null) return
    const trimmedName = createName.trim()
    if (!trimmedName) {
      setCreateParentPath(null)
      return
    }

    setCreating(true)
    try {
      await createFileTreeEntry(
        folder.path,
        createParentPath,
        trimmedName,
        createKind
      )
      setCreateParentPath(null)
      setCreateName("")
      await fetchTree()
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.createFailed"), { description: message })
    } finally {
      setCreating(false)
    }
  }, [createKind, createName, createParentPath, fetchTree, folder?.path, t])

  const handleRenameConfirm = useCallback(async () => {
    if (!folder?.path || !renameTarget) return
    const nextName = renameValue.trim()
    if (!nextName || nextName === renameTarget.name) {
      setRenameTarget(null)
      return
    }

    setRenaming(true)
    try {
      await renameFileTreeEntry(folder.path, renameTarget.path, nextName)
      setRenameTarget(null)
      setRenameValue("")
      await fetchTree()
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.renameFailed"), { description: message })
    } finally {
      setRenaming(false)
    }
  }, [fetchTree, folder?.path, renameTarget, renameValue, t])

  const handleDeleteConfirm = useCallback(async () => {
    if (!folder?.path || !deleteTarget) return
    setDeleting(true)
    try {
      await deleteFileTreeEntry(folder.path, deleteTarget.path)
      setDeleteTarget(null)
      await fetchTree()
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.deleteFailed"), { description: message })
    } finally {
      setDeleting(false)
    }
  }, [deleteTarget, fetchTree, folder?.path, t])

  const handleRollbackConfirm = useCallback(async () => {
    if (!folder?.path || !rollbackTarget) return
    setRollingBack(true)
    try {
      await gitRollbackFile(folder.path, rollbackTarget.path)
      toast.success(t("toasts.rolledBack", { name: rollbackTarget.name }))
      setRollbackTarget(null)
      await fetchTree()
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.rollbackFailed"), { description: message })
    } finally {
      setRollingBack(false)
    }
  }, [fetchTree, folder?.path, rollbackTarget, t])

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
      await fetchTree()
    } catch (error) {
      const message = toErrorMessage(error)
      setDirectoryGitError(message)
      toast.error(
        directoryGitActionType === "add"
          ? t("toasts.addToVcsFailed")
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
    fetchTree,
    folder?.path,
    resetDirectoryGitActionDialog,
    t,
  ])

  const handleCompareBranchClick = useCallback(
    async (branch: string) => {
      const nextBranch = branch.trim()
      if (!compareTarget || !nextBranch || comparing) return
      setComparing(true)
      try {
        if (compareTarget.kind === "dir") {
          await openBranchDiff(nextBranch, compareTarget.path, {
            mode: "overview",
          })
        } else {
          await openBranchDiff(nextBranch, compareTarget.path)
        }
        setCompareTarget(null)
        setCompareBranchFilter("")
        setCompareCurrentBranch(null)
      } finally {
        setComparing(false)
      }
    },
    [compareTarget, comparing, openBranchDiff]
  )

  const handleCompareExternalConflict = useCallback(() => {
    if (!externalConflictPrompt) return

    const latestTab = activeFileTabRef.current
    const unsavedContent =
      latestTab &&
      latestTab.kind === "file" &&
      latestTab.path &&
      normalizeComparePath(latestTab.path) ===
        normalizeComparePath(externalConflictPrompt.path) &&
      !latestTab.loading
        ? latestTab.content
        : externalConflictPrompt.unsavedContent

    openExternalConflictDiff(
      externalConflictPrompt.path,
      externalConflictPrompt.diskContent,
      unsavedContent
    )
    setExternalConflictPrompt(null)
  }, [externalConflictPrompt, openExternalConflictDiff])

  const handleReloadExternalConflict = useCallback(() => {
    if (!externalConflictPrompt) return
    externalConflictSignatureByPathRef.current.delete(
      externalConflictPrompt.path
    )
    setExternalConflictPrompt(null)
    void openFilePreview(externalConflictPrompt.path, { reload: true })
  }, [externalConflictPrompt, openFilePreview])

  const handleSaveExternalConflictCopy = useCallback(async () => {
    if (!folder?.path || !externalConflictPrompt) return

    const latestTab = activeFileTabRef.current
    const unsavedContent =
      latestTab &&
      latestTab.kind === "file" &&
      latestTab.path &&
      normalizeComparePath(latestTab.path) ===
        normalizeComparePath(externalConflictPrompt.path) &&
      !latestTab.loading
        ? latestTab.content
        : externalConflictPrompt.unsavedContent

    setSavingExternalConflictCopy(true)
    try {
      const result = await saveFileCopy(
        folder.path,
        externalConflictPrompt.path,
        unsavedContent
      )
      toast.success(t("toasts.savedAsCopy"), {
        description: result.path,
      })
      setExternalConflictPrompt(null)
      void fetchTree({ silent: true })
    } catch (error) {
      const message = toErrorMessage(error)
      toast.error(t("toasts.saveCopyFailed"), { description: message })
    } finally {
      setSavingExternalConflictCopy(false)
    }
  }, [externalConflictPrompt, fetchTree, folder?.path, t])

  const rootNodeName = useMemo(() => {
    if (!folder?.path) return t("workspace")
    return baseName(folder.path)
  }, [folder?.path, t])

  const systemExplorerLabel =
    typeof navigator === "undefined"
      ? t("openInFileManager")
      : (() => {
          const platform =
            `${navigator.platform} ${navigator.userAgent}`.toLowerCase()
          if (platform.includes("mac")) return t("openInFinder")
          if (platform.includes("win")) return t("openInExplorer")
          return t("openInFileManager")
        })()

  const rootTarget: FileActionTarget = useMemo(
    () => ({ kind: "dir", path: "", name: rootNodeName }),
    [rootNodeName]
  )

  type FileChangeDecision =
    | { kind: "none" }
    | { kind: "reload"; path: string; latest: FileEditContent }
    | {
        kind: "conflict"
        path: string
        diskContent: string
        unsavedContent: string
        signature: string
      }
    | { kind: "missing"; path: string; error: string }

  // Per-tab disk-vs-buffer resolver. Compares the tab's known etag against
  // the latest disk read for the same path. Independent of activation —
  // callable for any open file tab so the watcher can scan inactive tabs
  // too. Re-reads the tab from `fileTabsRef.current` after the fetch to
  // guard against close/reopen races during the async window. The `reload`
  // decision carries the fetched FileEditContent so the watcher can write
  // it directly via applyExternalReload, avoiding a second readFileForEdit.
  const resolveFileChangeDecision = useCallback(
    async (tabSnapshot: FileWorkspaceTab): Promise<FileChangeDecision> => {
      const rootPath = folder?.path
      if (!rootPath) return { kind: "none" }
      if (tabSnapshot.kind !== "file") return { kind: "none" }
      const path = tabSnapshot.path
      if (!path) return { kind: "none" }
      if (tabSnapshot.loading) return { kind: "none" }

      const tabId = tabSnapshot.id

      const stillSameTab = (): FileWorkspaceTab | null => {
        const latestTab = fileTabsRef.current.find((t) => t.id === tabId)
        if (!latestTab || latestTab.kind !== "file") return null
        if (
          normalizeComparePath(latestTab.path ?? "") !==
          normalizeComparePath(path)
        ) {
          return null
        }
        if (latestTab.loading) return null
        return latestTab
      }

      let latest: FileEditContent
      try {
        latest = await readFileForEdit(rootPath, path)
      } catch (error) {
        // Disk read failed — most commonly an external delete, but also
        // permission revocation, an exclusive lock, or a transient FS
        // error. Surface this as its own decision: the watcher routes it
        // to rejectFileTab (clean) or markTabsStale (dirty) so the user
        // is never silently shown a buffer that no longer matches disk.
        const latestTab = stillSameTab()
        if (!latestTab) return { kind: "none" }
        return { kind: "missing", path, error: toErrorMessage(error) }
      }

      const latestTab = stillSameTab()
      if (!latestTab) return { kind: "none" }

      const latestTabEtag = latestTab.etag ?? null
      if (latest.etag === latestTabEtag) return { kind: "none" }

      if (latestTab.isDirty) {
        return {
          kind: "conflict",
          path,
          diskContent: latest.content,
          unsavedContent: latestTab.content,
          signature: `${path}:${latest.etag}`,
        }
      }

      return { kind: "reload", path, latest }
    },
    [folder?.path]
  )

  // Surface a conflict prompt for `decision`, deduping by signature so a
  // repeated workspace event or activation transition does not flicker
  // the UI.
  const announceConflict = useCallback(
    (decision: Extract<FileChangeDecision, { kind: "conflict" }>) => {
      const shownSignature = externalConflictSignatureByPathRef.current.get(
        decision.path
      )
      if (shownSignature === decision.signature) return
      externalConflictSignatureByPathRef.current.set(
        decision.path,
        decision.signature
      )
      setExternalConflictPrompt((current) => {
        if (current?.signature === decision.signature) return current
        return {
          path: decision.path,
          diskContent: decision.diskContent,
          unsavedContent: decision.unsavedContent,
          signature: decision.signature,
        }
      })
    },
    []
  )

  // Envelope-driven change watcher.
  //
  // Replaces the prior workspaceState.seq polling loop, which forced a
  // full open-tab scan on every workspace event and read each candidate
  // file twice (resolver + reload). The new path:
  //   • subscribes to envelopes and uses `changed_paths` to scope work to
  //     tabs whose paths actually changed;
  //   • falls back to a full sweep only on `resync_hint` or envelopes
  //     missing changed_paths, so external changes cannot be lost when
  //     the backend cannot enumerate paths;
  //   • coalesces bursts (e.g. git checkout) via queueMicrotask and a
  //     single in-flight drainer, so concurrent reconciliations cannot
  //     double-fire;
  //   • skips image tabs (no etag — would otherwise trigger spurious
  //     full base64 re-reads every workspace event);
  //   • re-reads activeFileTabRef.current AFTER each per-tab resolve
  //     await so a user mid-scan tab switch is honored;
  //   • dispatches reload via applyExternalReload — handing the prefetched
  //     content from the resolver to the context — instead of triggering
  //     a second readFileForEdit through openFilePreview.
  useEffect(() => {
    if (!folder?.path) return
    if (!subscribeWorkspaceEnvelopes) return

    const pendingPaths = new Set<string>()
    let pendingFullScan = false
    let flushScheduled = false
    let flushPromise: Promise<void> | null = null
    let disposed = false

    const reconcileChanges = async (
      paths: string[],
      fullScan: boolean
    ): Promise<void> => {
      const openFileTabs = fileTabsRef.current.filter(
        (t) => t.kind === "file" && t.path && !t.loading
      )

      const candidates = (() => {
        if (fullScan) return openFileTabs
        const pathSet = new Set(paths.map(normalizeComparePath))
        return openFileTabs.filter(
          (t) => t.path && pathSet.has(normalizeComparePath(t.path))
        )
      })()

      for (const tab of candidates) {
        if (disposed) return
        const path = tab.path
        if (!path) continue

        // Image tabs do not carry an etag and load via readFileBase64.
        // Bypass the text-file resolver: a single path-match is enough
        // to trigger a refresh, and only when changed_paths actually
        // names the image (full-scan covers the resync fallback).
        if (isImageFile(path)) {
          void reloadOpenFileBackground(path)
          continue
        }

        const decision = await resolveFileChangeDecision(tab)
        if (disposed) return
        if (decision.kind === "none") continue

        // CRITICAL: re-read active tab id AFTER the resolver await. If
        // we used a snapshot captured before the loop, a tab the user
        // switched away from could be silently re-activated by a
        // foreground reload — which is the exact bug this guards.
        const isActive = tab.id === (activeFileTabRef.current?.id ?? null)

        if (decision.kind === "reload") {
          // Either active or inactive: applyExternalReload writes the
          // prefetched payload without changing activeFileTabId, so the
          // user's focus is preserved either way and no second
          // readFileForEdit fires.
          externalConflictSignatureByPathRef.current.delete(decision.path)
          void applyExternalReload(decision.path, decision.latest)
          continue
        }

        if (decision.kind === "missing") {
          // Disk read failed on a path that an envelope flagged as
          // changed — the file is gone, locked, or otherwise unreadable.
          // Clean tab: reject to error state so the user is never shown a
          // stale buffer that no longer corresponds to disk. Dirty tab:
          // mark stale to preserve unsaved edits; the next save attempt
          // (or activation transition) surfaces the failure.
          externalConflictSignatureByPathRef.current.delete(decision.path)
          const liveTab = fileTabsRef.current.find((t) => t.id === tab.id)
          if (liveTab?.isDirty) {
            markTabsStale(decision.path)
          } else {
            rejectFileTab(decision.path, decision.error)
          }
          continue
        }

        if (isActive) {
          announceConflict(decision)
        } else {
          markTabsStale(decision.path)
        }
      }
    }

    const ensureFlushing = () => {
      if (flushPromise || flushScheduled) return
      flushScheduled = true
      queueMicrotask(() => {
        flushScheduled = false
        if (disposed) return
        flushPromise = (async () => {
          try {
            // Drain anything pending — including envelopes that arrive
            // while we're awaiting reconcileChanges. ensureFlushing()
            // calls from those listener callbacks see flushPromise !=
            // null and return; their paths land in pendingPaths and the
            // next loop iteration picks them up.
            while (!disposed && (pendingPaths.size > 0 || pendingFullScan)) {
              const paths = Array.from(pendingPaths)
              pendingPaths.clear()
              const fullScan = pendingFullScan
              pendingFullScan = false
              await reconcileChanges(paths, fullScan)
            }
          } finally {
            flushPromise = null
          }
        })()
      })
    }

    const unsubscribe = subscribeWorkspaceEnvelopes(
      ({ changed_paths, kind }) => {
        if (
          kind === "resync_hint" ||
          !changed_paths ||
          changed_paths.length === 0
        ) {
          // Resync or untargeted event — we cannot scope work, so cover
          // every open tab. Targeted paths from later envelopes are
          // additive (full scan is a superset).
          pendingFullScan = true
        } else {
          for (const path of changed_paths) {
            pendingPaths.add(path)
          }
        }
        ensureFlushing()
      }
    )

    return () => {
      disposed = true
      unsubscribe()
      pendingPaths.clear()
    }
  }, [
    folder?.path,
    subscribeWorkspaceEnvelopes,
    resolveFileChangeDecision,
    applyExternalReload,
    reloadOpenFileBackground,
    markTabsStale,
    rejectFileTab,
    announceConflict,
  ])

  // Stale-on-activation: when the user switches to (or just opened) a tab
  // that the watcher previously flagged stale, fire the appropriate action
  // now — without waiting for the next workspace event. Clean stale is
  // promoted to reload by openFilePreview's decideLoad path; this effect
  // covers dirty stale (conflict prompt) and the defensive fallback for
  // clean stale that somehow survived activation (e.g. switchFileTab).
  useEffect(() => {
    const tab = activeFileTab
    if (!tab || tab.kind !== "file" || !tab.path) return
    if (!tab.stale || tab.loading) return

    const path = tab.path
    if (!tab.isDirty) {
      void openFilePreview(path, { reload: true })
      return
    }

    void (async () => {
      const decision = await resolveFileChangeDecision(tab)
      if (decision.kind === "conflict") {
        announceConflict(decision)
      } else if (decision.kind === "reload") {
        void applyExternalReload(decision.path, decision.latest)
      } else if (decision.kind === "missing") {
        // File vanished while the dirty buffer sat in a non-active tab.
        // The buffer is still dirty here (this branch only runs for
        // tab.isDirty === true) so we keep markTabsStale on the path —
        // refusing to silently lose the user's unsaved edits. The user
        // discovers the deletion on save (backend recreates or errors).
        markTabsStale(decision.path)
      }
    })()
  }, [
    activeFileTab,
    openFilePreview,
    applyExternalReload,
    resolveFileChangeDecision,
    announceConflict,
    markTabsStale,
  ])

  if (!folder) {
    return <AuxPanelNoFolderEmpty />
  }

  if (loading && nodes.length === 0) {
    return (
      <div className="p-3 space-y-2">
        <Skeleton className="h-4 w-3/4" />
        <Skeleton className="h-4 w-1/2 ml-4" />
        <Skeleton className="h-4 w-2/3 ml-4" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-3/4 ml-4" />
      </div>
    )
  }

  if (error) {
    return (
      <div className="p-3 text-xs text-destructive">
        <p>{error}</p>
        <Button
          variant="ghost"
          size="xs"
          className="mt-2"
          onClick={() => {
            void fetchTree()
          }}
        >
          {t("retry")}
        </Button>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full">
      {workspaceState.degraded && (
        <WorkspaceDegradedBanner onRetry={workspaceState.restart} />
      )}
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <ScrollArea className="flex-1 min-h-0 pb-1" x="scroll">
            <FileTree
              key={folder?.path ?? "file-tree-empty"}
              className="border-0 rounded-none bg-transparent w-max min-w-full"
              expanded={expandedPaths}
              onExpandedChange={setExpandedPaths}
              selectedPath={activeFilePath ?? undefined}
              onSelect={handleTreeSelect}
            >
              {folder?.path && (
                <ContextMenu>
                  <ContextMenuTrigger>
                    <FileTreeFolder
                      path={FILE_TREE_ROOT_PATH}
                      name={rootNodeName}
                      className="font-medium"
                    >
                      {nodes.map((node) => (
                        <RenderNode
                          key={node.path}
                          node={node}
                          expandedPaths={expandedPaths}
                          workspacePath={folder.path}
                          activeSessionTabId={activeSessionTabId}
                          gitEnabled={gitEnabled}
                          webMode={webMode}
                          folderUploadSupported={folderUploadSupported}
                          gitStatusByPath={gitStatusByPath}
                          gitChangedDirPaths={gitChangedDirPaths}
                          untrackedDirPaths={untrackedDirPaths}
                          gitignoreIgnoredPaths={gitignoreIgnoredPaths}
                          ancestorGitignoreIgnored={false}
                          ancestorUntracked={false}
                          onOpenFilePreview={(path) => {
                            void openFilePreview(path)
                          }}
                          onOpenFileDiff={(path) => {
                            void openWorkingTreeDiff(path)
                          }}
                          onOpenDirDiff={(path) => {
                            void openWorkingTreeDiff(path, {
                              mode: "overview",
                            })
                          }}
                          onOpenCommitWindow={handleOpenCommitWindow}
                          onRequestCompareWithBranch={
                            handleRequestCompareWithBranch
                          }
                          onRequestRollback={handleRequestRollback}
                          onOpenDirInTerminal={handleOpenDirInTerminal}
                          onRequestCreate={handleRequestCreate}
                          onRequestAddToVcs={handleAddToVcs}
                          onRequestRename={handleRequestRename}
                          onRequestDelete={handleRequestDelete}
                          onRequestUpload={handleRequestUpload}
                          onRequestDownloadFile={(target) =>
                            void handleRequestDownloadFile(target)
                          }
                          onRequestDownloadDir={(target) =>
                            void handleRequestDownloadDir(target)
                          }
                          onRefresh={fetchTree}
                        />
                      ))}
                    </FileTreeFolder>
                  </ContextMenuTrigger>
                  <ContextMenuContent>
                    <ContextMenuSub>
                      <ContextMenuSubTrigger>{t("new")}</ContextMenuSubTrigger>
                      <ContextMenuSubContent>
                        <ContextMenuItem
                          onSelect={() => handleRequestCreate("", "file")}
                        >
                          {t("newFile")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() => handleRequestCreate("", "dir")}
                        >
                          {t("newDirectory")}
                        </ContextMenuItem>
                      </ContextMenuSubContent>
                    </ContextMenuSub>
                    <ContextMenuSub>
                      <ContextMenuSubTrigger disabled={!gitEnabled}>
                        {t("git")}
                      </ContextMenuSubTrigger>
                      <ContextMenuSubContent>
                        <ContextMenuItem
                          onSelect={() => handleOpenCommitWindow()}
                          disabled={!gitEnabled}
                        >
                          {t("actions.commitCode")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() => void handleAddToVcs(rootTarget)}
                          disabled={!gitEnabled}
                        >
                          {t("actions.addToVcs")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() =>
                            void openWorkingTreeDiff(".", {
                              mode: "overview",
                            })
                          }
                          disabled={!gitEnabled}
                        >
                          {tCommon("viewDiff")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() =>
                            handleRequestCompareWithBranch(rootTarget)
                          }
                          disabled={!gitEnabled}
                        >
                          {t("compareWithBranch")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          variant="destructive"
                          onSelect={() => handleRequestRollback(rootTarget)}
                          disabled={!gitEnabled}
                        >
                          {t("actions.rollback")}
                        </ContextMenuItem>
                      </ContextMenuSubContent>
                    </ContextMenuSub>
                    <ContextMenuItem
                      onSelect={() => {
                        void fetchTree()
                      }}
                    >
                      {t("reloadFromDisk")}
                    </ContextMenuItem>
                    <ContextMenuSub>
                      <ContextMenuSubTrigger>
                        {t("openIn")}
                      </ContextMenuSubTrigger>
                      <ContextMenuSubContent>
                        <ContextMenuItem
                          onSelect={() => {
                            void revealItemInDir(folder.path)
                          }}
                        >
                          {systemExplorerLabel}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() => {
                            void handleOpenDirInTerminal(
                              folder.path,
                              rootNodeName
                            )
                          }}
                        >
                          {t("openInTerminal")}
                        </ContextMenuItem>
                      </ContextMenuSubContent>
                    </ContextMenuSub>
                    <ContextMenuItem
                      onSelect={() =>
                        void copyPathToClipboard(folder.path, {
                          success: t("toasts.pathCopied"),
                          failure: t("toasts.copyPathFailed"),
                        })
                      }
                    >
                      {t("copyPath")}
                    </ContextMenuItem>
                    {webMode && (
                      <>
                        <ContextMenuItem
                          onSelect={() => handleRequestUpload("")}
                        >
                          {t("upload")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onSelect={() =>
                            void handleRequestDownloadDir(rootTarget)
                          }
                        >
                          {t("downloadAsZip")}
                        </ContextMenuItem>
                      </>
                    )}
                  </ContextMenuContent>
                </ContextMenu>
              )}
            </FileTree>
          </ScrollArea>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuSub>
            <ContextMenuSubTrigger>{t("new")}</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItem onSelect={() => handleRequestCreate("", "file")}>
                {t("newFile")}
              </ContextMenuItem>
              <ContextMenuItem onSelect={() => handleRequestCreate("", "dir")}>
                {t("newDirectory")}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>
          {webMode && (
            <ContextMenuItem onSelect={() => handleRequestUpload("")}>
              {t("upload")}
            </ContextMenuItem>
          )}
          <ContextMenuItem
            onSelect={() => {
              void fetchTree()
            }}
          >
            {t("reloadFromDisk")}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      {webMode && folder?.path && (
        <WorkspaceUploadDialog
          open={uploadDialogOpen}
          onOpenChange={setUploadDialogOpen}
          rootPath={folder.path}
          targetPath={uploadDialogTarget}
          folderUploadSupported={folderUploadSupported}
          onComplete={handleUploadComplete}
        />
      )}

      <Dialog
        open={createParentPath !== null}
        onOpenChange={(open) => {
          if (open) return
          setCreateParentPath(null)
          setCreateName("")
        }}
      >
        <DialogContent
          onOpenAutoFocus={(e) => {
            e.preventDefault()
            const input = (
              e.currentTarget as HTMLElement | null
            )?.querySelector("input")
            if (input) requestAnimationFrame(() => input.focus())
          }}
        >
          <DialogHeader>
            <DialogTitle>
              {createKind === "dir"
                ? t("createDialog.newDirectory")
                : t("createDialog.newFile")}
            </DialogTitle>
            <DialogDescription>
              {t("createDialog.description", {
                kind:
                  createKind === "dir"
                    ? t("newDirectory").toLowerCase()
                    : t("newFile").toLowerCase(),
              })}
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => {
              event.preventDefault()
              void handleCreateConfirm()
            }}
            className="space-y-4"
          >
            <Input
              value={createName}
              onChange={(event) => setCreateName(event.target.value)}
              disabled={creating}
              placeholder={
                createKind === "dir"
                  ? t("createDialog.placeholderDirectory")
                  : t("createDialog.placeholderFile")
              }
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={creating}
                onClick={() => {
                  setCreateParentPath(null)
                  setCreateName("")
                }}
              >
                {tCommon("cancel")}
              </Button>
              <Button type="submit" disabled={creating || !createName.trim()}>
                {tCommon("create")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(renameTarget)}
        onOpenChange={(open) => {
          if (open) return
          setRenameTarget(null)
          setRenameValue("")
        }}
      >
        <DialogContent
          onOpenAutoFocus={(e) => {
            e.preventDefault()
            const input = (
              e.currentTarget as HTMLElement | null
            )?.querySelector("input")
            if (input) requestAnimationFrame(() => input.focus())
          }}
        >
          <DialogHeader>
            <DialogTitle>
              {renameTarget?.kind === "dir"
                ? t("renameDialog.renameDirectory")
                : t("renameDialog.renameFile")}
            </DialogTitle>
            <DialogDescription>
              {t("renameDialog.description")}
            </DialogDescription>
          </DialogHeader>
          <form
            onSubmit={(event) => {
              event.preventDefault()
              void handleRenameConfirm()
            }}
            className="space-y-4"
          >
            <Input
              value={renameValue}
              onChange={(event) => setRenameValue(event.target.value)}
              disabled={renaming}
              placeholder={
                renameTarget?.kind === "dir"
                  ? t("renameDialog.placeholderDirectory")
                  : t("renameDialog.placeholderFile")
              }
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={renaming}
                onClick={() => {
                  setRenameTarget(null)
                  setRenameValue("")
                }}
              >
                {tCommon("cancel")}
              </Button>
              <Button type="submit" disabled={renaming}>
                {tCommon("confirm")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

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
                : t("actions.rollback")}
            </DialogTitle>
            <DialogDescription>
              {directoryGitActionTarget
                ? directoryGitActionType === "add"
                  ? t("directoryDialog.descriptionAdd", {
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
              ) : directoryGitTreeNodes.length > 0 &&
                directoryGitActionTarget ? (
                <FileTree
                  className="text-xs [&>div]:p-1"
                  expanded={directoryGitExpandedPaths}
                  onSelect={handleDirectoryGitTreeSelect}
                  onExpandedChange={setDirectoryGitExpandedPaths}
                >
                  <FileTreeFolder
                    path={DIRECTORY_GIT_TREE_ROOT_PATH}
                    name={directoryGitActionTarget.name}
                    suffix={`(${directoryGitAllFilePaths.length})`}
                    suffixClassName="text-muted-foreground/45"
                    title={directoryGitActionTarget.path}
                  >
                    {directoryGitTreeNodes.map(renderDirectoryGitTreeNode)}
                  </FileTreeFolder>
                </FileTree>
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
                  directoryGitActionType === "rollback"
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
                  : t("actions.rollback")}
              </Button>
            </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(compareTarget)}
        onOpenChange={(open) => {
          if (open) return
          setCompareTarget(null)
          setCompareBranchFilter("")
          setCompareCurrentBranch(null)
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("compareDialog.title")}</DialogTitle>
            <DialogDescription>
              {compareTarget
                ? t("compareDialog.descriptionWithTarget", {
                    kind:
                      compareTarget.kind === "dir"
                        ? t("compareDialog.kindDirectory")
                        : t("compareDialog.kindFile"),
                    path: compareTarget.path,
                  })
                : t("compareDialog.descriptionFallback")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <Input
              value={compareBranchFilter}
              onChange={(event) => setCompareBranchFilter(event.target.value)}
              placeholder={t("compareDialog.filterPlaceholder")}
              autoFocus
              disabled={comparing}
            />
            <div className="text-xs text-muted-foreground">
              {t("compareDialog.singleClickHint")}
            </div>
            <div className="space-y-2">
              <div className="max-h-56 overflow-y-auto rounded-xl border p-2 space-y-3">
                {compareBranchLoading ? (
                  <div className="py-6 text-center text-xs text-muted-foreground">
                    {t("compareDialog.loadingBranches")}
                  </div>
                ) : (
                  <>
                    <Collapsible
                      open={compareRecentOpen}
                      onOpenChange={setCompareRecentOpen}
                    >
                      <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                        <ChevronRight className="h-3.5 w-3.5 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                        {t("compareDialog.recentBranches", {
                          count: filteredCompareRecentBranches.length,
                        })}
                      </CollapsibleTrigger>
                      <CollapsibleContent className="space-y-1 pt-1">
                        {filteredCompareRecentBranches.length > 0 ? (
                          filteredCompareRecentBranches.map((branch) => (
                            <Button
                              key={`recent-${branch}`}
                              type="button"
                              size="xs"
                              variant="ghost"
                              className="w-full justify-start"
                              onClick={() => {
                                void handleCompareBranchClick(branch)
                              }}
                              disabled={comparing}
                            >
                              {branch}
                            </Button>
                          ))
                        ) : (
                          <div className="px-2 text-xs text-muted-foreground">
                            {t("compareDialog.noCurrentBranch")}
                          </div>
                        )}
                      </CollapsibleContent>
                    </Collapsible>
                    <Collapsible
                      open={compareLocalOpen}
                      onOpenChange={setCompareLocalOpen}
                    >
                      <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                        <ChevronRight className="h-3.5 w-3.5 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                        {t("compareDialog.localBranches", {
                          count: filteredCompareBranches.local.length,
                        })}
                      </CollapsibleTrigger>
                      <CollapsibleContent className="space-y-1 pt-1">
                        {filteredCompareBranches.local.length > 0 ? (
                          filteredCompareBranches.local.map((branch) => (
                            <Button
                              key={`local-${branch}`}
                              type="button"
                              size="xs"
                              variant="ghost"
                              className="w-full justify-start"
                              onClick={() => {
                                void handleCompareBranchClick(branch)
                              }}
                              disabled={comparing}
                            >
                              {branch}
                            </Button>
                          ))
                        ) : (
                          <div className="px-2 text-xs text-muted-foreground">
                            {t("compareDialog.noMatchingBranches")}
                          </div>
                        )}
                      </CollapsibleContent>
                    </Collapsible>
                    <Collapsible
                      open={compareRemoteOpen}
                      onOpenChange={setCompareRemoteOpen}
                    >
                      <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                        <ChevronRight className="h-3.5 w-3.5 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                        {t("compareDialog.remoteBranches", {
                          count: filteredCompareBranches.remote.length,
                        })}
                      </CollapsibleTrigger>
                      <CollapsibleContent className="space-y-1 pt-1">
                        {filteredCompareBranches.remote.length > 0 ? (
                          hasMultipleCompareRemotes ? (
                            compareRemoteNames.map((remoteName) => (
                              <Collapsible key={remoteName}>
                                <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 pl-5 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                                  <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                                  {remoteName} (
                                  {
                                    groupedCompareRemoteBranches[remoteName]
                                      .length
                                  }
                                  )
                                </CollapsibleTrigger>
                                <CollapsibleContent className="space-y-1 pt-1 pl-3">
                                  {groupedCompareRemoteBranches[remoteName].map(
                                    (branch) => (
                                      <Button
                                        key={`remote-${branch}`}
                                        type="button"
                                        size="xs"
                                        variant="ghost"
                                        className="w-full justify-start"
                                        onClick={() => {
                                          void handleCompareBranchClick(branch)
                                        }}
                                        disabled={comparing}
                                      >
                                        {branch.substring(
                                          remoteName.length + 1
                                        )}
                                      </Button>
                                    )
                                  )}
                                </CollapsibleContent>
                              </Collapsible>
                            ))
                          ) : (
                            filteredCompareBranches.remote.map((branch) => {
                              const slashIndex = branch.indexOf("/")
                              const shortName =
                                slashIndex > 0
                                  ? branch.substring(slashIndex + 1)
                                  : branch
                              return (
                                <Button
                                  key={`remote-${branch}`}
                                  type="button"
                                  size="xs"
                                  variant="ghost"
                                  className="w-full justify-start pl-4"
                                  onClick={() => {
                                    void handleCompareBranchClick(branch)
                                  }}
                                  disabled={comparing}
                                >
                                  {shortName}
                                </Button>
                              )
                            })
                          )
                        ) : (
                          <div className="px-2 text-xs text-muted-foreground">
                            {t("compareDialog.noMatchingBranches")}
                          </div>
                        )}
                      </CollapsibleContent>
                    </Collapsible>
                  </>
                )}
              </div>
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={comparing}
                onClick={() => {
                  setCompareTarget(null)
                  setCompareBranchFilter("")
                  setCompareCurrentBranch(null)
                }}
              >
                {tCommon("cancel")}
              </Button>
            </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(externalConflictPrompt)}
        onOpenChange={(open) => {
          if (open) return
          setExternalConflictPrompt(null)
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("externalConflictDialog.title")}</DialogTitle>
            <DialogDescription>
              {externalConflictPrompt
                ? t("externalConflictDialog.descriptionWithPath", {
                    path: externalConflictPrompt.path,
                  })
                : t("externalConflictDialog.descriptionFallback")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={savingExternalConflictCopy}
              onClick={handleCompareExternalConflict}
            >
              {t("externalConflictDialog.compare")}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={savingExternalConflictCopy}
              onClick={() => {
                void handleSaveExternalConflictCopy()
              }}
            >
              {savingExternalConflictCopy
                ? t("externalConflictDialog.savingCopy")
                : t("externalConflictDialog.saveAsCopy")}
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={savingExternalConflictCopy}
              onClick={handleReloadExternalConflict}
            >
              {t("externalConflictDialog.reload")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

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
              {tCommon("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

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
    </div>
  )
}
