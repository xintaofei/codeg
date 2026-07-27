"use client"

import { useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { ListTodoIcon, Loader2 } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { MessageResponse } from "@/components/ai-elements/message"
import type {
  PendingPlanApprovalState,
  PlanApprovalAnswer,
  PlanApprovalDecision,
} from "@/lib/types"

interface PlanApprovalCardProps {
  /** The awaiting-decision plan approval. The shell renders this card only when
   *  an approval is pending, so the prop is always present. */
  approval: PendingPlanApprovalState
  /** Resolves the parked `exit_plan_mode` ext request. Returns a promise so the
   *  card can hold an in-flight state and surface a retryable error on failure. */
  onAnswer: (
    approvalId: string,
    answer: PlanApprovalAnswer
  ) => void | Promise<void>
}

/**
 * Grok plan-mode approval card. When the agent finishes planning it calls
 * `exit_plan_mode` and BLOCKS on the user's decision; this renders the plan above
 * the composer with Approve / Request-changes / Abandon actions (mirroring Grok's
 * own TUI approval bar). "Request changes" reveals a textarea for freeform
 * revision notes. Modeled on `AskQuestionCard`'s in-flight/disable pattern: on
 * success the backend's `plan_approval_resolved` clears `pendingPlanApproval` and
 * unmounts this card, so controls stay disabled rather than flashing back on.
 */
export function PlanApprovalCard({
  approval,
  onAnswer,
}: PlanApprovalCardProps) {
  const t = useTranslations("Folder.chat.planApproval")
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState(false)
  const [changesOpen, setChangesOpen] = useState(false)
  const [feedback, setFeedback] = useState("")
  const inFlight = useRef(false)

  const plan = approval.plan_markdown.trim()

  // Run a decision round-trip, holding the card in an in-flight state until it
  // resolves. On failure re-enable and surface a retryable error.
  const run = async (answer: PlanApprovalAnswer) => {
    if (inFlight.current) return
    inFlight.current = true
    setSubmitting(true)
    setError(false)
    try {
      await onAnswer(approval.approval_id, answer)
      inFlight.current = false
    } catch {
      setError(true)
      setSubmitting(false)
      inFlight.current = false
    }
  }

  const decide = (decision: PlanApprovalDecision, fb?: string) =>
    void run({ decision, feedback: fb ?? null })

  const locked = submitting

  return (
    <div className="w-full space-y-3 rounded-lg border border-border/60 bg-card/60 p-4">
      <div className="flex items-center gap-1.5 text-sm font-medium text-foreground">
        <ListTodoIcon className="size-4 shrink-0 text-muted-foreground" />
        {t("title")}
      </div>

      {plan ? (
        <div className="prose prose-sm max-h-80 max-w-none overflow-auto rounded-md border border-border/60 bg-muted/30 px-3.5 py-3 text-sm dark:prose-invert [&_ol]:list-inside [&_ul]:list-inside">
          <MessageResponse>{plan}</MessageResponse>
        </div>
      ) : (
        <p className="rounded-md border border-dashed border-border/60 px-3 py-4 text-sm text-muted-foreground">
          {t("emptyPlan")}
        </p>
      )}

      {changesOpen ? (
        <div className="space-y-2">
          <Textarea
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder={t("feedbackPlaceholder")}
            disabled={locked}
            rows={3}
            autoFocus
          />
          <div className="flex items-center justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              disabled={locked}
              onClick={() => setChangesOpen(false)}
            >
              {t("cancel")}
            </Button>
            <Button
              size="sm"
              disabled={locked || !feedback.trim()}
              onClick={() => decide("request_changes", feedback.trim())}
            >
              {submitting && <Loader2 className="size-3.5 animate-spin" />}
              {t("sendChanges")}
            </Button>
          </div>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" disabled={locked} onClick={() => decide("approve")}>
            {submitting && <Loader2 className="size-3.5 animate-spin" />}
            {t("approve")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={locked}
            onClick={() => setChangesOpen(true)}
          >
            {t("requestChanges")}
          </Button>
          <Button
            variant="destructive"
            size="sm"
            disabled={locked}
            onClick={() => decide("abandon")}
          >
            {t("abandon")}
          </Button>
        </div>
      )}

      {error && <p className="text-xs text-destructive">{t("submitError")}</p>}
    </div>
  )
}
