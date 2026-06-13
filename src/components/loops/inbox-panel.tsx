"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

import {
  addLoopIssueBudget,
  approveLoopDesign,
  approveLoopMerge,
  cancelLoopIssue,
  listLoopInbox,
  rejectLoopDesign,
  rejectLoopMerge,
  retryLoopIssue,
} from "@/lib/loops-api"
import type { LoopInboxItemRow } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Textarea } from "@/components/ui/textarea"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

type Gate = "design" | "merge"

/** Kinds that block the loop and need a person to clear them — the first pane. */
const BLOCKING_KINDS = new Set(["approval", "blocked", "budget_exhausted"])

function payloadObj(p: unknown): Record<string, unknown> {
  return p && typeof p === "object" ? (p as Record<string, unknown>) : {}
}

/** The approval gate an `approval` card refers to, from its payload. */
function gateOf(item: LoopInboxItemRow): Gate | null {
  const g = payloadObj(item.payload).gate
  return g === "design" || g === "merge" ? g : null
}

/** The questions an agent is asking, best-effort from the card payload. */
function questionText(item: LoopInboxItemRow): string {
  const p = payloadObj(item.payload)
  if (typeof p.prompt === "string" && p.prompt.trim()) return p.prompt
  if (typeof p.question === "string" && p.question.trim()) return p.question
  if (Array.isArray(p.questions)) {
    const lines = p.questions
      .map((q) =>
        q &&
        typeof q === "object" &&
        typeof (q as { question?: unknown }).question === "string"
          ? (q as { question: string }).question
          : ""
      )
      .filter(Boolean)
    if (lines.length) return lines.join("\n")
  }
  return ""
}

/** Stable identity for dedupe — mirrors the backend's pending-uniqueness key. */
function identity(item: LoopInboxItemRow): string {
  return `${item.issue_id}:${item.kind}:${item.subject_key}`
}

interface IssueGroup {
  issueId: number
  issueSeq: number
  items: LoopInboxItemRow[]
}

/** Group items by issue, ordered by ascending issue seq. */
function groupByIssue(items: LoopInboxItemRow[]): IssueGroup[] {
  const groups = new Map<number, IssueGroup>()
  for (const item of items) {
    let g = groups.get(item.issue_id)
    if (!g) {
      g = { issueId: item.issue_id, issueSeq: item.issue_seq, items: [] }
      groups.set(item.issue_id, g)
    }
    g.items.push(item)
  }
  return [...groups.values()].sort((a, b) => a.issueSeq - b.issueSeq)
}

/**
 * The space inbox: two panes — **blocking** items a person clears (design/merge
 * approvals, blocked tasks, exhausted budgets) and **questions** an agent is
 * asking. Items are grouped by issue and deduped. Question routing (opening the
 * iteration to answer) is provided by the host via `onOpenQuestion` (M2.3 Task
 * 3.2 wires the iteration dialog).
 */
