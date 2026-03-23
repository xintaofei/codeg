"use client"

import { useState, useRef, useCallback, useMemo, useEffect } from "react"
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event"
import {
  GitBranch,
  ChevronDown,
  ChevronRight,
  ArrowDownToLine,
  Upload,
  GitBranchPlus,
  GitCommitHorizontal,
  Archive,
  ArchiveRestore,
  GitFork,
  GitMerge,
  GitPullRequestArrow,
  Trash2,
  Loader2,
  RefreshCw,
  FolderGit2,
  FolderOpen,
  ArrowLeftRight,
  Globe,
} from "lucide-react"
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
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { open } from "@tauri-apps/plugin-dialog"
import {
  gitInit,
  gitPull,
  gitFetch,
  gitNewBranch,
  gitWorktreeAdd,
  gitCheckout,
  gitListAllBranches,
  gitMerge,
  gitRebase,
  gitDeleteBranch,
  openFolderWindow,
  openCommitWindow,
  setFolderParentBranch,
  openStashWindow,
  openPushWindow,
} from "@/lib/tauri"
import { RemoteManageDialog } from "@/components/layout/remote-manage-dialog"
import { ConflictDialog } from "@/components/layout/conflict-dialog"
import { StashDialog } from "@/components/layout/stash-dialog"
import { disposeTauriListener } from "@/lib/tauri-listener"
import { toErrorMessage } from "@/lib/app-error"
import type { GitBranchList, GitConflictInfo } from "@/lib/types"
import { toast } from "sonner"
import { useFolderContext } from "@/contexts/folder-context"
import { useTaskContext } from "@/contexts/task-context"
import { useAlertContext } from "@/contexts/alert-context"
import { useGitCredential } from "@/contexts/git-credential-context"

interface BranchDropdownProps {
  branch: string | null
  parentBranch: string | null
  onBranchChange: () => void
}

