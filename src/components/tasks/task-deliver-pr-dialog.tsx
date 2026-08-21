"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { workTaskDeliverPr } from "@/lib/api"
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
import { mustDeliverToPr, usesMergeRequests } from "./task-acceptance"
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
 *   decisions, and they are the only fields here;
 * - a PULL REQUEST's task pushes back onto that pull request's own branch.
 *   Nothing is created, so there is nothing to name: the dialog just says
 *   where the commits are about to go and asks for a confirmation.
 *
 * Everything else — which repository, which account, which base branch, the
 * body with its closing keyword — is derived by the backend from what the task
 * was created with, so nothing typed in this dialog can point the push
 * somewhere else.
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
  const [submitting, setSubmitting] = useState(false)

  useEffect(() => {
    if (!open || !task) return
    /* eslint-disable react-hooks/set-state-in-effect */
    setSubmitting(false)
    setDraft(false)
    setTitle(task.title)
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
      // A push-back creates nothing, so it carries neither title nor draft.
      const url = pushBack
        ? await workTaskDeliverPr(task.id, null, false)
        : await workTaskDeliverPr(task.id, title.trim() || null, draft)
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

        {pushBack ? null : (
          <div className="flex flex-col gap-3">
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
