"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  ChevronDown,
  FolderOpen,
  GitBranch,
  GitCommitHorizontal,
  GitFork,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
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
import { useSwitchToBranch } from "@/hooks/use-switch-to-branch"
import {
  buildBranchTree,
  buildRemoteBranchSections,
  localBranchItems,
} from "@/lib/branch-tree"
import { BranchSelectorList } from "@/components/layout/branch-selector-list"
import type {
  BranchLeafAction,
  BranchOperationMeta,
} from "@/lib/branch-selector-rows"
import { useScrollbarSafeDismiss } from "@/hooks/use-scrollbar-safe-dismiss"
import type { FolderDetail, GitBranchList, GitConflictInfo } from "@/lib/types"
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

interface BranchDropdownProps {
  /** The row's OWN folder (each conversation tile passes its own), not the
   *  active one — so a tiled view keeps every tile's branch chip live. */
  folder: FolderDetail | null
  /** Whether this tile is folderless "chat mode" (self-hides the chip). */
  isChatMode: boolean
}

// The branch chip in the below-composer folder/branch row. It's mounted once per
// conversation tile with that tile's own `folder`, and carries per-instance
// machinery (git event subscriptions + dialogs).
export function BranchDropdown({ folder, isChatMode }: BranchDropdownProps) {
  const t = useTranslations("Folder.branchDropdown")
  const tCommon = useTranslations("Folder.common")
  const activeFolder = folder
  const refreshFolder = useAppWorkspaceStore((s) => s.refreshFolder)
  const openWorktreeFolder = useAppWorkspaceStore((s) => s.openWorktreeFolder)
  const { openNewConversationTab } = useTabActions()
  const { openConversations } = useWorkbenchRoute()
  const { addTask, updateTask, removeTask } = useTaskContext()
  const { pushAlert } = useAlertContext()
  const { withCredentialRetry } = useGitCredential()
  const switchToBranch = useSwitchToBranch()
  // Grabbing the popover's inner scrollbar blurs focus, which WebKit bounces to
  // an outside element that Radix reads as a dismiss — keep it open (see hook).
  const { contentRef, onPointerDownOutside, onFocusOutside } =
    useScrollbarSafeDismiss()

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
  // Operations shown as a searchable block at the top of the popup; the list
  // resolves each id to an icon and dispatches back through `runOperation`.
  // `groupEnd` inserts a separator after that op (non-search) to restore the old
  // menu's pull/fetch | commit/push | new | stash | remotes blocking.
  const operations = useMemo<BranchOperationMeta[]>(
    () => [
      { id: "pull", label: t("pullCode") },
      { id: "fetch", label: t("fetchRemoteBranches"), groupEnd: true },
      { id: "commit", label: t("openCommitWindow") },
      { id: "push", label: t("pushCode"), groupEnd: true },
      { id: "newBranch", label: t("newBranch") },
      { id: "newWorktree", label: t("newWorktree"), groupEnd: true },
      { id: "stash", label: t("stashChanges") },
      { id: "stashPop", label: t("stashPop"), groupEnd: true },
      { id: "manageRemotes", label: t("manageRemotes") },
    ],
    [t]
  )

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

  // Pull, invoked by the dropdown's "Pull Code" menu item.
  function handlePull() {
    setDropdownOpen(false)
    void runGitTask(
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
        return t("toasts.updatedFiles", { count: result.updated_files })
      }
    )
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

  // Dispatch a top-of-list operation back to its handler. Every op closes the
  // popover (some then open a dialog/window); `handlePull` closes it too.
  function runOperation(opId: string) {
    setDropdownOpen(false)
    switch (opId) {
      case "pull":
        handlePull()
        break
      case "fetch":
        void runGitTask(t("tasks.fetchInfo"), () =>
          withCredentialRetry((creds) => gitFetch(folderPath, creds), {
            folderPath,
          })
        )
        break
      case "commit":
        if (!folderId) return
        openCommitWindow(folderId).catch((err) => {
          const title = t("toasts.openCommitWindowFailed")
          const msg = toErrorMessage(err)
          pushAlert("error", title, msg)
          toast.error(title, { description: msg })
        })
        break
      case "push":
        if (!folderId) return
        openPushWindow(folderId).catch((err) => {
          const title = t("toasts.openPushWindowFailed")
          const msg = toErrorMessage(err)
          pushAlert("error", title, msg)
          toast.error(title, { description: msg })
        })
        break
      case "newBranch":
        setNewBranchName("")
        setNewBranchOpen(true)
        break
      case "newWorktree":
        handleOpenWorktreeDialog()
        break
      case "stash":
        setStashDialogOpen(true)
        break
      case "stashPop":
        if (!folderId) return
        openStashWindow(folderId).catch((err) => {
          const msg = toErrorMessage(err)
          pushAlert("error", t("stashPop"), msg)
        })
        break
      case "manageRemotes":
        setManageRemotesOpen(true)
        break
    }
  }

  // Dispatch an inline branch action: switch checks out directly (that handler
  // closes the popover itself), the rest open the shared confirm dialog.
  function runLeafAction(
    action: BranchLeafAction,
    fullName: string,
    isRemote: boolean
  ) {
    if (action === "switch") {
      if (isRemote) void handleCheckoutRemote(fullName)
      else void handleCheckout(fullName)
      return
    }
    setDropdownOpen(false)
    setConfirmAction({ type: action, branchName: fullName })
  }

  // Folderless chat conversations have no git branch — hide the branch chip
  // entirely (the below-composer row still shows the folder chip beside it).
  if (!activeFolder || isChatMode) return null

  if (!isRepo) {
    // Non-git folder: no branch and nothing to pull, so a single chip (no split
    // pull half) opening a one-item popover that offers to init a repo.
    return (
      <Popover open={dropdownOpen} onOpenChange={handleDropdownOpenChange}>
        <PopoverTrigger asChild>
          <button
            type="button"
            title={t("noBranch")}
            className="flex h-6 min-w-0 items-center gap-1.5 rounded-full px-2 text-xs text-muted-foreground outline-none transition-colors hover:bg-foreground/10 hover:text-foreground"
          >
            <GitFork className="size-3 shrink-0" />
            <span className="max-w-[160px] truncate">{t("noBranch")}</span>
          </button>
        </PopoverTrigger>
        <PopoverContent side="top" align="start" className="w-64 p-1">
          <button
            type="button"
            disabled={loading}
            onClick={() => {
              setDropdownOpen(false)
              void runGitTask(t("tasks.initGitRepo"), () => gitInit(folderPath))
            }}
            className="flex w-full select-none items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors hover:bg-accent hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50"
          >
            <GitBranch className="size-3.5 shrink-0" />
            {t("initGitRepo")}
          </button>
        </PopoverContent>
      </Popover>
    )
  }

  return (
    <>
      {/* Single chip: the branch name + chevron opens the searchable popup (pull
          lives inside it now). Matches the sibling folder chip's ghost xs feel. */}
      <Popover open={dropdownOpen} onOpenChange={handleDropdownOpenChange}>
        <PopoverTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            title={
              isDetached
                ? t("detachedHead", { sha: head?.short_sha ?? "" })
                : (branch ?? undefined)
            }
            className="min-w-0 gap-0.5 px-1.5"
          >
            {isDetached ? (
              <GitCommitHorizontal className="size-3 shrink-0 text-muted-foreground" />
            ) : (
              <GitBranch className="size-3 shrink-0 text-muted-foreground" />
            )}
            <span className="max-w-[160px] truncate">
              {branch ?? head?.branch ?? head?.short_sha ?? t("noBranch")}
            </span>
            <ChevronDown className="size-3 shrink-0 text-muted-foreground/60" />
          </Button>
        </PopoverTrigger>
        {/* No `overflow-hidden`: the list's inner shell clips to the rounding so
            the right-side action bubble can overflow past this edge. */}
        <PopoverContent
          ref={contentRef}
          side="top"
          align="start"
          onPointerDownOutside={onPointerDownOutside}
          onFocusOutside={onFocusOutside}
          className="w-[22rem] max-w-[calc(100vw-1rem)] p-0"
        >
          <BranchSelectorList
            operations={operations}
            localNodes={localNodes}
            remoteSections={remoteSections}
            localCount={branchList.local.length}
            remoteCount={branchList.remote.length}
            branch={branch}
            worktreeBranchSet={worktreeBranchSet}
            branchLoading={branchLoading}
            loading={loading}
            onRunOperation={runOperation}
            onLeafAction={runLeafAction}
          />
        </PopoverContent>
      </Popover>

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
