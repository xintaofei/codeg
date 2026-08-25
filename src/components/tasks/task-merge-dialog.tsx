"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { CircleAlert, Clock } from "lucide-react"
import { workTaskMerge, workTaskSettingsEffective } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import type { WorkTask } from "@/lib/types"

interface TaskMergeDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  task: WorkTask | null
  /** Another task of the same project is landing right now, so this merge will
   *  join the queue instead of starting. Drives the wording — the backend
   *  decides for real, and the result of the call is what the toast reports. */
  folderMerging?: boolean
  /** This task is already in the queue: submitting updates its intent (and
   *  keeps its place in line) rather than adding a second one. */
  alreadyQueued?: boolean
}

/**
 * Accept a reviewed task. The merge itself is performed by the agent in the
 * task's session (conflicts resolved in the same turn), so the form is down to
 * two choices: let the agent write the commit message (default) or provide one,
 * and whether to delete the worktree after landing. Submit awaits only the
 * dispatch; the outcome rides `task://changed` (merging → done, or back to
 * review with a readable error on the card).
 *
 * A project lands one task at a time, so a submit that arrives while another
 * merge is running is QUEUED — the dialog says so up front and the button
 * changes what it promises. Refusals are shown INSIDE the dialog: a toast
 * raised behind an open modal is the one message the user cannot read, and
 * "nothing happened" is exactly how that reads.
 */
export function TaskMergeDialog({
  open,
  onOpenChange,
  task,
  folderMerging = false,
  alreadyQueued = false,
}: TaskMergeDialogProps) {
  const t = useTranslations("Tasks")
  const [autoMessage, setAutoMessage] = useState(true)
  const [message, setMessage] = useState("")
  const [instructions, setInstructions] = useState("")
  const [deleteWorktree, setDeleteWorktree] = useState(true)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Seeded off the task's VALUES, never off the row object: the board hands out
  // a fresh array on every refetch, and re-seeding on identity would wipe a
  // half-typed commit message (or the refusal above the buttons) whenever any
  // task anywhere changed.
  const taskId = task?.id ?? null
  const folderId = task?.folder_id ?? null
  const seedMessage = task?.title ?? ""
  // A task that is ALREADY queued opens on the merge it has parked — reopening
  // to change one thing must not silently discard the commit message and the
  // worktree choice made when it was queued.
  const queuedMessage = task?.merge_queued?.message ?? null
  const queuedInstructions = task?.merge_queued?.instructions ?? null
  const queuedDeleteWorktree = task?.merge_queued?.delete_worktree ?? null

  useEffect(() => {
    if (!open || taskId == null || folderId == null) return
    // Seed per open: the parked merge if there is one, else the task title and
    // the folder's effective delete-worktree default.
    /* eslint-disable react-hooks/set-state-in-effect */
    setAutoMessage(queuedMessage == null)
    setMessage(queuedMessage ?? seedMessage)
    setInstructions(queuedInstructions ?? "")
    setSubmitting(false)
    setError(null)
    if (queuedDeleteWorktree != null) {
      setDeleteWorktree(queuedDeleteWorktree)
      return
    }
    let cancelled = false
    workTaskSettingsEffective(folderId)
      .then((s) => {
        if (cancelled) return
        setDeleteWorktree(s.delete_worktree_default)
      })
      .catch(() => {
        if (cancelled) return
        setDeleteWorktree(true)
      })
    return () => {
      cancelled = true
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [
    open,
    taskId,
    folderId,
    seedMessage,
    queuedMessage,
    queuedInstructions,
    queuedDeleteWorktree,
  ])

  // "Will be queued" as far as the client can tell. The engine re-checks under
  // the project's git lock, and the call reports what actually happened.
  const willQueue = folderMerging || alreadyQueued

  const submit = async () => {
    if (!task || (!autoMessage && !message.trim())) return
    setSubmitting(true)
    setError(null)
    try {
      const queued = await workTaskMerge(
        task.id,
        autoMessage ? null : message.trim(),
        deleteWorktree,
        instructions.trim() || null
      )
      onOpenChange(false)
      // Only the queued outcome needs saying: a started merge is already
      // visible on the card as "合并中", while a queued one looks like nothing
      // happened until the card is read closely.
      if (queued) toast.success(t("mergeQueuedToast"))
    } catch (e) {
      setError(toErrorMessage(e))
      setSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[28rem]">
        <DialogHeader>
          <DialogTitle>
            {alreadyQueued ? t("mergeQueuedTitle") : t("mergeTitle")}
          </DialogTitle>
          <DialogDescription>
            {task
              ? t("mergeDescription", {
                  branch: task.work_branch ?? "?",
                  base: task.base_branch ?? "?",
                })
              : null}
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          {willQueue ? (
            <p className="flex items-start gap-2 rounded-lg bg-amber-500/10 px-2.5 py-2 text-[0.8125rem] leading-snug text-amber-700 dark:text-amber-400">
              <Clock className="mt-px size-3.5 shrink-0" aria-hidden="true" />
              <span>
                {alreadyQueued
                  ? t("mergeQueueUpdateHint")
                  : t("mergeQueueHint")}
              </span>
            </p>
          ) : null}

          {/* First, and above the two checkboxes: this is the only field that
              takes free text about THIS landing, and the one the user is most
              likely to have come here to fill. Optional all the same — it never
              gates the button, because the standing recipe lands the task on
              its own and an empty box just means "do that". */}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="task-merge-instructions">
              {t("mergeInstructions")}
            </Label>
            <Textarea
              id="task-merge-instructions"
              value={instructions}
              onChange={(e) => setInstructions(e.target.value)}
              placeholder={t("mergeInstructionsPlaceholder")}
              rows={2}
            />
          </div>

          {/* The two options, kept adjacent — splitting them around a text box
              read as two unrelated sections rather than one set of choices. */}
          <Label className="text-sm font-normal">
            <Checkbox
              checked={autoMessage}
              onCheckedChange={(v) => setAutoMessage(v === true)}
            />
            {t("mergeAutoMessage")}
          </Label>

          {autoMessage ? null : (
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="task-merge-message">{t("mergeMessage")}</Label>
              <Textarea
                id="task-merge-message"
                value={message}
                onChange={(e) => setMessage(e.target.value)}
                placeholder={t("mergeMessagePlaceholder")}
                rows={3}
              />
            </div>
          )}

          <Label className="text-sm font-normal">
            <Checkbox
              checked={deleteWorktree}
              onCheckedChange={(v) => setDeleteWorktree(v === true)}
            />
            {t("mergeDeleteWorktree")}
          </Label>

          {error ? (
            <p
              role="alert"
              className="flex items-start gap-2 rounded-lg bg-destructive/10 px-2.5 py-2 text-[0.8125rem] leading-snug text-destructive"
            >
              <CircleAlert
                className="mt-px size-3.5 shrink-0"
                aria-hidden="true"
              />
              <span className="min-w-0 break-words">{error}</span>
            </p>
          ) : null}
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="ghost"
            onClick={() => onOpenChange(false)}
            disabled={submitting}
          >
            {t("cancel")}
          </Button>
          <Button
            type="button"
            onClick={submit}
            disabled={submitting || (!autoMessage && !message.trim())}
          >
            {willQueue ? t("mergeSubmitQueue") : t("mergeSubmit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