type ConfirmAction = {
  type: "merge" | "rebase" | "delete" | "forceDelete"
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

export function BranchDropdown({
  branch,
  parentBranch,
  onBranchChange,
}: BranchDropdownProps) {
  const t = useTranslations("Folder.branchDropdown")
  const tCommon = useTranslations("Folder.common")
  const { folder } = useFolderContext()
  const folderPath = folder?.path ?? ""
  const { addTask, updateTask, removeTask } = useTaskContext()
  const { pushAlert } = useAlertContext()
  const { withCredentialRetry } = useGitCredential()
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
  const [localOpen, setLocalOpen] = useState(false)
  const [remoteOpen, setRemoteOpen] = useState(false)
  const [confirmAction, setConfirmAction] = useState<ConfirmAction | null>(null)
  const [expandedBranch, setExpandedBranch] = useState<string | null>(null)
  const [worktreeOpen, setWorktreeOpen] = useState(false)
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

  useEffect(() => {
    if (!folder) return

    let unlisten: UnlistenFn | null = null

    listen<GitCommitSucceededEventPayload>(
      "folder://git-commit-succeeded",
      (event) => {
        if (event.payload.folder_id !== folder.id) return
        toast.success(t("toasts.commitCodeCompleted"), {
          description: t("toasts.committedFiles", {
            count: event.payload.committed_files,
          }),
        })
        onBranchChange()
      }
    )
      .then((fn) => {
        unlisten = fn
      })
      .catch((err) => {
        console.error("[BranchDropdown] failed to listen commit event:", err)
      })

    return () => {
      disposeTauriListener(unlisten, "BranchDropdown.gitCommitSucceeded")
    }
  }, [folder, onBranchChange, t])

  useEffect(() => {
    if (!folder) return

    let unlisten: UnlistenFn | null = null

    listen<GitPushSucceededEventPayload>(
      "folder://git-push-succeeded",
      (event) => {
        if (event.payload.folder_id !== folder.id) return
        const { pushed_commits, upstream_set } = event.payload
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
        onBranchChange()
      }
    )
      .then((fn) => {
        unlisten = fn
      })
      .catch((err) => {
        console.error("[BranchDropdown] failed to listen push event:", err)
      })

    return () => {
      disposeTauriListener(unlisten, "BranchDropdown.gitPushSucceeded")
    }
  }, [folder, onBranchChange, t])

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
      onBranchChange()
      void emit("folder://git-branch-changed", {
        folder_id: folder?.id,
      })
      if (successDescription !== false) {
        toast.success(
          t("toasts.taskCompleted", { label }),
          successDescription
            ? {
                description: successDescription,
              }
            : undefined
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
    if (open && branch !== null) {
      loadAllBranches()
    }
    if (!open) {
      setLocalOpen(false)
      setRemoteOpen(false)
      setExpandedBranch(null)
    }
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

  function handleWorktreeBranchChange(name: string) {
    setWorktreeBranchName(name)
  }

  async function handleBrowseWorktreePath() {
    const selected = await open({ directory: true, multiple: false })
    if (selected) {
      setWorktreePath(selected)
    }
  }

  async function handleNewWorktree() {
    const name = worktreeBranchName.trim()
    const wtPath = worktreePath.trim()
    if (!name || !wtPath) return
    setWorktreeOpen(false)
    await runGitTask(t("tasks.newWorktree", { name }), async () => {
      await gitWorktreeAdd(folderPath, name, wtPath)
      await openFolderWindow(wtPath)
      await setFolderParentBranch(wtPath, branch)
    })
  }

  function handleMergeParent() {
    if (!parentBranch) return
    setConfirmAction({ type: "merge", branchName: parentBranch })
  }

  async function handleCheckout(branchName: string) {
    setDropdownOpen(false)
    await runGitTask(t("tasks.checkoutTo", { branchName }), () =>
      gitCheckout(folderPath, branchName)
    )
  }

  async function handleCheckoutRemote(remoteBranch: string) {
    const localName = remoteBranch.replace(/^[^/]+\//, "")
    setDropdownOpen(false)
    await runGitTask(t("tasks.checkoutTo", { branchName: localName }), () =>
      gitCheckout(folderPath, localName)
    )
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
    }
  }

  function renderBranchItem(
    b: string,
    isRemote: boolean,
    displayName?: string
  ) {
    const label = displayName ?? b
    const isCurrent = b === branch
    const isWorktree = worktreeBranchSet.has(
      isRemote ? b.replace(/^[^/]+\//, "") : b
    )
    const BranchIcon = isWorktree ? FolderGit2 : GitBranch

    if (isCurrent) {
      return (
        <div
          key={b}
          className="flex items-center gap-2.5 rounded-xl px-3 py-2 text-sm opacity-50 select-none"
        >
          <BranchIcon className="h-3.5 w-3.5 shrink-0" />
          <span className="truncate">{label}</span>
          <span className="ml-auto text-xs">{t("current")}</span>
        </div>
      )
    }

    return (
      <DropdownMenuSub
        key={b}
        open={expandedBranch === b}
        onOpenChange={(open) => {
          if (!open) setExpandedBranch(null)
        }}
      >
        <DropdownMenuSubTrigger
          className="hover:bg-accent hover:text-accent-foreground"
          disabled={loading}
          onClick={() => setExpandedBranch(expandedBranch === b ? null : b)}
          onPointerMove={(e) => {
            e.preventDefault()
            if (expandedBranch !== null && expandedBranch !== b) {
              setExpandedBranch(null)
              if (document.activeElement instanceof HTMLElement) {
                document.activeElement.blur()
              }
            }
          }}
          onPointerLeave={(e) => e.preventDefault()}
        >
          <BranchIcon className="h-3.5 w-3.5" />
          {label}
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <DropdownMenuItem
            onSelect={() => {
              if (isRemote) {
                handleCheckoutRemote(b)
              } else {
                handleCheckout(b)
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
          {!isRemote && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                variant="destructive"
                onSelect={() => {
                  setDropdownOpen(false)
                  setConfirmAction({ type: "delete", branchName: b })
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

  if (branch === null) {
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button className="flex items-center gap-1 text-sm tracking-tight hover:text-foreground/80 transition-colors outline-none cursor-default">
            <GitFork className="h-3 w-3 shrink-0" />
            <span className="truncate">{t("versionControl")}</span>
            <ChevronDown className="h-3 w-3 shrink-0 opacity-50" />
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
          <button className="flex items-center gap-1 text-sm tracking-tight hover:text-foreground/80 transition-colors outline-none cursor-default">
            <GitBranch className="h-3 w-3 shrink-0" />
            <span className="truncate">{branch}</span>
            <ChevronDown className="h-3 w-3 shrink-0 opacity-50" />
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
                if (!folder) return
                setDropdownOpen(false)
                openCommitWindow(folder.id).catch((err) => {
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
                if (!folder) return
                setDropdownOpen(false)
                openPushWindow(folder.id).catch((err) => {
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
                if (!folder) return
                openStashWindow(folder.id).catch((err) => {
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
              <Collapsible open={localOpen} onOpenChange={setLocalOpen}>
                <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("localBranches", { count: branchList.local.length })}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  {branchList.local.length === 0 ? (
                    <DropdownMenuItem disabled>
                      {t("noLocalBranches")}
                    </DropdownMenuItem>
                  ) : (
                    branchList.local.map((b) => renderBranchItem(b, false))
                  )}
                </CollapsibleContent>
              </Collapsible>

              <Collapsible open={remoteOpen} onOpenChange={setRemoteOpen}>
                <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                  {t("remoteBranches", { count: branchList.remote.length })}
                </CollapsibleTrigger>
                <CollapsibleContent>
                  {branchList.remote.length === 0 ? (
                    <DropdownMenuItem disabled>
                      {t("noRemoteBranches")}
                    </DropdownMenuItem>
                  ) : hasMultipleRemotes ? (
                    remoteNames.map((remoteName) => (
                      <Collapsible key={remoteName}>
                        <CollapsibleTrigger className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 pl-6 text-sm hover:bg-accent hover:text-accent-foreground select-none outline-hidden">
                          <ChevronRight className="h-3 w-3 shrink-0 transition-transform [[data-state=open]>&]:rotate-90" />
                          {remoteName} (
                          {groupedRemoteBranches[remoteName].length})
                        </CollapsibleTrigger>
                        <CollapsibleContent className="pl-3">
                          {groupedRemoteBranches[remoteName].map((b) =>
                            renderBranchItem(
                              b,
                              true,
                              b.substring(remoteName.length + 1)
                            )
                          )}
                        </CollapsibleContent>
                      </Collapsible>
                    ))
                  ) : (
                    branchList.remote.map((b) => {
                      const slashIndex = b.indexOf("/")
                      const shortName =
                        slashIndex > 0 ? b.substring(slashIndex + 1) : b
                      return renderBranchItem(b, true, shortName)
                    })
                  )}
                </CollapsibleContent>
              </Collapsible>
            </ScrollArea>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      {parentBranch && (
        <button
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-orange-500 dark:text-orange-400 hover:bg-accent hover:text-orange-600 dark:hover:text-orange-300 transition-colors cursor-default select-none"
          disabled={loading}
          onClick={handleMergeParent}
          title={t("parentBranchHint", { parentBranch })}
        >
          <ArrowLeftRight className="h-3 w-3 shrink-0" />
          <span className="truncate max-w-32">{parentBranch}</span>
        </button>
      )}

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
                confirmAction?.type === "delete" ? "destructive" : "default"
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
                onChange={(e) => handleWorktreeBranchChange(e.target.value)}
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

      <RemoteManageDialog
        open={manageRemotesOpen}
        onOpenChange={setManageRemotesOpen}
        folderPath={folderPath}
        onSaved={() => loadAllBranches()}
      />

      <ConflictDialog
        conflictInfo={conflictInfo}
        folderId={folder?.id ?? 0}
        folderPath={folderPath}
        onClose={() => setConflictInfo(null)}
        onResolved={onBranchChange}
      />

      <StashDialog
        open={stashDialogOpen}
        folderPath={folderPath}
        onClose={() => setStashDialogOpen(false)}
        onStashed={onBranchChange}
      />
    </>
  )
}
