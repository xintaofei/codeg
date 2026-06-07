"use client"

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Check, ChevronDown, Folder, GitBranch, Loader2 } from "lucide-react"
import type { OverlayScrollbarsComponentRef } from "overlayscrollbars-react"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { useTabContext } from "@/contexts/tab-context"
import { useTaskContext } from "@/contexts/task-context"
import { gitListAllBranches, gitCheckout } from "@/lib/api"
import type { GitBranchList } from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"
import { toErrorMessage } from "@/lib/app-error"
import {
  filterTopLevelFolders,
  resolveFolderDisplayName,
  resolvePickerSelectedFolderId,
} from "@/lib/folder-display"
import { useSwitchToBranch } from "@/hooks/use-switch-to-branch"

interface ConversationContextBarProps {
  extraContent?: React.ReactNode
  hasExtraContent?: boolean
  scrollEndTrigger?: number
}

export const ConversationContextBar = memo(function ConversationContextBar({
  extraContent,
  hasExtraContent = false,
  scrollEndTrigger,
}: ConversationContextBarProps = {}) {
  const scrollRef = useRef<OverlayScrollbarsComponentRef>(null)
  const innerRef = useRef<HTMLDivElement>(null)
  const prevScrollTriggerRef = useRef<number>(scrollEndTrigger ?? 0)
  useEffect(() => {
    if (scrollEndTrigger == null) return
    if (scrollEndTrigger <= prevScrollTriggerRef.current) {
      prevScrollTriggerRef.current = scrollEndTrigger
      return
    }
    prevScrollTriggerRef.current = scrollEndTrigger
    requestAnimationFrame(() => {
      const viewport = scrollRef.current?.osInstance()?.elements().viewport
      if (!viewport) return
      viewport.scrollTo({ left: viewport.scrollWidth, behavior: "smooth" })
    })
  }, [scrollEndTrigger])

  useEffect(() => {
    if (!hasExtraContent) return
    const inner = innerRef.current
    if (!inner) return
    const handler = (e: WheelEvent) => {
      const viewport = scrollRef.current?.osInstance()?.elements().viewport
      if (!viewport) return
      if (viewport.scrollWidth <= viewport.clientWidth) return
      const delta = e.deltaY !== 0 ? e.deltaY : e.deltaX
      if (delta === 0) return
      e.preventDefault()
      viewport.scrollLeft += delta
    }
    inner.addEventListener("wheel", handler, { passive: false })
    return () => inner.removeEventListener("wheel", handler)
  }, [hasExtraContent])

  if (!hasExtraContent) return null

  return (
    <div className="flex shrink-0 items-center gap-1.5 px-2 pt-2 text-xs text-muted-foreground">
      <ScrollArea
        x="scroll"
        y="hidden"
        className="min-w-0 flex-1"
        ref={scrollRef}
      >
        <div ref={innerRef} className="flex w-max items-center gap-1.5">
          {extraContent}
        </div>
      </ScrollArea>
    </div>
  )
})

ConversationContextBar.displayName = "ConversationContextBar"

// ============================================================================
// ConversationFolderBranchPicker — folder + branch buttons rendered below the
// message input.
// ============================================================================

interface ConversationFolderBranchPickerProps {
  tabId?: string | null
}