export function InboxPanel({
  spaceId,
  onOpenQuestion,
}: {
  spaceId: number
  onOpenQuestion?: (item: LoopInboxItemRow) => void
}) {
  const t = useTranslations("Loops.inbox")
  const tToasts = useTranslations("Loops.toasts")
  const tCommon = useTranslations("Loops.common")

  const [items, setItems] = useState<LoopInboxItemRow[]>([])
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [rejecting, setRejecting] = useState<LoopInboxItemRow | null>(null)
  const [comment, setComment] = useState("")
  const [budgeting, setBudgeting] = useState<LoopInboxItemRow | null>(null)
  const [budgetAmount, setBudgetAmount] = useState("")

  const refresh = useCallback(async () => {
    try {
      setItems(await listLoopInbox(spaceId, "pending"))
    } catch {
      // listing failures are non-fatal here; the empty state covers it
    } finally {
      setLoading(false)
    }
  }, [spaceId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  }, spaceId)

  // Split into the two panes, deduping defensively (the backend's partial unique
  // index already forbids two pending cards with the same issue/kind/subject).
  const { blocking, questions } = useMemo(() => {
    const seen = new Set<string>()
    const blocking: LoopInboxItemRow[] = []
    const questions: LoopInboxItemRow[] = []
    for (const item of items) {
      const id = identity(item)
      if (seen.has(id)) continue
      seen.add(id)
      if (BLOCKING_KINDS.has(item.kind)) blocking.push(item)
      else questions.push(item)
    }
    return { blocking, questions }
  }, [items])

  const run = async (id: number, fn: () => Promise<void>) => {
    setBusyId(id)
    try {
      await fn()
      toast.success(tToasts("inboxResolved"))
      await refresh()
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      setBusyId(null)
    }
  }

  const approve = (item: LoopInboxItemRow) => {
    const gate = gateOf(item)
    if (gate === "design")
      void run(item.id, () => approveLoopDesign(item.issue_id))
    else if (gate === "merge")
      void run(item.id, () => approveLoopMerge(item.issue_id))
  }

  const confirmReject = () => {
    if (!rejecting) return
    const item = rejecting
    const gate = gateOf(item)
    const text = comment.trim() || undefined
    setRejecting(null)
    setComment("")
    if (gate === "design")
      void run(item.id, () => rejectLoopDesign(item.issue_id, text))
    else if (gate === "merge")
      void run(item.id, () => rejectLoopMerge(item.issue_id, text))
  }

  const confirmAddBudget = () => {
    if (!budgeting) return
    const item = budgeting
    const amount = Math.floor(Number(budgetAmount))
    setBudgeting(null)
    setBudgetAmount("")
    if (Number.isFinite(amount) && amount > 0)
      void run(item.id, () => addLoopIssueBudget(item.issue_id, amount))
  }

  const describe = (
    item: LoopInboxItemRow
  ): { label: string; desc: string } => {
    const p = payloadObj(item.payload)
    switch (item.kind) {
      case "approval":
        return gateOf(item) === "merge"
          ? { label: t("gateMerge"), desc: t("mergeDesc") }
          : { label: t("gateDesign"), desc: t("designDesc") }
      case "blocked": {
        const reason =
          (typeof p.reason === "string" && p.reason) ||
          (typeof p.detail === "string" && p.detail) ||
          ""
        return { label: t("kindBlocked"), desc: reason }
      }
      case "budget_exhausted":
        return {
          label: t("kindBudget"),
          desc: t("budgetDesc", {
            used: String(p.token_used ?? "?"),
            budget: String(p.token_budget ?? "?"),
          }),
        }
      case "question": {
        const q = questionText(item)
        return { label: t("kindQuestion"), desc: q || t("questionDesc") }
      }
      default:
        return { label: item.kind, desc: "" }
    }
  }

  const actions = (item: LoopInboxItemRow) => {
    const busy = busyId === item.id
    const spin = busy ? (
      <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
    ) : null
    const stop = (
      <Button
        size="sm"
        variant="outline"
        className="h-7 text-destructive"
        disabled={busy}
        onClick={() => void run(item.id, () => cancelLoopIssue(item.issue_id))}
      >
        {t("stop")}
      </Button>
    )
    switch (item.kind) {
      case "approval":
        return (
          <>
            <Button
              size="sm"
              className="h-7"
              disabled={busy}
              onClick={() => approve(item)}
            >
              {spin}
              {gateOf(item) === "merge" ? t("merge") : t("approve")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              className="h-7"
              disabled={busy}
              onClick={() => {
                setComment("")
                setRejecting(item)
              }}
            >
              {t("reject")}
            </Button>
          </>
        )
      case "blocked":
        return (
          <>
            <Button
              size="sm"
              className="h-7"
              disabled={busy}
              onClick={() =>
                void run(item.id, () => retryLoopIssue(item.issue_id))
              }
            >
              {spin}
              {t("retry")}
            </Button>
            {stop}
          </>
        )
      case "budget_exhausted":
        return (
          <>
            <Button
              size="sm"
              className="h-7"
              disabled={busy}
              onClick={() => {
                setBudgetAmount("")
                setBudgeting(item)
              }}
            >
              {spin}
              {t("addBudget")}
            </Button>
            {stop}
          </>
        )
      case "question":
        return onOpenQuestion ? (
          <Button
            size="sm"
            variant="outline"
            className="h-7"
            onClick={() => onOpenQuestion(item)}
          >
            {t("openConversation")}
          </Button>
        ) : null
      default:
        return null
    }
  }

  const renderCard = (item: LoopInboxItemRow) => {
    const { label, desc } = describe(item)
    return (
      <li key={item.id} className="rounded-md border p-3">
        <div className="text-sm font-medium">{label}</div>
        {desc && (
          <p className="mt-1 whitespace-pre-wrap break-words text-xs text-muted-foreground">
            {desc}
          </p>
        )}
        <div className="mt-2 flex flex-wrap gap-2">{actions(item)}</div>
      </li>
    )
  }

  const renderSection = (title: string, list: LoopInboxItemRow[]) => (
    <div className="space-y-2">
      <h3 className="px-1 text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      {list.length === 0 ? (
        <p className="px-1 text-xs text-muted-foreground/70">
          {t("sectionEmpty")}
        </p>
      ) : (
        groupByIssue(list).map((group) => (
          <div key={group.issueId} className="space-y-1.5">
            <div className="px-1 font-mono text-[11px] text-muted-foreground">
              {t("issueLabel", { seq: group.issueSeq })}
            </div>
            <ul className="space-y-2">{group.items.map(renderCard)}</ul>
          </div>
        ))
      )}
    </div>
  )

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {loading ? (
          <div className="flex h-24 items-center justify-center text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : items.length === 0 ? (
          <p className="px-3 py-10 text-center text-sm text-muted-foreground">
            {t("empty")}
          </p>
        ) : (
          <div className="space-y-5">
            {renderSection(t("sectionBlocking"), blocking)}
            {renderSection(t("sectionQuestions"), questions)}
          </div>
        )}
      </div>

      <Dialog
        open={rejecting != null}
        onOpenChange={(o) => {
          if (!o) {
            setRejecting(null)
            setComment("")
          }
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("rejectTitle")}</DialogTitle>
          </DialogHeader>
          <Textarea
            value={comment}
            onChange={(e) => setComment(e.target.value)}
            placeholder={t("rejectPlaceholder")}
            rows={4}
            autoFocus
          />
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={() => {
                setRejecting(null)
                setComment("")
              }}
            >
              {tCommon("cancel")}
            </Button>
            <Button type="button" onClick={confirmReject}>
              {t("submitReject")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={budgeting != null}
        onOpenChange={(o) => {
          if (!o) {
            setBudgeting(null)
            setBudgetAmount("")
          }
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("addBudgetTitle")}</DialogTitle>
          </DialogHeader>
          <Input
            type="number"
            min={1}
            value={budgetAmount}
            onChange={(e) => setBudgetAmount(e.target.value)}
            placeholder={t("addBudgetPlaceholder")}
            autoFocus
          />
          <DialogFooter>
            <Button
              variant="outline"
              type="button"
              onClick={() => {
                setBudgeting(null)
                setBudgetAmount("")
              }}
            >
              {tCommon("cancel")}
            </Button>
            <Button type="button" onClick={confirmAddBudget}>
              {t("submitAddBudget")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
