"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

import {
  approveLoopDesign,
  approveLoopMerge,
  getLoopArtifact,
  getLoopIssue,
  rejectLoopDesign,
  rejectLoopMerge,
} from "@/lib/loops-api"
import { toErrorMessage } from "@/lib/app-error"
import { diffLines, type DiffLine } from "@/lib/line-diff"
import type {
  LoopArtifactDetail,
  LoopIssueDetail,
  LoopRevision,
} from "@/lib/types"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Skeleton } from "@/components/ui/skeleton"
import { Textarea } from "@/components/ui/textarea"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

type Gate = "design" | "merge"

/**
 * Read-only drawer for a single artifact, plus the two human gates the loop
 * routes through it:
 *
 * - **content** — the latest revision's text;
 * - **revision history** — each adjacent revision rendered as a colored
 *   line diff (so a human can see what a rework or a rejection note changed);
 * - **acceptance criteria** + (for a `review` artifact) its **verdict** and
 *   per-criterion findings;
 * - **linked iteration** — which iteration produced the artifact;
 * - **gates** — a `design` awaiting approval shows approve / reject (with an
 *   optional comment); a `result` whose issue is still running shows merge /
 *   reject. No other manual status controls — the engine owns every other
 *   transition.
 */
export function ArtifactDrawer({
  artifactId,
  onClose,
}: {
  artifactId: number | null
  onClose: () => void
}) {
  const t = useTranslations("Loops.artifactDrawer")
  const tKind = useTranslations("Loops.artifactKind")
  const tStatus = useTranslations("Loops.artifactStatus")
  const tVerdict = useTranslations("Loops.reviewVerdict")
  const tActor = useTranslations("Loops.actorKind")
  const tGate = useTranslations("Loops.inbox")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [detail, setDetail] = useState<LoopArtifactDetail | null>(null)
  // Loaded only for a `result`, to tell a live merge gate (issue running) from an
  // already-merged or blocked one.
  const [issue, setIssue] = useState<LoopIssueDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [rejecting, setRejecting] = useState<Gate | null>(null)
  const [comment, setComment] = useState("")
  // Monotonic request id: a slower earlier fetch must not overwrite a newer one.
  const reqRef = useRef(0)

  const load = useCallback(async (id: number) => {
    const req = ++reqRef.current
    setLoading(true)
    setDetail(null)
    setIssue(null)
    try {
      const d = await getLoopArtifact(id)
      if (reqRef.current !== req) return
      setDetail(d)
      if (d && d.kind === "result") {
        const iss = await getLoopIssue(d.issue_id).catch(() => null)
        if (reqRef.current === req) setIssue(iss)
      }
    } finally {
      if (reqRef.current === req) setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (artifactId != null) void load(artifactId)
  }, [artifactId, load])

  // Newest revision first; the latest drives the content section.
  const revisions: LoopRevision[] = detail
    ? [...detail.revisions].sort((a, b) => b.seq - a.seq)
    : []
  const latest = revisions[0]

  const designGate =
    detail?.kind === "design" && detail.status === "awaiting_approval"
  const mergeGate = detail?.kind === "result" && issue?.status === "running"

  const run = async (fn: () => Promise<void>) => {
    setBusy(true)
    try {
      await fn()
      toast.success(tToasts("inboxResolved"))
      if (artifactId != null) await load(artifactId)
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      setBusy(false)
    }
  }

  const approve = () => {
    if (!detail) return
    if (designGate) void run(() => approveLoopDesign(detail.issue_id))
    else if (mergeGate) void run(() => approveLoopMerge(detail.issue_id))
  }

  const confirmReject = () => {
    if (!detail) return
    const gate = rejecting
    const text = comment.trim() || undefined
    setRejecting(null)
    setComment("")
    if (gate === "design")
      void run(() => rejectLoopDesign(detail.issue_id, text))
    else if (gate === "merge")
      void run(() => rejectLoopMerge(detail.issue_id, text))
  }

  return (
    <Sheet open={artifactId != null} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full sm:max-w-lg">
        <SheetHeader>
          <SheetTitle className="truncate">
            {detail?.title ?? t("loading")}
          </SheetTitle>
          <SheetDescription asChild>
            <div className="flex flex-wrap items-center gap-1.5">
              {detail && (
                <>
                  <Badge variant="outline">{tKind(detail.kind)}</Badge>
                  <Badge variant="secondary">{tStatus(detail.status)}</Badge>
                  {detail.kind === "review" && detail.verdict && (
                    <Badge
                      variant="outline"
                      className={
                        detail.verdict === "pass"
                          ? "border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
                          : "border-red-500/40 text-red-600 dark:text-red-400"
                      }
                    >
                      {tVerdict(detail.verdict)}
                    </Badge>
                  )}
                  {detail.revisions.length > 0 && (
                    <span className="text-xs text-muted-foreground">
                      {t("revisionCount", { count: detail.revisions.length })}
                    </span>
                  )}
                </>
              )}
            </div>
          </SheetDescription>
        </SheetHeader>

        <ScrollArea className="min-h-0 flex-1 px-4 pb-4">
          {loading ? (
            <div className="space-y-2 pt-1">
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-2/3" />
            </div>
          ) : !detail ? (
            <p className="pt-1 text-sm text-muted-foreground">
              {t("noContent")}
            </p>
          ) : (
            <div className="space-y-5 pt-1">
              <Section title={t("contentHeading")}>
                {latest && latest.content.trim().length > 0 ? (
                  <p className="whitespace-pre-wrap break-words text-sm">
                    {latest.content}
                  </p>
                ) : (
                  <p className="text-sm text-muted-foreground">
                    {t("noContent")}
                  </p>
                )}
              </Section>

              {detail.criteria.length > 0 && (
                <Section title={t("criteriaHeading")}>
                  <ul className="space-y-1.5">
                    {detail.criteria.map((c) => (
                      <li key={c.id} className="text-sm">
                        <span className="font-medium">{c.label}</span>
                        {c.text ? (
                          <span className="text-muted-foreground">
                            {" — "}
                            {c.text}
                          </span>
                        ) : null}
                      </li>
                    ))}
                  </ul>
                </Section>
              )}

              {revisions.length > 1 && (
                <Section title={t("revisionsHeading")}>
                  <div className="space-y-3">
                    {revisions.slice(0, -1).map((rev, i) => {
                      const prev = revisions[i + 1]
                      return (
                        <RevisionDiff
                          key={rev.id}
                          label={t("revisionLabel", { seq: rev.seq })}
                          actor={tActor(rev.actor_kind)}
                          lines={diffLines(prev.content, rev.content)}
                          emptyLabel={t("noChanges")}
                        />
                      )
                    })}
                  </div>
                </Section>
              )}

              {detail.produced_by_iteration_id != null && (
                <Section title={t("linkedHeading")}>
                  <p className="text-sm text-muted-foreground">
                    {t("producedBy", {
                      id: detail.produced_by_iteration_id,
                    })}
                  </p>
                </Section>
              )}
            </div>
          )}
        </ScrollArea>

        {(designGate || mergeGate) && (
          <div className="shrink-0 border-t px-4 py-3">
            <p className="mb-2 text-xs text-muted-foreground">
              {designGate ? t("gateDesignPrompt") : t("gateMergePrompt")}
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                size="sm"
                className="h-8"
                disabled={busy}
                onClick={approve}
              >
                {busy && <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />}
                {designGate ? tGate("approve") : tGate("merge")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="h-8"
                disabled={busy}
                onClick={() => {
                  setComment("")
                  setRejecting(designGate ? "design" : "merge")
                }}
              >
                {tGate("reject")}
              </Button>
            </div>
          </div>
        )}

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
              <DialogTitle>{tGate("rejectTitle")}</DialogTitle>
            </DialogHeader>
            <Textarea
              value={comment}
              onChange={(e) => setComment(e.target.value)}
              placeholder={tGate("rejectPlaceholder")}
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
                {tGate("submitReject")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </SheetContent>
    </Sheet>
  )
}

function Section({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <section className="space-y-1.5">
      <h3 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {title}
      </h3>
      {children}
    </section>
  )
}

/** One revision's change set, as a colored monospace line diff. */
function RevisionDiff({
  label,
  actor,
  lines,
  emptyLabel,
}: {
  label: string
  actor: string
  lines: DiffLine[]
  emptyLabel: string
}) {
  return (
    <div className="overflow-hidden rounded-md border">
      <div className="flex items-center justify-between border-b bg-muted/40 px-2.5 py-1 text-[11px] text-muted-foreground">
        <span className="font-medium">{label}</span>
        <span>{actor}</span>
      </div>
      {lines.length === 0 ? (
        <p className="px-2.5 py-1.5 text-xs text-muted-foreground">
          {emptyLabel}
        </p>
      ) : (
        <pre className="overflow-x-auto px-2.5 py-1.5 font-mono text-xs leading-relaxed">
          {lines.map((line, i) => (
            <div
              key={i}
              className={
                line.type === "add"
                  ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                  : line.type === "del"
                    ? "bg-red-500/10 text-red-700 dark:text-red-300"
                    : "text-muted-foreground"
              }
            >
              <span className="select-none opacity-60">
                {line.type === "add" ? "+ " : line.type === "del" ? "- " : "  "}
              </span>
              {line.text || " "}
            </div>
          ))}
        </pre>
      )}
    </div>
  )
}