export const ConversationFolderBranchPicker = memo(
  function ConversationFolderBranchPicker({
    tabId,
  }: ConversationFolderBranchPickerProps) {
    const t = useTranslations("Folder.conversationContextBar")
    const tBd = useTranslations("Folder.branchDropdown")
    const { tabs, activeTabId, openNewConversationTab } = useTabContext()
    const { folders, allFolders, branches, setBranch, refreshFolder } =
      useAppWorkspace()
    const { addTask, updateTask } = useTaskContext()
    const switchToBranch = useSwitchToBranch()

    const ownTab = useMemo(() => {
      const lookupId = tabId ?? activeTabId
      return tabs.find((x) => x.id === lookupId) ?? null
    }, [tabs, tabId, activeTabId])

    const ownFolder = useMemo(
      () =>
        ownTab
          ? (allFolders.find((f) => f.id === ownTab.folderId) ?? null)
          : null,
      [ownTab, allFolders]
    )

    // The folder picker lists only top-level repos — worktree folders
    // (`parent_id != null`) are reached through the branch picker, not here, so
    // they're hidden to keep this picker a clean repo switcher.
    const topLevelFolders = useMemo(
      () => filterTopLevelFolders(folders),
      [folders]
    )

    if (!ownTab || !ownFolder) return null

    const isNewConversation = ownTab.conversationId == null
    const currentBranch =
      branches.get(ownFolder.id) ?? ownFolder.git_branch ?? null
    const showBranchPicker = currentBranch != null
    // Worktree folders surface their parent (root repo) name here; the picker's
    // own list below keeps real folder names/paths for selection, and every
    // git/path operation still uses `ownFolder` (the worktree) unchanged.
    const displayFolderName = resolveFolderDisplayName(ownFolder, allFolders)
    // When the conversation lives in a worktree, the picker highlights its
    // parent repo (the worktree itself isn't listed). Display-only — the tab's
    // real folder/working dir is untouched.
    const pickerSelectedId = resolvePickerSelectedFolderId(ownFolder)

    return (
      <>
        <FolderPicker
          folders={topLevelFolders}
          currentFolderId={pickerSelectedId}
          currentFolderName={displayFolderName}
          title={`${t("folderTitle")}: ${displayFolderName}`}
          editable={isNewConversation}
          onSelect={async (folderId) => {
            const target = folders.find((f) => f.id === folderId)
            if (!target) return
            try {
              // Route through openNewConversationTab so the target folder's
              // saved default agent is applied. The function's existing-
              // draft branch reuses ownTab via the singleton invariant and
              // runs the disconnect-then-patch dance for folder+agent
              // changes. `inheritFromActive: true` preserves the user's
              // current agent when the target folder has no pinned default
              // — "I'm switching folders, keep my workflow".
              openNewConversationTab(target.id, target.path, {
                inheritFromActive: true,
              })
              toast.success(t("toasts.folderChanged", { name: target.name }))
            } catch (err) {
              console.error(
                "[ConversationFolderBranchPicker] switch folder failed:",
                err
              )
              toast.error(t("toasts.openFolderFailed"))
            }
          }}
          labelEmpty={t("noFolders")}
          labelSearch={t("searchFolder")}
        />

        {showBranchPicker && (
          <BranchPicker
            folderId={ownFolder.id}
            folderPath={ownFolder.path}
            currentBranch={currentBranch}
            title={`${t("branchTitle")}: ${currentBranch ?? t("noBranch")}`}
            onCheckout={async (branchName, isRemote) => {
              // Draft conversation: route through the shared switch logic so a
              // worktree branch navigates to its folder instead of a doomed
              // in-place checkout.
              if (isNewConversation) {
                await switchToBranch({
                  activeFolder: ownFolder,
                  branchName,
                  currentBranch,
                  isRemote,
                })
                return
              }
              // Existing conversation: check out in place in its own folder —
              // never navigate away from a live conversation's working dir.
              const taskId = `checkout-${ownFolder.id}-${Date.now()}`
              addTask(taskId, tBd("tasks.checkoutTo", { branchName }))
              updateTask(taskId, { status: "running" })
              try {
                await gitCheckout(ownFolder.path, branchName)
                setBranch(ownFolder.id, branchName)
                await refreshFolder(ownFolder.id)
                updateTask(taskId, { status: "completed" })
              } catch (err) {
                const msg = toErrorMessage(err)
                updateTask(taskId, { status: "failed", error: msg })
                toast.error(msg)
              }
            }}
          />
        )}
      </>
    )
  }
)

ConversationFolderBranchPicker.displayName = "ConversationFolderBranchPicker"

/**
 * Mirror the visibility check inside `ConversationFolderBranchPicker` so the
 * parent can decide whether to render its wrapper row at all. The picker
 * itself returns `null` when no tab/folder is resolved (e.g. while folders
 * are still loading on first paint), and the parent must avoid rendering an
 * otherwise-empty wrapper in that interval.
 */
export function useConversationFolderBranchPickerVisible(
  tabId?: string | null
): boolean {
  const { tabs, activeTabId } = useTabContext()
  const { allFolders } = useAppWorkspace()
  const lookupId = tabId ?? activeTabId
  const ownTab = tabs.find((x) => x.id === lookupId) ?? null
  const ownFolder = ownTab
    ? (allFolders.find((f) => f.id === ownTab.folderId) ?? null)
    : null
  return Boolean(ownTab && ownFolder)
}

// ============================================================================
// FolderPicker
// ============================================================================

interface FolderPickerProps {
  folders: { id: number; name: string; path: string }[]
  currentFolderId: number
  currentFolderName: string
  title: string
  editable: boolean
  onSelect: (folderId: number) => void | Promise<void>
  labelEmpty: string
  labelSearch: string
}

