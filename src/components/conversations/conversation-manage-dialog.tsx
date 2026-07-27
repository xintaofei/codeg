"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  CheckSquare,
  ChevronDown,
  ListChecks,
  Loader2,
  Square,
  Trash2,
} from "lucide-react"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
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
import { AgentIcon } from "@/components/agent-icon"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useTabActions } from "@/contexts/tab-context"
import {
  deleteConversation,
  listAllConversations,
  updateConversationStatus,
} from "@/lib/api"
import type {
  AgentType,
  ConversationStatus,
  DbConversationSummary,
} from "@/lib/types"
import { ALL_AGENT_TYPES, STATUS_ORDER } from "@/lib/types"
import { getAgentLabel } from "@/lib/custom-agents"
import { cn } from "@/lib/utils"
import { formatConversationTitle } from "@/lib/conversation-title"
import { toErrorMessage } from "@/lib/app-error"
import { ConversationStatusDot } from "@/components/conversations/conversation-status-dot"

interface ConversationManageDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  folderId: number
  folderName: string
}

function parseTimestamp(value: string): number {
  const ts = Date.parse(value)
  return Number.isNaN(ts) ? 0 : ts
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

export function ConversationManageDialog({
  open,
  onOpenChange,
  folderId,
  folderName,
}: ConversationManageDialogProps) {
  const t = useTranslations("Folder.sidebar.manageConversations")
  const tCommon = useTranslations("Folder.common")
  const tStatus = useTranslations("Folder.statusLabels")

  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const allFolders = useAppWorkspaceStore((s) => s.allFolders)
  const { closeConversationTab } = useTabActions()

  // The dialog is always opened on a top-level repo folder (worktree children
  // get no header/menu entry of their own). Worktree conversations, however,
  // keep `folder_id = the worktree child folder` — the sidebar merges them into
  // the parent group as a display-only redirect and never rewrites `folder_id`.
  // So query the parent id PLUS every worktree child folder id, mirroring that
  // merge; otherwise `list_all`'s exact `folder_id IN (...)` filter drops them.
  // `allFolders` includes open and closed (non-deleted) folders, and a
  // worktree's `parent_id` is flattened to the root, so one level of filtering
  // is complete.
  const queryFolderIds = useMemo(() => {
    const ids = [folderId]
    for (const f of allFolders) {
      if (f.parent_id === folderId) ids.push(f.id)
    }
    return ids
  }, [allFolders, folderId])

  const [search, setSearch] = useState("")
  const [agentFilter, setAgentFilter] = useState<AgentType | "all">("all")
  const [statusFilter, setStatusFilter] = useState<ConversationStatus | "all">(
    "all"
  )
  const [rows, setRows] = useState<DbConversationSummary[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [pending, setPending] = useState(false)
  const [refreshKey, setRefreshKey] = useState(0)
  const [confirmDelete, setConfirmDelete] = useState(false)

  // Reset state on open/close transitions
  useEffect(() => {
    if (!open) {
      setSearch("")
      setAgentFilter("all")
      setStatusFilter("all")
      setSelected(new Set())
      setConfirmDelete(false)
      setError(null)
    }
  }, [open])

  // Debounced data fetch
  useEffect(() => {
    if (!open) return
    const timer = setTimeout(async () => {
      setLoading(true)
      try {
        const data = await listAllConversations({
          folder_ids: queryFolderIds,
          search: search.trim() || null,
          agent_type: agentFilter === "all" ? null : agentFilter,
          status: statusFilter === "all" ? null : statusFilter,
        })
        const sorted = [...data].sort(
          (a, b) => parseTimestamp(b.created_at) - parseTimestamp(a.created_at)
        )
        setRows(sorted)
        setError(null)
      } catch (e) {
        setError(toErrorMessage(e))
      } finally {
        setLoading(false)
      }
    }, 300)
    return () => clearTimeout(timer)
  }, [open, queryFolderIds, search, agentFilter, statusFilter, refreshKey])

  const toggleOne = useCallback((id: number) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  const allVisibleSelected = useMemo(
    () => rows.length > 0 && rows.every((r) => selected.has(r.id)),
    [rows, selected]
  )

  const toggleSelectAll = useCallback(() => {
    if (allVisibleSelected) {
      setSelected((prev) => {
        const next = new Set(prev)
        for (const r of rows) next.delete(r.id)
        return next
      })
    } else {
      setSelected((prev) => {
        const next = new Set(prev)
        for (const r of rows) next.add(r.id)
        return next
      })
    }
  }, [allVisibleSelected, rows])

  const afterBulkOp = useCallback(() => {
    setSelected(new Set())
    setRefreshKey((k) => k + 1)
    refreshConversations()
  }, [refreshConversations])

  const selectedIds = useMemo(() => [...selected], [selected])
  const selectedCount = selected.size

  const handleBulkDelete = useCallback(async () => {
    if (selectedIds.length === 0) return
    setPending(true)
    try {
      const affected = rows.filter((r) => selected.has(r.id))
      await Promise.all(selectedIds.map((id) => deleteConversation(id)))
      for (const conv of affected) {
        closeConversationTab(conv.folder_id, conv.id, conv.agent_type)
      }
      toast.success(t("toastDeleted", { count: selectedIds.length }))
      afterBulkOp()
    } catch (e) {
      toast.error(
        t("toastOpFailed", {
          message: toErrorMessage(e),
        })
      )
    } finally {
      setPending(false)
      setConfirmDelete(false)
    }
  }, [selectedIds, rows, selected, closeConversationTab, t, afterBulkOp])

  const handleBulkStatus = useCallback(
    async (status: ConversationStatus) => {
      if (selectedIds.length === 0) return
      setPending(true)
      try {
        await Promise.all(
          selectedIds.map((id) => updateConversationStatus(id, status))
        )
        toast.success(t("toastStatusUpdated", { count: selectedIds.length }))
        afterBulkOp()
      } catch (e) {
        toast.error(
          t("toastOpFailed", {
            message: toErrorMessage(e),
          })
        )
      } finally {
        setPending(false)
      }
    },
    [selectedIds, t, afterBulkOp]
  )

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>{t("title", { name: folderName })}</DialogTitle>
          </DialogHeader>

          {/* Filter row */}
          <div className="flex items-center justify-between gap-2">
            <Input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t("searchPlaceholder")}
              className="h-9 w-64"
            />
            <div className="flex items-center gap-2">
              <Select
                value={agentFilter}
                onValueChange={(v) => setAgentFilter(v as AgentType | "all")}
              >
                <SelectTrigger className="h-9 w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t("agentFilterAll")}</SelectItem>
                  {ALL_AGENT_TYPES.map((at) => (
                    <SelectItem key={at} value={at}>
                      <span className="flex items-center gap-2">
                        <AgentIcon agentType={at} className="h-3.5 w-3.5" />
                        {getAgentLabel(at)}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Select
                value={statusFilter}
                onValueChange={(v) =>
                  setStatusFilter(v as ConversationStatus | "all")
                }
              >
                <SelectTrigger className="h-9 w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">{t("statusFilterAll")}</SelectItem>
                  {STATUS_ORDER.map((s) => (
                    <SelectItem key={s} value={s}>
                      <span className="flex items-center gap-2">
                        <ConversationStatusDot status={s} />
                        {tStatus(s)}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          {/* List container: select-all header + scrollable list */}
          <div className="flex flex-col rounded-md border border-border/50 overflow-hidden">
            <div className="flex items-center justify-between px-3 py-2 border-b border-border/50 bg-muted/20">
              <button
                type="button"
                onClick={toggleSelectAll}
                disabled={rows.length === 0}
                className="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
              >
                <span className="flex h-5 w-5 items-center justify-center">
                  {allVisibleSelected ? (
                    <CheckSquare className="h-4 w-4 text-primary" />
                  ) : (
                    <Square className="h-4 w-4" />
                  )}
                </span>
                {allVisibleSelected ? t("deselectAll") : t("selectAllVisible")}
              </button>
              <span className="text-xs text-muted-foreground">
                {t("matchedCount", { count: rows.length })}
              </span>
            </div>
            <ScrollArea className="h-[26rem]">
              <div className="flex flex-col gap-0.5 p-1">
                {loading ? (
                  Array.from({ length: 6 }).map((_, i) => (
                    <Skeleton key={i} className="h-9 w-full rounded-md" />
                  ))
                ) : error ? (
                  <p className="text-destructive text-sm px-3 py-6 text-center">
                    {error}
                  </p>
                ) : rows.length === 0 ? (
                  <p className="text-muted-foreground text-sm px-3 py-6 text-center">
                    {search.trim() ||
                    agentFilter !== "all" ||
                    statusFilter !== "all"
                      ? t("noMatchingConversations")
                      : t("noConversations")}
                  </p>
                ) : (
                  rows.map((conv) => {
                    const checked = selected.has(conv.id)
                    return (
                      <div
                        key={conv.id}
                        onClick={() => toggleOne(conv.id)}
                        className={cn(
                          "flex items-center gap-2 rounded-md px-2 py-1.5 cursor-pointer border border-transparent",
                          "hover:bg-accent/50",
                          checked && "bg-accent/40 border-accent/60"
                        )}
                      >
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation()
                            toggleOne(conv.id)
                          }}
                          className="flex h-5 w-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground"
                          aria-pressed={checked}
                        >
                          {checked ? (
                            <CheckSquare className="h-4 w-4 text-primary" />
                          ) : (
                            <Square className="h-4 w-4" />
                          )}
                        </button>
                        <AgentIcon
                          agentType={conv.agent_type}
                          className="h-4 w-4 shrink-0"
                        />
                        <span className="flex-1 min-w-0 truncate text-sm">
                          {formatConversationTitle(conv.title) ||
                            t("untitledConversation")}
                        </span>
                        <span className="shrink-0 text-xs text-muted-foreground tabular-nums w-14 text-right">
                          {t("messagesShort", { count: conv.message_count })}
                        </span>
                        <span className="shrink-0 text-xs text-muted-foreground w-10 text-right">
                          {formatRelative(conv.created_at)}
                        </span>
                        <ConversationStatusDot
                          status={conv.status as ConversationStatus}
                          title={
                            STATUS_ORDER.includes(
                              conv.status as ConversationStatus
                            )
                              ? tStatus(conv.status as ConversationStatus)
                              : conv.status
                          }
                        />
                      </div>
                    )
                  })
                )}
              </div>
            </ScrollArea>
          </div>

          {/* Footer: bulk actions */}
          <DialogFooter className="flex items-center justify-between gap-2 sm:justify-between">
            <span className="text-xs text-muted-foreground">
              {t("selectedCount", { count: selectedCount })}
            </span>
            <div className="flex flex-wrap items-center gap-2">
              {/* Set status */}
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={selectedCount === 0 || pending}
                  >
                    <ListChecks className="h-3.5 w-3.5 mr-1" />
                    {t("setStatus")}
                    <ChevronDown className="h-3 w-3 ml-1 opacity-60" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {STATUS_ORDER.map((s) => (
                    <DropdownMenuItem
                      key={s}
                      onSelect={() => handleBulkStatus(s)}
                    >
                      <ConversationStatusDot status={s} />
                      {tStatus(s)}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>

              {/* Delete */}
              <Button
                size="sm"
                variant="destructive"
                disabled={selectedCount === 0 || pending}
                onClick={() => setConfirmDelete(true)}
              >
                {pending ? (
                  <Loader2 className="h-3.5 w-3.5 mr-1 animate-spin" />
                ) : (
                  <Trash2 className="h-3.5 w-3.5 mr-1" />
                )}
                {t("deleteSelected")}
              </Button>

              <Button
                size="sm"
                variant="outline"
                onClick={() => onOpenChange(false)}
              >
                {tCommon("close")}
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Delete confirmation */}
      <AlertDialog
        open={confirmDelete}
        onOpenChange={(o) => !o && setConfirmDelete(false)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("confirmDeleteTitle", { count: selectedCount })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("confirmDeleteDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{tCommon("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleBulkDelete}>
              {tCommon("confirm")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
