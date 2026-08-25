"use client"

import { useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { workTaskDeliverPr, workTaskSettingsEffective } from "@/lib/api"
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
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  isWorktreeGone,
  mustDeliverToPr,
  usesMergeRequests,
} from "./task-acceptance"
import type { WorkTask } from "@/lib/types"

interface TaskDeliverPrDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  task: WorkTask | null
}

/**
 * Accept a reviewed forge-sourced task by delivering it. Two shapes, because
 * the two sources mean different things by "deliver":
 *
 * - an ISSUE's task publishes its own branch and opens (or adopts) the pull
 *   request that carries it — the title and the draft flag are the only
 *   decisions it takes;
 * - a PULL REQUEST's task pushes back onto that pull request's own branch.
 *   Nothing is created, so there is nothing to name: the dialog just says
 *   where the commits are about to go and asks for a confirmation.
 *
 * Everything else — which repository, which account, which base branch, the
 * body with its closing keyword — is derived by the backend from what the task
 * was created with, so nothing typed in this dialog can point the push
 * somewhere else.
 *
 * Both shapes end the task, so both ask what the other two acceptances ask:
 * what happens to the checkout it ran in. Delivering publishes the commits
 * first, so the worktree — and the local branch that goes with it — is as
 * expendable here as after a merge. It is still a choice rather than a
 * consequence, because the review it just fed is open and a second round on
 * this branch is a normal next step.
 *
 * Like the complete dialog and unlike the merge dialog, this settles
 * synchronously: no agent runs, so the command returns once the task is
 * `done`, and a failure belongs here rather than on the card.
 */
export function TaskDeliverPrDialog({
  open,
  onOpenChange,
  task,
}: TaskDeliverPrDialogProps) {
  const t = useTranslations("Tasks")
  const [title, setTitle] = useState("")
  const [draft, setDraft] = useState(false)
  // Off until the folder's default has been read: the seed below is one await
  // away, and a submit that beats it must not delete a worktree on the
  // strength of a value nobody chose — including the value the LAST task
  // opened in this dialog resolved to.
  const [deleteWorktree, setDeleteWorktree] = useState(false)
  // Whether the box has been touched since this open. The seed is async, so
  // without this a user who clears the box inside that window has the folder
  // default land on top of the choice they just made — on the one control
  // here that destroys something.
  const worktreeChoiceMade = useRef(false)
  const [submitting, setSubmitting] = useState(false)
  // The board only offers delivery while the worktree is there, so this is
  // normally true — it guards the stale card, keeping the request honest
  // rather than asking the backend to remove something already gone.
  const hasWorktree = task != null && !isWorktreeGone(task)

  useEffect(() => {
    if (!open || !task) return
    /* eslint-disable react-hooks/set-state-in-effect */
    setSubmitting(false)
    setDraft(false)
    setTitle(task.title)
    setDeleteWorktree(false)
    worktreeChoiceMade.current = false
    // Same seed as the merge and complete dialogs: the folder's
    // worktree-cleanup default, and `true` when it cannot be read (which is
    // what those two fall back to as well). It lands only on a box the user
    // has not answered yet — a default arriving late is still a default.
    let cancelled = false
    const seed = (value: boolean) => {
      if (cancelled || worktreeChoiceMade.current) return
      setDeleteWorktree(value)
    }
    workTaskSettingsEffective(task.folder_id)
      .then((s) => seed(s.delete_worktree_default))
      .catch(() => seed(true))
    return () => {
      cancelled = true
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [open, task])

  const pushBack = task != null && mustDeliverToPr(task)
  // GitLab's word for the same thing. Only the wording differs — every step
  // below is the same push and the same lookup.
  const mr = usesMergeRequests(task)

  const submit = async () => {
    if (!task) return
    setSubmitting(true)
    try {
      const cleanup = hasWorktree && deleteWorktree
      // A push-back creates nothing, so it carries neither title nor draft.
      const url = pushBack
        ? await workTaskDeliverPr(task.id, null, false, cleanup)
        : await workTaskDeliverPr(task.id, title.trim() || null, draft, cleanup)
      onOpenChange(false)
      const done = pushBack
        ? mr
          ? t("deliverPrBackDoneMr")
          : t("deliverPrBackDone")
        : mr
          ? t("deliverPrDoneMr")
          : t("deliverPrDone")
      toast.success(done, {
        action: {
          label: t("deliverPrOpen"),
          onClick: () => window.open(url, "_blank", "noopener,noreferrer"),
        },
      })
    } catch (e) {
      toast.error(toErrorMessage(e))
      setSubmitting(false)
    }
  }

  const repo = task?.source_meta?.owner_repo ?? ""
  const base = task?.base_branch ?? ""
  // As a string: a message placeholder would otherwise be number-formatted,
  // and "#1,234" is not a pull request number anyone recognizes.
  const number = String(task?.source_meta?.number ?? "")
  const branch = task?.source_meta?.head_ref ?? ""

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[30rem]">
        <DialogHeader>
          <DialogTitle>
            {pushBack
              ? mr
                ? t("deliverPrBackTitleMr")
                : t("deliverPrBackTitle")
              : mr
                ? t("deliverPrTitleMr")
                : t("deliverPrTitle")}
          </DialogTitle>
          <DialogDescription>
            {pushBack
              ? mr
                ? t("deliverPrBackDescriptionMr", { repo, number, branch })
                : t("deliverPrBackDescription", { repo, number, branch })
              : mr
                ? t("deliverPrDescriptionMr", { repo, base })
                : t("deliverPrDescription", { repo, base })}
          </DialogDescription>
        </DialogHeader>

        {/* A push-back has nothing to name, so the cleanup checkbox is its
            only control — and a push-back whose worktree is already gone has
            no control at all, which is the block the outer test skips. */}
        {pushBack && !hasWorktree ? null : (
          <div className="flex flex-col gap-3">
            {pushBack ? null : (
              <>
                <Label className="flex flex-col items-start gap-1.5 text-sm font-normal">
                  {t(mr ? "deliverPrTitleLabelMr" : "deliverPrTitleLabel")}
                  <Input
                    value={title}
                    onChange={(e) => setTitle(e.target.value)}
                    disabled={submitting}
                  />
                </Label>
                <Label className="text-sm font-normal">
                  <Checkbox
                    checked={draft}
                    onCheckedChange={(v) => setDraft(v === true)}
                    disabled={submitting}
                  />
                  {t("deliverPrDraft")}
                </Label>
              </>
            )}
            {hasWorktree ? (
              <Label className="text-sm font-normal">
                <Checkbox
                  checked={deleteWorktree}
                  onCheckedChange={(v) => {
                    worktreeChoiceMade.current = true
                    setDeleteWorktree(v === true)
                  }}
                  disabled={submitting}
                />
                {t(
                  pushBack
                    ? "deliverPrBackDeleteWorktree"
                    : "deliverPrDeleteWorktree"
                )}
              </Label>
            ) : null}
          </div>
        )}

        <DialogFooter>
          <Button
            type="button"
            variant="ghost"
            onClick={() => onOpenChange(false)}
            disabled={submitting}
          >
            {t("cancel")}
          </Button>
          <Button type="button" onClick={submit} disabled={submitting}>
            {submitting
              ? t("deliverPrSubmitting")
              : pushBack
                ? t("deliverPrBackSubmit")
                : t("deliverPrSubmit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