const FolderPicker = memo(function FolderPicker({
  folders,
  currentFolderId,
  currentFolderName,
  title,
  editable,
  onSelect,
  labelEmpty,
  labelSearch,
}: FolderPickerProps) {
  const [open, setOpen] = useState(false)

  const trigger = (
    <Button
      variant="ghost"
      size="xs"
      title={title}
      className={cn(
        "min-w-0",
        !editable && "cursor-default opacity-60 hover:bg-transparent"
      )}
    >
      <Folder className="size-3 shrink-0 text-muted-foreground" />
      <span className="max-w-[140px] truncate">{currentFolderName}</span>
      <ChevronDown className="size-3 shrink-0 text-muted-foreground/60" />
    </Button>
  )

  if (!editable) {
    return trigger
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent align="start" className="p-0 w-72 overflow-hidden">
        <Command className="rounded-2xl">
          <CommandInput placeholder={labelSearch} />
          <CommandList>
            <CommandEmpty>{labelEmpty}</CommandEmpty>
            <CommandGroup>
              {folders.map((f) => (
                <CommandItem
                  key={f.id}
                  value={`${f.name} ${f.path}`}
                  onSelect={() => {
                    setOpen(false)
                    void onSelect(f.id)
                  }}
                >
                  <Folder className="h-4 w-4" />
                  <div className="flex flex-col min-w-0 flex-1">
                    <span className="truncate font-medium">{f.name}</span>
                    <span className="truncate text-xs text-muted-foreground">
                      {f.path}
                    </span>
                  </div>
                  {f.id === currentFolderId && (
                    <Check className="h-4 w-4 shrink-0" />
                  )}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
})

// ============================================================================
// BranchPicker
// ============================================================================

interface BranchPickerProps {
  folderId: number
  folderPath: string
  currentBranch: string | null
  title: string
  onCheckout: (branchName: string, isRemote: boolean) => Promise<void>
}

const BranchPicker = memo(function BranchPicker({
  folderId,
  folderPath,
  currentBranch,
  title,
  onCheckout,
}: BranchPickerProps) {
  const t = useTranslations("Folder.conversationContextBar")
  const tBd = useTranslations("Folder.branchDropdown")
  const [open, setOpen] = useState(false)
  const [branchList, setBranchList] = useState<GitBranchList | null>(null)
  const [loading, setLoading] = useState(false)

  const loadBranches = useCallback(async () => {
    setLoading(true)
    try {
      const list = await gitListAllBranches(folderPath)
      setBranchList(list)
    } catch (err) {
      console.error("[BranchPicker] list failed:", err)
      setBranchList({ local: [], remote: [], worktree_branches: [] })
    } finally {
      setLoading(false)
    }
  }, [folderPath])

  useEffect(() => {
    if (open) void loadBranches()
  }, [open, loadBranches])

  // Reset branches cache when folder changes
  useEffect(() => {
    setBranchList(null)
  }, [folderId])

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="xs" title={title} className="min-w-0">
          <GitBranch className="size-3 shrink-0 text-muted-foreground" />
          <span className="max-w-[160px] truncate">
            {currentBranch ?? t("noBranch")}
          </span>
          <ChevronDown className="size-3 shrink-0 text-muted-foreground/60" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="p-0 w-80 overflow-hidden">
        <Command className="rounded-2xl">
          <CommandInput placeholder={t("searchBranch")} />
          <CommandList>
            {loading ? (
              <div className="py-6 text-center text-xs text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin mx-auto" />
              </div>
            ) : (
              <>
                <CommandEmpty>{t("noBranches")}</CommandEmpty>
                {branchList && branchList.local.length > 0 && (
                  <CommandGroup
                    heading={tBd("localBranches", {
                      count: branchList.local.length,
                    })}
                  >
                    {branchList.local.map((b) => (
                      <CommandItem
                        key={`local-${b}`}
                        value={`local ${b}`}
                        onSelect={() => {
                          setOpen(false)
                          if (b !== currentBranch) void onCheckout(b, false)
                        }}
                      >
                        <GitBranch className="h-4 w-4" />
                        <span className="flex-1 truncate">{b}</span>
                        {b === currentBranch && (
                          <Check className="h-4 w-4 shrink-0" />
                        )}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                )}
                {branchList && branchList.remote.length > 0 && (
                  <CommandGroup
                    heading={tBd("remoteBranches", {
                      count: branchList.remote.length,
                    })}
                  >
                    {branchList.remote.map((b) => {
                      const localName = b.replace(/^[^/]+\//, "")
                      return (
                        <CommandItem
                          key={`remote-${b}`}
                          value={`remote ${b}`}
                          onSelect={() => {
                            setOpen(false)
                            if (localName !== currentBranch)
                              void onCheckout(localName, true)
                          }}
                        >
                          <GitBranch className="h-4 w-4 opacity-60" />
                          <span className="flex-1 truncate text-muted-foreground">
                            {b}
                          </span>
                          {localName === currentBranch && (
                            <Check className="h-4 w-4 shrink-0" />
                          )}
                        </CommandItem>
                      )
                    })}
                  </CommandGroup>
                )}
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
})
