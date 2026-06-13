"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2, MoreVertical, Plus, Trash2 } from "lucide-react"

import {
  createLoopIssue,
  deleteLoopIssue,
  listLoopIssues,
} from "@/lib/loops-api"
import type {
  LoopIssuePriority,
  LoopIssueRow,
  LoopIssueStatus,
} from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
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
import {
  IssuePriorityBadge,
  IssueRouteBadge,
  IssueStatusBadge,
} from "@/components/loops/issue-badges"

const ALL_STATUSES: LoopIssueStatus[] = [
  "pending",
  "running",
  "paused",
  "blocked",
  "done",
  "cancelled",
]
const DEFAULT_FILTER: LoopIssueStatus[] = ["pending", "running"]

export function IssueList({
  spaceId,
  selectedIssueId,
  onSelectIssue,
}: {
  spaceId: number
  selectedIssueId: number | null
  onSelectIssue: (id: number) => void
}) {
  const t = useTranslations("Loops.issueList")
  const tStatus = useTranslations("Loops.status")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [issues, setIssues] = useState<LoopIssueRow[]>([])
  const [loading, setLoading] = useState(true)
  const [filter, setFilter] = useState<Set<LoopIssueStatus>>(
    () => new Set(DEFAULT_FILTER)
  )
  const [createOpen, setCreateOpen] = useState(false)
  const [deleting, setDeleting] = useState<LoopIssueRow | null>(null)
  const [deleteBusy, setDeleteBusy] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const statuses = filter.size > 0 ? [...filter] : undefined
      const list = await listLoopIssues(spaceId, statuses)
      setIssues(list)
    } catch {
      // listing failures are non-fatal here; the empty state covers it
    } finally {
      setLoading(false)
    }
  }, [spaceId, filter])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  }, spaceId)

  const toggleStatus = (status: LoopIssueStatus) => {
    setFilter((prev) => {
      const next = new Set(prev)
      if (next.has(status)) next.delete(status)
      else next.add(status)
      return next
    })
  }

  const handleDelete = async () => {
    if (!deleting) return
    setDeleteBusy(true)
    try {
      await deleteLoopIssue(deleting.id)
      setDeleting(null)
      await refresh()
    } catch (err) {
      toast.error(
        tToasts("issueDeleteFailed", { message: toErrorMessage(err) })
      )
    } finally {
      setDeleteBusy(false)
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-2 px-3 pt-3">
        <span className="text-sm font-medium">{t("title")}</span>
        <Button size="sm" className="h-7" onClick={() => setCreateOpen(true)}>
          <Plus className="mr-1 h-3.5 w-3.5" />
          {t("newIssue")}
        </Button>
      </div>

      <div className="flex shrink-0 flex-wrap gap-1 px-3 py-2">
        {ALL_STATUSES.map((status) => {
          const active = filter.has(status)
          return (
            <button
              key={status}
              type="button"
              onClick={() => toggleStatus(status)}
              className={cn(
                "rounded-full border px-2 py-0.5 text-xs transition-colors",
                active
                  ? "border-primary/40 bg-primary/10 text-primary"
                  : "border-transparent text-muted-foreground hover:bg-accent"
              )}
            >
              {tStatus(status)}
            </button>
          )
        })}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
        {loading ? (
          <div className="flex h-24 items-center justify-center text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : issues.length === 0 ? (
          <p className="px-3 py-6 text-center text-xs text-muted-foreground">
            {t("empty")}
          </p>
        ) : (
          <ul className="space-y-1">
            {issues.map((issue) => (
              <li key={issue.id}>
                <div
                  role="button"
                  tabIndex={0}
                  onClick={() => onSelectIssue(issue.id)}
                  onKeyDown={(e) => {
                    if (e.target !== e.currentTarget) return
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault()
                      onSelectIssue(issue.id)
                    }
                  }}
                  className={cn(
                    "group flex cursor-pointer flex-col gap-1.5 rounded-md border px-2.5 py-2 outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
                    selectedIssueId === issue.id
                      ? "border-primary/40 bg-accent"
                      : "border-transparent"
                  )}
                >
                  <div className="flex items-start gap-2">
                    <span className="mt-0.5 shrink-0 font-mono text-xs text-muted-foreground">
                      #{issue.seq_no}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-sm">
                      {issue.title}
                    </span>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-6 w-6 shrink-0 opacity-0 transition-opacity group-hover:opacity-100 data-[state=open]:opacity-100"
                          onClick={(e) => e.stopPropagation()}
                          aria-label={t("deleteIssue")}
                        >
                          <MoreVertical className="h-3.5 w-3.5" />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent
                        align="end"
                        onClick={(e) => e.stopPropagation()}
                      >
                        <DropdownMenuItem
                          onSelect={() => setDeleting(issue)}
                          className="text-destructive focus:text-destructive"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                          {t("deleteIssue")}
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                  <div className="flex flex-wrap items-center gap-1">
                    <IssueStatusBadge status={issue.status} />
                    <IssuePriorityBadge priority={issue.priority} />
                    <IssueRouteBadge route={issue.route} />
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <IssueFormDialog
        spaceId={spaceId}
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={(issue) => {
          void refresh()
          onSelectIssue(issue.id)
        }}
      />

      <AlertDialog
        open={deleting != null}
        onOpenChange={(o) => !o && setDeleting(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("confirmDeleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("confirmDeleteDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteBusy}>
              {tCommon("cancel")}
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault()
                void handleDelete()
              }}
              disabled={deleteBusy}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteBusy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {tCommon("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

const PRIORITIES: LoopIssuePriority[] = ["high", "medium", "low"]

function IssueFormDialog({
  spaceId,
  open,
  onOpenChange,
  onCreated,
}: {
  spaceId: number
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (issue: LoopIssueRow) => void
}) {
  const t = useTranslations("Loops.issueForm")
  const tPriority = useTranslations("Loops.priority")
  const tToasts = useTranslations("Loops.toasts")

  const [title, setTitle] = useState("")
  const [description, setDescription] = useState("")
  const [priority, setPriority] = useState<LoopIssuePriority>("medium")
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (open) {
      setTitle("")
      setDescription("")
      setPriority("medium")
      setBusy(false)
    }
  }, [open])

  const handleCreate = async () => {
    if (!title.trim() || busy) return
    setBusy(true)
    try {
      const issue = await createLoopIssue({
        spaceId,
        title: title.trim(),
        description: description.trim(),
        priority,
      })
      toast.success(tToasts("issueCreated", { title: issue.title }))
      onCreated(issue)
      onOpenChange(false)
    } catch (err) {
      toast.error(
        tToasts("issueCreateFailed", { message: toErrorMessage(err) })
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("createTitle")}</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-2">
            <Label htmlFor="loop-issue-title">{t("titleLabel")}</Label>
            <Input
              id="loop-issue-title"
              placeholder={t("titlePlaceholder")}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              disabled={busy}
              autoFocus
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="loop-issue-desc">{t("descriptionLabel")}</Label>
            <Textarea
              id="loop-issue-desc"
              placeholder={t("descriptionPlaceholder")}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              disabled={busy}
              rows={5}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="loop-issue-priority">{t("priorityLabel")}</Label>
            <Select
              value={priority}
              onValueChange={(v) => setPriority(v as LoopIssuePriority)}
              disabled={busy}
            >
              <SelectTrigger id="loop-issue-priority" className="w-40">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {PRIORITIES.map((p) => (
                  <SelectItem key={p} value={p}>
                    {tPriority(p)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={busy}
            type="button"
          >
            {t("cancel")}
          </Button>
          <Button
            onClick={handleCreate}
            disabled={!title.trim() || busy}
            type="button"
          >
            {busy && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("create")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
