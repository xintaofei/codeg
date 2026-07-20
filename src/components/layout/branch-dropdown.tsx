"use client"

import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import {
  ArchiveRestore,
  Archive,
  ArrowDownToLine,
  ChevronDown,
  ChevronRight,
  FolderGit2,
  FolderOpen,
  GitBranch,
  GitBranchPlus,
  GitCommitHorizontal,
  GitFork,
  GitMerge,
  GitPullRequestArrow,
  Globe,
  Loader2,
  RefreshCw,
  Trash2,
  Upload,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  gitInit,
  gitPull,
  gitFetch,
  gitNewBranch,
  gitWorktreeAdd,
  gitListAllBranches,
  gitMerge,
  gitRebase,
  gitDeleteBranch,
  gitDeleteRemoteBranch,
  openCommitWindow,
  openPushWindow,
  openStashWindow,
} from "@/lib/api"
import { isDesktop, openFileDialog, subscribe } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { RemoteManageDialog } from "@/components/layout/remote-manage-dialog"
import { ConflictDialog } from "@/components/layout/conflict-dialog"
import { StashDialog } from "@/components/layout/stash-dialog"
import { DirectoryBrowserDialog } from "@/components/shared/directory-browser-dialog"
import { toErrorMessage } from "@/lib/app-error"
import { resolveFolderDisplayName } from "@/lib/folder-display"
import { useSwitchToBranch } from "@/hooks/use-switch-to-branch"
import {
  buildBranchTree,
  buildRemoteBranchSections,
  containsBranch,
  expandedKeysForBranch,
  localBranchItems,
  sectionKey,
  type BranchTreeLeaf,
  type BranchTreeNode,
} from "@/lib/branch-tree"
import { branchRowPaddingLeft } from "@/components/layout/branch-tree-collapsible"
import { useBranchTreeExpansion } from "@/hooks/use-branch-tree-expansion"
import { cn } from "@/lib/utils"
import type { GitBranchList, GitConflictInfo } from "@/lib/types"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useIsActiveChatMode } from "@/hooks/use-is-active-chat-mode"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions } from "@/contexts/tab-context"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useTaskContext } from "@/contexts/task-context"
import { useAlertContext } from "@/contexts/alert-context"
import { useGitCredential } from "@/contexts/git-credential-context"

const emitEvent = async (event: string, payload?: unknown) => {
  try {
    const { emit } = await import("@tauri-apps/api/event")
    await emit(event, payload)
  } catch {
    /* not in Tauri */
  }
}

type ConfirmAction = {
  type: "merge" | "rebase" | "delete" | "forceDelete" | "deleteRemote"
  branchName: string
}

interface GitCommitSucceededEventPayload {
  folder_id: number
  committed_files: number
}

interface GitPushSucceededEventPayload {
  folder_id: number
  pushed_commits: number
  upstream_set: boolean
}

