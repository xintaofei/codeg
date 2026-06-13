"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

import {
  approveLoopDesign,
  approveLoopMerge,
  cancelLoopIssue,
  listLoopInbox,
  rejectLoopDesign,
  rejectLoopMerge,
} from "@/lib/loops-api"
import type { LoopInboxItemRow } from "@/lib/types"
import { toErrorMessage } from "@/lib/app-error"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

type Gate = "design" | "merge"

function payloadObj(p: unknown): Record<string, unknown> {
  return p && typeof p === "object" ? (p as Record<string, unknown>) : {}
}

/** The approval gate an `approval` card refers to, from its payload. */
function gateOf(item: LoopInboxItemRow): Gate | null {
  const g = payloadObj(item.payload).gate
  return g === "design" || g === "merge" ? g : null
}

/**
 * Minimal inbox (M2.2): the blocking items a person resolves — design / merge
 * approvals (approve / reject-with-comment) plus blocked / budget cards (cancel
 * the issue). Full two-pane inbox with retry / add-budget / question routing is
 * M2.3.
 */
export function InboxPanel({ spaceId }: { spaceId: number }) {
  const t = useTranslations("Loops.inbox")
  const tToasts = useTranslations("Loops.toasts")
  const tCommon = useTranslations("Loops.common")

  const [items, setItems] = useState<LoopInboxItemRow[]>([])
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [rejecting, setRejecting] = useState<LoopInboxItemRow | null>(null)
  const [comment, setComment] = useState("")

  const refresh = useCallback(async () => {
    try {
      const list = await listLoopInbox(spaceId, "pending")
      setItems(list)
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

  const describe = (
    item: LoopInboxItemRow
  ): { label: string; desc: string } => {
    const p = payloadObj(item.payload)
    if (item.kind === "approval") {
      return gateOf(item) === "merge"
        ? { label: t("gateMerge"), desc: t("mergeDesc") }
        : { label: t("gateDesign"), desc: t("designDesc") }
    }
    if (item.kind === "blocked") {
      const reason =
        (typeof p.reason === "string" && p.reason) ||
        (typeof p.detail === "string" && p.detail) ||
        ""
      return { label: t("kindBlocked"), desc: reason }
    }
    if (item.kind === "budget_exhausted") {
      return {
        label: t("kindBudget"),
        desc: t("budgetDesc", {
          used: String(p.token_used ?? "?"),
          budget: String(p.token_budget ?? "?"),
        }),
      }
    }
    return { label: t("kindQuestion"), desc: t("questionDesc") }
  }

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
          <ul className="space-y-2">
            {items.map((item) => {
              const { label, desc } = describe(item)
              const busy = busyId === item.id
              const isApproval = item.kind === "approval"
              const isStoppable =
                item.kind === "blocked" || item.kind === "budget_exhausted"
              return (
                <li key={item.id} className="rounded-md border p-3">
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-xs text-muted-foreground">
                      {t("issueLabel", { seq: item.issue_seq })}
                    </span>
                    <span className="text-sm font-medium">{label}</span>
                  </div>
                  {desc && (
                    <p className="mt-1 whitespace-pre-wrap break-words text-xs text-muted-foreground">
                      {desc}
                    </p>
                  )}
                  <div className="mt-2 flex flex-wrap gap-2">
                    {isApproval && (
                      <>
                        <Button
                          size="sm"
                          className="h-7"
                          disabled={busy}
                          onClick={() => approve(item)}
                        >
                          {busy && (
                            <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                          )}
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
                    )}
                    {isStoppable && (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-7 text-destructive"
                        disabled={busy}
                        onClick={() =>
                          void run(item.id, () =>
                            cancelLoopIssue(item.issue_id)
                          )
                        }
                      >
                        {busy && (
                          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                        )}
                        {t("cancelIssue")}
                      </Button>
                    )}
                  </div>
                </li>
              )
            })}
          </ul>
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
              onClick={() => {
                setRejecting(null)
                setComment("")
              }}
              type="button"
            >
              {tCommon("cancel")}
            </Button>
            <Button onClick={confirmReject} type="button">
              {t("submitReject")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