export function BranchDropdown({ compact = false }: { compact?: boolean }) {
  const t = useTranslations("Folder.branchDropdown")
  const tCommon = useTranslations("Folder.common")
  const { activeFolder } = useActiveFolder()
  const isChatMode = useIsActiveChatMode()
  const allFolders = useAppWorkspaceStore((s) => s.allFolders)
  const refreshFolder = useAppWorkspaceStore((s) => s.refreshFolder)
  const openWorktreeFolder = useAppWorkspaceStore((s) => s.openWorktreeFolder)
  const { openNewConversationTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const { addTask, updateTask, removeTask } = useTaskContext()
  const { pushAlert } = useAlertContext()
  const { withCredentialRetry } = useGitCredential()
  const switchToBranch = useSwitchToBranch()

  const folderPath = activeFolder?.path ?? ""
  const folderId = activeFolder?.id ?? 0
  // Per-folder selections (primitive / equality-guarded object): unrelated
  // folders' branch updates never re-render this dropdown.
  const branch = useAppWorkspaceStore((s) =>
    activeFolder
      ? (s.branches.get(activeFolder.id) ?? activeFolder.git_branch ?? null)
      : null
  )
  const head = useAppWorkspaceStore((s) =>
    activeFolder ? (s.gitHeads.get(activeFolder.id) ?? null) : null
  )
  // The gate is "is this a git repo?" — not "is there a branch?". A detached
  // HEAD has no branch name yet is still a repo whose git operations must
  // remain available (issue #279). Until the first poll resolves `head`, fall
  // back to branch presence so the first-frame behavior is unchanged.
  const isRepo = head ? head.is_repo : branch !== null
  const isDetached = !branch && !!head?.detached

  const [branchList, setBranchList] = useState<GitBranchList>({
    local: [],
    remote: [],
    worktree_branches: [],
  })
  const [newBranchOpen, setNewBranchOpen] = useState(false)
  const [newBranchName, setNewBranchName] = useState("")
  const [loading, setLoading] = useState(false)
  const [dropdownOpen, setDropdownOpen] = useState(false)
  const [branchLoading, setBranchLoading] = useState(false)
  const [confirmAction, setConfirmAction] = useState<ConfirmAction | null>(null)
  const [worktreeOpen, setWorktreeOpen] = useState(false)
  const [worktreeBrowserOpen, setWorktreeBrowserOpen] = useState(false)
  const [worktreeBranchName, setWorktreeBranchName] = useState("")
  const [worktreePath, setWorktreePath] = useState("")
  const [manageRemotesOpen, setManageRemotesOpen] = useState(false)
  const [stashDialogOpen, setStashDialogOpen] = useState(false)
  const [conflictInfo, setConflictInfo] = useState<GitConflictInfo | null>(null)
  const taskSeq = useRef(0)

  const worktreeBranchSet = useMemo(
    () => new Set(branchList.worktree_branches),
    [branchList.worktree_branches]
  )
  const localNodes = useMemo(
    () => buildBranchTree(localBranchItems(branchList.local), "local"),
    [branchList.local]
  )
  const remoteSections = useMemo(
    () => buildRemoteBranchSections(branchList.remote),
    [branchList.remote]
  )
  const localSectionKey = sectionKey("local")
  const remoteSectionKey = sectionKey("remote")

  // Auto-expand the section + prefix groups leading to the current branch once
  // the branch list has loaded.
  const seedKeys = useMemo(() => {
    if (!branch) return []
    if (containsBranch(localNodes, branch)) {
      return [localSectionKey, ...expandedKeysForBranch(localNodes, branch)]
    }
    for (const section of remoteSections) {
      if (containsBranch(section.nodes, branch)) {
        const keys = [
          remoteSectionKey,
          ...expandedKeysForBranch(section.nodes, branch),
        ]
        if (section.key) keys.push(section.key)
        return keys
      }
    }
    return []
  }, [branch, localNodes, remoteSections, localSectionKey, remoteSectionKey])

  const { isExpanded, toggle } = useBranchTreeExpansion(dropdownOpen, seedKeys)

  const refresh = useCallback(() => {
    if (folderId) void refreshFolder(folderId)
  }, [folderId, refreshFolder])

  useEffect(() => {
    if (!folderId) return
    let unlisten: (() => void) | null = null
    subscribe<GitCommitSucceededEventPayload>(
      "folder://git-commit-succeeded",
      (payload) => {
        if (payload.folder_id !== folderId) return
        toast.success(t("toasts.commitCodeCompleted"), {
          description: t("toasts.committedFiles", {
            count: payload.committed_files,
          }),
        })
        refresh()
      }
    )
      .then((fn) => {
        unlisten = fn
      })
      .catch((err) => {
        console.error("[BranchDropdown] failed to listen commit event:", err)
      })
    return () => {
      unlisten?.()
    }
  }, [folderId, refresh, t])

  useEffect(() => {
    if (!folderId) return
    let unlisten: (() => void) | null = null
    subscribe<GitPushSucceededEventPayload>(
      "folder://git-push-succeeded",
      (payload) => {
        if (payload.folder_id !== folderId) return
        const { pushed_commits, upstream_set } = payload
        let description: string
        if (upstream_set) {
          description =
            pushed_commits === 0
              ? t("toasts.upstreamSet")
              : t("toasts.upstreamSetAndPushed", { count: pushed_commits })
        } else if (pushed_commits === 0) {
          description = t("toasts.noCommitsToPush")
        } else {
          description = t("toasts.pushedCommits", { count: pushed_commits })
        }
        toast.success(t("toasts.pushCodeCompleted"), { description })
        refresh()
      }
    )
      .then((fn) => {
        unlisten = fn
      })
      .catch((err) => {
        console.error("[BranchDropdown] failed to listen push event:", err)
      })
    return () => {
      unlisten?.()
    }
  }, [folderId, refresh, t])

  async function runGitTask<T>(
    label: string,
    action: () => Promise<T>,
    getSuccessDescription?: (result: T) => string | false | undefined,
    onError?: (errorMsg: string) => boolean
  ) {
    const taskId = `git-${++taskSeq.current}-${Date.now()}`
    setLoading(true)
    addTask(taskId, label)
    updateTask(taskId, { status: "running" })
    try {
      const result = await action()
      const successDescription = getSuccessDescription?.(result)
      updateTask(taskId, { status: "completed" })
      refresh()
      void emitEvent("folder://git-branch-changed", { folder_id: folderId })
      if (successDescription !== false) {
        toast.success(
          t("toasts.taskCompleted", { label }),
          successDescription ? { description: successDescription } : undefined
        )
      }
    } catch (err) {
      removeTask(taskId)
      const errorMsg = toErrorMessage(err)
      if (onError?.(errorMsg)) {
        return
      }
      const errorTitle = t("toasts.taskFailed", { label })
      pushAlert("error", errorTitle, errorMsg)
      toast.error(errorTitle, { description: errorMsg })
    } finally {
      setLoading(false)
    }
  }

  const loadAllBranches = useCallback(async () => {
    if (!folderPath) return
    setBranchLoading(true)
    try {
      const list = await gitListAllBranches(folderPath)
      setBranchList(list)
    } catch {
      setBranchList({ local: [], remote: [], worktree_branches: [] })
    } finally {
      setBranchLoading(false)
    }
  }, [folderPath])

  function handleDropdownOpenChange(open: boolean) {
    setDropdownOpen(open)
    if (open && isRepo) {
      void loadAllBranches()
    }
  }

  async function handleCheckout(branchName: string) {
    if (!activeFolder) return
    setDropdownOpen(false)
    await switchToBranch({ activeFolder, branchName, currentBranch: branch })
  }

  async function handleCheckoutRemote(remoteBranch: string) {
    if (!activeFolder) return
    const localName = remoteBranch.replace(/^[^/]+\//, "")
    setDropdownOpen(false)
    await switchToBranch({
      activeFolder,
      branchName: localName,
      currentBranch: branch,
      isRemote: true,
    })
  }

  async function handleNewBranch() {
    const name = newBranchName.trim()
    if (!name) return
    setNewBranchOpen(false)
    setNewBranchName("")
    await runGitTask(t("tasks.newBranch", { name }), () =>
      gitNewBranch(folderPath, name)
    )
  }

  function handleOpenWorktreeDialog() {
    const chars = "abcdefghijklmnopqrstuvwxyz0123456789"
    let random = ""
    for (let i = 0; i < 6; i++) {
      random += chars[Math.floor(Math.random() * chars.length)]
    }
    const folderName = folderPath.split("/").filter(Boolean).pop() ?? "project"
    const currentBranch = branch ?? "main"
    const defaultBranch = `cv-${currentBranch}-${random}`
    const parentDir = folderPath.substring(0, folderPath.lastIndexOf("/"))
    setWorktreeBranchName(defaultBranch)
    setWorktreePath(`${parentDir}/${folderName}-${currentBranch}-${random}`)
    setWorktreeOpen(true)
  }

  async function handleBrowseWorktreePath() {
    // The worktree is created on whatever host runs the git binary — local
    // for the desktop, remote for a remote workspace. The picker must
    // therefore browse the matching filesystem, otherwise the user
    // ends up with a path the wrong side can't resolve.
    if (isDesktop() && getActiveRemoteConnectionId() === null) {
      const selected = await openFileDialog({
        directory: true,
        multiple: false,
      })
      if (selected) {
        setWorktreePath(Array.isArray(selected) ? selected[0] : selected)
      }
    } else {
      setWorktreeBrowserOpen(true)
    }
  }

  async function handleNewWorktree() {
    const name = worktreeBranchName.trim()
    const wtPath = worktreePath.trim()
    if (!name || !wtPath) return
    setWorktreeOpen(false)
    await runGitTask(t("tasks.newWorktree", { name }), async () => {
      await gitWorktreeAdd(folderPath, name, wtPath)
      // Register the worktree as a folder parented to this repo (flattened to
      // the root), then open a draft conversation in it. Once child folders are
      // merged under their parent in the sidebar, a worktree with no
      // conversations would otherwise be unreachable; this also lands the new
      // session with its cwd set to the worktree directory (detail.path).
      const detail = await openWorktreeFolder(wtPath, folderId)
      // Return to the conversation workspace if a route (e.g. Automations)
      // was covering the content region, else the new tab opens unseen.
      openConversations()
      openNewConversationTab(detail.id, detail.path)
    })
  }

  async function handleConfirm() {
    if (!confirmAction) return
    const { type, branchName } = confirmAction
    setConfirmAction(null)

    switch (type) {
      case "merge":
        await runGitTask(
          t("tasks.mergeBranch", { branchName }),
          () => gitMerge(folderPath, branchName),
          (result) => {
            if (result.conflict?.has_conflicts) {
              setConflictInfo(result.conflict)
              return false
            }
            if (result.merged_commits === 0) {
              return t("toasts.mergeNoNewCommits", { branchName })
            }
            return t("toasts.mergedCommits", { count: result.merged_commits })
          }
        )
        break
      case "rebase":
        await runGitTask(
          t("tasks.rebaseTo", { branchName }),
          () => gitRebase(folderPath, branchName),
          (result) => {
            if (result.conflict?.has_conflicts) {
              setConflictInfo(result.conflict)
              return false
            }
            return undefined
          }
        )
        break
      case "delete":
        await runGitTask(
          t("tasks.deleteBranch", { branchName }),
          () => gitDeleteBranch(folderPath, branchName),
          undefined,
          (errorMsg) => {
            if (/not fully merged/i.test(errorMsg)) {
              setConfirmAction({ type: "forceDelete", branchName })
              return true
            }
            return false
          }
        )
        break
      case "forceDelete":
        await runGitTask(t("tasks.deleteBranch", { branchName }), () =>
          gitDeleteBranch(folderPath, branchName, true)
        )
        break
      case "deleteRemote": {
        const idx = branchName.indexOf("/")
        const remote = branchName.substring(0, idx)
        const rb = branchName.substring(idx + 1)
        await runGitTask(t("tasks.deleteRemoteBranch", { branchName }), () =>
          withCredentialRetry(
            (creds) => gitDeleteRemoteBranch(folderPath, remote, rb, creds),
            { folderPath }
          )
        )
        break
      }
    }
  }

  function getConfirmTitle() {
    if (!confirmAction) return ""
    switch (confirmAction.type) {
      case "merge":
        return t("confirm.mergeTitle")
      case "rebase":
        return t("confirm.rebaseTitle")
      case "delete":
        return t("confirm.deleteTitle")
      case "forceDelete":
        return t("confirm.forceDeleteTitle")
      case "deleteRemote":
        return t("confirm.deleteRemoteTitle")
    }
  }

  function getConfirmDescription() {
    if (!confirmAction) return ""
    switch (confirmAction.type) {
      case "merge":
        return t("confirm.mergeDescription", {
          branchName: confirmAction.branchName,
          currentBranch: branch ?? "-",
        })
      case "rebase":
        return t("confirm.rebaseDescription", {
          currentBranch: branch ?? "-",
          branchName: confirmAction.branchName,
        })
      case "delete":
        return t("confirm.deleteDescription", {
          branchName: confirmAction.branchName,
        })
      case "forceDelete":
        return t("confirm.forceDeleteDescription", {
          branchName: confirmAction.branchName,
        })
      case "deleteRemote":
        return t("confirm.deleteRemoteDescription", {
          branchName: confirmAction.branchName,
        })
    }
  }

  function renderBranchItem(
    leaf: BranchTreeLeaf,
    isRemote: boolean,
    depth: number
  ) {
    const b = leaf.fullName
    const label = leaf.label
    const paddingLeft = branchRowPaddingLeft("dropdown", depth)
    const isCurrent = b === branch
    const isTrackingCurrent =
      isRemote && !!branch && b.replace(/^[^/]+\//, "") === branch
    const isWorktree = worktreeBranchSet.has(
      isRemote ? b.replace(/^[^/]+\//, "") : b
    )
    const BranchIcon = isWorktree ? FolderGit2 : GitBranch

    if (isCurrent) {
      return (
        <div
          key={leaf.key}
          className="flex select-none items-center gap-2.5 rounded-xl py-2 pr-3 text-sm opacity-50"
          style={{ paddingLeft }}
          title={b}
        >
          <BranchIcon className="h-3.5 w-3.5 shrink-0" />
          <span className="min-w-0 flex-1 truncate">{label}</span>
          <span className="shrink-0 pl-2 text-xs">{t("current")}</span>
        </div>
      )
    }

    return (
      <DropdownMenuSub key={leaf.key}>
        <DropdownMenuSubTrigger
          className="hover:bg-accent hover:text-accent-foreground"
          style={{ paddingLeft }}
          disabled={loading}
          title={b}
        >
          <BranchIcon className="h-3.5 w-3.5 shrink-0" />
          <span className="min-w-0 flex-1 truncate">{label}</span>
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem
            onSelect={() => {
              if (isRemote) {
                void handleCheckoutRemote(b)
              } else {
                void handleCheckout(b)
              }
            }}
          >
            <GitBranch className="h-3.5 w-3.5" />
            {t("switchToBranch")}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={() => {
              setDropdownOpen(false)
              setConfirmAction({ type: "merge", branchName: b })
            }}
          >
            <GitMerge className="h-3.5 w-3.5" />
            {t("mergeBranchIntoCurrent", {
              branchName: b,
              currentBranch: branch ?? "-",
            })}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={() => {
              setDropdownOpen(false)
              setConfirmAction({ type: "rebase", branchName: b })
            }}
          >
            <GitPullRequestArrow className="h-3.5 w-3.5" />
            {t("rebaseCurrentToBranch", {
              currentBranch: branch ?? "-",
              branchName: b,
            })}
          </DropdownMenuItem>
          {!isTrackingCurrent && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                variant="destructive"
                onSelect={() => {
                  setDropdownOpen(false)
                  setConfirmAction({
                    type: isRemote ? "deleteRemote" : "delete",
                    branchName: b,
                  })
                }}
              >
                <Trash2 className="h-3.5 w-3.5" />
                {t("deleteBranch")}
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
    )
  }

  // Render the prefix tree as a flat sequence of menu items. Group rows are
  // DropdownMenuItems (not plain Collapsible buttons) so Radix's roving focus /
  // typeahead can reach them by keyboard; `onSelect` preventDefault keeps the
  // menu open while toggling. Collapsed children simply aren't rendered.
  function renderDropdownTree(
    nodes: BranchTreeNode[],
    depth: number,
    isRemote: boolean
  ): React.ReactNode[] {
    return nodes.flatMap((node) => {
      if (node.type === "leaf") {
        return [renderBranchItem(node, isRemote, depth)]
      }
      const groupOpen = isExpanded(node.key)
      return [
        <DropdownMenuItem
          key={node.key}
          aria-expanded={groupOpen}
          onSelect={(e) => {
            e.preventDefault()
            toggle(node.key)
          }}
          style={{ paddingLeft: branchRowPaddingLeft("dropdown", depth) }}
        >
          <ChevronRight
            className={cn(
              "h-3.5 w-3.5 shrink-0 transition-transform",
              groupOpen && "rotate-90"
            )}
          />
          <span className="min-w-0 flex-1 truncate">{node.label}</span>
          <span className="shrink-0 pl-2 text-xs text-muted-foreground/70">
            {node.count}
          </span>
        </DropdownMenuItem>,
        ...(groupOpen
          ? renderDropdownTree(node.children, depth + 1, isRemote)
          : []),
      ]
    })
  }

  // Folderless chat conversations have no git branch — hide the top-bar
  // selector entirely (covers both the mobile and desktop title-bar instances).
  if (!activeFolder || isChatMode) return null

  // Worktree folders display their parent (root repo) name; paths/ids/git ops
  // below still use `activeFolder` (the worktree) unchanged.
  const folderName = resolveFolderDisplayName(activeFolder, allFolders)

  const triggerLabel = (branchLabel: string) =>
    compact ? (
      <span className="min-w-0 flex-1 text-left">
        <span className="block truncate text-[13px] font-semibold leading-4">
          {folderName}
        </span>
        <span className="block truncate text-[11px] font-normal leading-4 text-muted-foreground">
          {branchLabel}
        </span>
      </span>
    ) : (
      <span className="max-w-[320px] truncate">
        {folderName}
        <span className="mx-1.5 inline-block h-3 w-px bg-foreground/20 align-middle" />
        <span className="text-primary">{branchLabel}</span>
      </span>
    )

  if (!isRepo) {
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            className={cn(
              "flex min-w-0 items-center gap-1 text-sm tracking-tight outline-none transition-colors cursor-default hover:text-foreground/80",
              compact &&
                "h-11 w-full max-w-full gap-2 overflow-hidden rounded-xl px-1 cursor-pointer"
            )}
          >
            <GitFork className={cn("h-3 w-3 shrink-0", compact && "h-4 w-4")} />
            {triggerLabel(t("noBranch"))}
            <ChevronDown
              className={cn(
                "h-3 w-3 shrink-0 opacity-50",
                compact && "h-4 w-4"
              )}
            />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="min-w-64" align="start">
          <DropdownMenuItem
            disabled={loading}
            onSelect={() =>
              runGitTask(t("tasks.initGitRepo"), () => gitInit(folderPath))
            }
          >
            <GitBranch className="h-3.5 w-3.5" />
            {t("initGitRepo")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    )
  }

  return (
    <>
      <DropdownMenu open={dropdownOpen} onOpenChange={handleDropdownOpenChange}>
        <DropdownMenuTrigger asChild>
          <button
            className={cn(
              "flex min-w-0 items-center gap-1 text-sm tracking-tight outline-none transition-colors cursor-default hover:text-foreground/80",
              compact &&
                "h-11 w-full max-w-full gap-2 overflow-hidden rounded-xl px-1 cursor-pointer"
            )}
            title={
              isDetached
                ? t("detachedHead", { sha: head?.short_sha ?? "" })
                : undefined
            }
          >
            {isDetached ? (
              <GitCommitHorizontal
                className={cn("h-3 w-3 shrink-0", compact && "h-4 w-4")}
              />
            ) : (
              <GitBranch
                className={cn("h-3 w-3 shrink-0", compact && "h-4 w-4")}
              />
            )}
            {triggerLabel(branch ?? head?.short_sha ?? t("noBranch"))}
            <ChevronDown
              className={cn(
                "h-3 w-3 shrink-0 opacity-50",
                compact && "h-4 w-4"
              )}
            />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="min-w-64" align="start">
          <DropdownMenuGroup>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() =>
                runGitTask(
                  t("tasks.pullCode"),
                  () =>
                    withCredentialRetry((creds) => gitPull(folderPath, creds), {
                      folderPath,
                    }),
                  (result) => {
                    if (result.conflict?.has_conflicts) {
                      setConflictInfo(result.conflict)
                      return false
                    }
                    if (result.updated_files === 0) {
                      return t("toasts.allFilesUpToDate")
                    }
                    return t("toasts.updatedFiles", {
                      count: result.updated_files,
                    })
                  }
                )
              }
            >
              <ArrowDownToLine className="h-3.5 w-3.5" />
              {t("pullCode")}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() =>
                runGitTask(t("tasks.fetchInfo"), () =>
                  withCredentialRetry((creds) => gitFetch(folderPath, creds), {
                    folderPath,
                  })
                )
              }
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("fetchRemoteBranches")}
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                if (!folderId) return
                setDropdownOpen(false)
                openCommitWindow(folderId).catch((err) => {
                  const title = t("toasts.openCommitWindowFailed")
                  const msg = toErrorMessage(err)
                  pushAlert("error", title, msg)
                  toast.error(title, { description: msg })
                })
              }}
            >
              <GitCommitHorizontal className="h-3.5 w-3.5" />
              {t("openCommitWindow")}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                if (!folderId) return
                setDropdownOpen(false)
                openPushWindow(folderId).catch((err) => {
                  const title = t("toasts.openPushWindowFailed")
                  const msg = toErrorMessage(err)
                  pushAlert("error", title, msg)
                  toast.error(title, { description: msg })
                })
              }}
            >
              <Upload className="h-3.5 w-3.5" />
              {t("pushCode")}
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                setNewBranchName("")
                setNewBranchOpen(true)
              }}
            >
              <GitBranchPlus className="h-3.5 w-3.5" />
              {t("newBranch")}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={loading}
              onSelect={handleOpenWorktreeDialog}
            >
              <FolderGit2 className="h-3.5 w-3.5" />
              {t("newWorktree")}
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                setDropdownOpen(false)
                setStashDialogOpen(true)
              }}
            >
              <Archive className="h-3.5 w-3.5" />
              {t("stashChanges")}
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                if (!folderId) return
                openStashWindow(folderId).catch((err) => {
                  const msg = toErrorMessage(err)
                  pushAlert("error", t("stashPop"), msg)
                })
              }}
            >
              <ArchiveRestore className="h-3.5 w-3.5" />
              {t("stashPop")}
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuGroup>
            <DropdownMenuItem
              disabled={loading}
              onSelect={() => {
                setDropdownOpen(false)
                setManageRemotesOpen(true)
              }}
            >
              <Globe className="h-3.5 w-3.5" />
              {t("manageRemotes")}
            </DropdownMenuItem>
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          {branchLoading ? (
            <div className="flex items-center justify-center py-3">
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
          ) : (
            <ScrollArea className="max-h-64">
              <DropdownMenuItem
                aria-expanded={isExpanded(localSectionKey)}
                onSelect={(e) => {
                  e.preventDefault()
                  toggle(localSectionKey)
                }}
              >
                <ChevronRight
                  className={cn(
                    "h-3.5 w-3.5 shrink-0 transition-transform",
                    isExpanded(localSectionKey) && "rotate-90"
                  )}
                />
                {t("localBranches", { count: branchList.local.length })}
              </DropdownMenuItem>
              {isExpanded(localSectionKey) &&
                (branchList.local.length === 0 ? (
                  <DropdownMenuItem disabled>
                    {t("noLocalBranches")}
                  </DropdownMenuItem>
                ) : (
                  renderDropdownTree(localNodes, 0, false)
                ))}

              <DropdownMenuItem
                aria-expanded={isExpanded(remoteSectionKey)}
                onSelect={(e) => {
                  e.preventDefault()
                  toggle(remoteSectionKey)
                }}
              >
                <ChevronRight
                  className={cn(
                    "h-3.5 w-3.5 shrink-0 transition-transform",
                    isExpanded(remoteSectionKey) && "rotate-90"
                  )}
                />
                {t("remoteBranches", { count: branchList.remote.length })}
              </DropdownMenuItem>
              {isExpanded(remoteSectionKey) &&
                (branchList.remote.length === 0 ? (
                  <DropdownMenuItem disabled>
                    {t("noRemoteBranches")}
                  </DropdownMenuItem>
                ) : (
                  remoteSections.map((section) =>
                    section.remoteName == null ? (
                      <Fragment key="remote-single">
                        {renderDropdownTree(section.nodes, 0, true)}
                      </Fragment>
                    ) : (
                      <Fragment key={section.key}>
                        <DropdownMenuItem
                          aria-expanded={isExpanded(section.key)}
                          onSelect={(e) => {
                            e.preventDefault()
                            toggle(section.key)
                          }}
                          style={{
                            paddingLeft: branchRowPaddingLeft("dropdown", 0),
                          }}
                        >
                          <ChevronRight
                            className={cn(
                              "h-3 w-3 shrink-0 transition-transform",
                              isExpanded(section.key) && "rotate-90"
                            )}
                          />
                          <span className="min-w-0 flex-1 truncate">
                            {section.remoteName}
                          </span>
                          <span className="shrink-0 pl-2 text-xs text-muted-foreground/70">
                            {section.count}
                          </span>
                        </DropdownMenuItem>
                        {isExpanded(section.key) &&
                          renderDropdownTree(section.nodes, 1, true)}
                      </Fragment>
                    )
                  )
                ))}
            </ScrollArea>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      <AlertDialog
        open={confirmAction !== null}
        onOpenChange={(open) => {
          if (!open) setConfirmAction(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{getConfirmTitle()}</AlertDialogTitle>
            <AlertDialogDescription>
              {getConfirmDescription()}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tCommon("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant={
                confirmAction?.type === "delete" ||
                confirmAction?.type === "forceDelete" ||
                confirmAction?.type === "deleteRemote"
                  ? "destructive"
                  : "default"
              }
              onClick={handleConfirm}
            >
              {tCommon("confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog open={newBranchOpen} onOpenChange={setNewBranchOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("dialogs.newBranchTitle")}</DialogTitle>
            <DialogDescription>
              {t("dialogs.newBranchDescription", { branch: branch ?? "-" })}
            </DialogDescription>
          </DialogHeader>
          <Input
            placeholder={t("dialogs.branchNamePlaceholder")}
            value={newBranchName}
            onChange={(e) => setNewBranchName(e.target.value)}
            onKeyDown={(e) => {
              if (e.nativeEvent.isComposing || e.key === "Process") return
              if (e.key === "Enter") handleNewBranch()
            }}
            autoFocus
          />
          <DialogFooter>
            <Button variant="outline" onClick={() => setNewBranchOpen(false)}>
              {tCommon("cancel")}
            </Button>
            <Button
              disabled={!newBranchName.trim() || loading}
              onClick={handleNewBranch}
            >
              {tCommon("create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={worktreeOpen} onOpenChange={setWorktreeOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("dialogs.newWorktreeTitle")}</DialogTitle>
            <DialogDescription>
              {t("dialogs.newWorktreeDescription", { branch: branch ?? "-" })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-2">
            <div className="space-y-2">
              <Label htmlFor="wt-branch">{t("dialogs.branchNameLabel")}</Label>
              <Input
                id="wt-branch"
                placeholder={t("dialogs.branchNamePlaceholder")}
                value={worktreeBranchName}
                onChange={(e) => setWorktreeBranchName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.nativeEvent.isComposing || e.key === "Process") return
                  if (e.key === "Enter") handleNewWorktree()
                }}
                autoFocus
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="wt-path">{t("dialogs.worktreePathLabel")}</Label>
              <div className="flex gap-2">
                <Input
                  id="wt-path"
                  placeholder={t("dialogs.worktreePathPlaceholder")}
                  value={worktreePath}
                  onChange={(e) => setWorktreePath(e.target.value)}
                  className="flex-1"
                />
                <Button
                  variant="outline"
                  size="icon"
                  onClick={handleBrowseWorktreePath}
                >
                  <FolderOpen className="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setWorktreeOpen(false)}>
              {tCommon("cancel")}
            </Button>
            <Button
              disabled={
                !worktreeBranchName.trim() || !worktreePath.trim() || loading
              }
              onClick={handleNewWorktree}
            >
              {tCommon("create")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <DirectoryBrowserDialog
        open={worktreeBrowserOpen}
        onOpenChange={setWorktreeBrowserOpen}
        onSelect={(path) => setWorktreePath(path)}
      />

      <RemoteManageDialog
        open={manageRemotesOpen}
        onOpenChange={setManageRemotesOpen}
        folderPath={folderPath}
        onSaved={() => loadAllBranches()}
      />

      <ConflictDialog
        conflictInfo={conflictInfo}
        folderId={folderId}
        folderPath={folderPath}
        onClose={() => setConflictInfo(null)}
        onResolved={refresh}
      />

      <StashDialog
        open={stashDialogOpen}
        folderPath={folderPath}
        onClose={() => setStashDialogOpen(false)}
        onStashed={refresh}
      />
    </>
  )
}
