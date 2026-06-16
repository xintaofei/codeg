"use client"

import { useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2 } from "lucide-react"

import {
  approveLoopDesign,
  approveLoopMerge,
  getLoopArtifact,
  getLoopDag,
  getLoopIssue,
  rejectLoopDesign,
  rejectLoopMerge,
} from "@/lib/loops-api"
import { toErrorMessage } from "@/lib/app-error"
import { diffLines, type DiffLine } from "@/lib/line-diff"
import {
  acceptanceOrdinalMap,
  coveringTaskTitles,
  criterionCheckMap,
  taskCovers,
  type CriterionOrdinal,
} from "@/lib/loop-coverage"
import type {
  LoopArtifactDetail,
  LoopCriterionCheckRow,
  LoopGateDecisionRow,
  LoopIssueDetail,
  LoopRevision,
} from "@/lib/types"
import { useLoopResource } from "@/hooks/use-loop-resource"
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
import { MessageResponse } from "@/components/ai-elements/message"

type Gate = "design" | "merge"

/** A per-criterion verdict pill — glyph PLUS the verdict word (never color- or
 * glyph-only) so the trace is accessible. The glyph is decorative (`aria-hidden`);
 * the visible label carries the meaning. */
function CriterionVerdict({
  check,
  label,
}: {
  check: LoopCriterionCheckRow
  label: string
}) {
  const pass = check.verdict === "pass"
  return (
    <span
      className={`inline-flex items-center gap-0.5 text-[10px] font-medium ${
        pass
          ? "text-emerald-600 dark:text-emerald-400"
          : "text-red-600 dark:text-red-400"
      }`}
    >
      <span aria-hidden>{pass ? "✓" : "✗"}</span>
      {label}
    </span>
  )
}

/** Criterion-level coverage view for the drawer (computed from the issue DAG). */
interface CoverageView {
  // Requirement drawer: acceptance criterion id → covering task titles
  // (empty array ⇒ that criterion is uncovered).
  coveredBy: Record<number, string[]>
  // Task drawer: the acceptance criteria this task covers (ordinal + text).
  covers: CriterionOrdinal[]
}

interface ArtifactDrawerData {
  detail: LoopArtifactDetail | null
  // Loaded only for a `result`, to tell a live merge gate (issue running) from
  // an already-merged or blocked one.
  issue: LoopIssueDetail | null
  // Loaded for requirement/task artifacts to render the coverage matrix.
  coverage: CoverageView | null
  // Latest reviewer check per criterion id (the per-criterion verdict glyph in
  // the coverage matrix). Empty for artifacts that show no matrix.
  checks: Map<number, LoopCriterionCheckRow>
  // The gate decision for THIS artifact's own target: a task's review gate or a
  // result's integration (finalize) gate. Null when none recorded yet.
  gateDecision: LoopGateDecisionRow | null
}

const EMPTY_DRAWER: ArtifactDrawerData = {
  detail: null,
  issue: null,
  coverage: null,
  checks: new Map(),
  gateDecision: null,
}

/** The most recent gate decision for a target+stage (highest attempt, then id). */
function latestDecisionFor(
  decisions: LoopGateDecisionRow[],
  targetId: number,
  stage: string
): LoopGateDecisionRow | null {
  return (
    decisions
      .filter((d) => d.target_artifact_id === targetId && d.stage === stage)
      .sort((a, b) => b.attempt - a.attempt || b.id - a.id)[0] ?? null
  )
}

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
 *
 * The body is keyed by `artifactId` so switching artifacts remounts it (fresh
 * skeleton, no stale content or gate flashing the previous artifact); while a
 * given artifact is open the body stays live via the realtime provider, so an
 * engine rework/approval updates it without reopening.
 */
export function ArtifactDrawer({
  artifactId,
  onClose,
}: {
  artifactId: number | null
  onClose: () => void
}) {
  const t = useTranslations("Loops.artifactDrawer")
  return (
    <Sheet open={artifactId != null} onOpenChange={(o) => !o && onClose()}>
      <SheetContent side="right" className="w-full sm:max-w-lg">
        {artifactId != null ? (
          <ArtifactDrawerBody key={artifactId} artifactId={artifactId} />
        ) : (
          // A title must exist for a11y even during the close-out animation,
          // when no artifact body is mounted.
          <SheetHeader>
            <SheetTitle className="truncate">{t("loading")}</SheetTitle>
            <SheetDescription className="sr-only">
              {t("loading")}
            </SheetDescription>
          </SheetHeader>
        )}
      </SheetContent>
    </Sheet>
  )
}

function ArtifactDrawerBody({ artifactId }: { artifactId: number }) {
  const t = useTranslations("Loops.artifactDrawer")
  const tKind = useTranslations("Loops.artifactKind")
  const tStatus = useTranslations("Loops.artifactStatus")
  const tVerdict = useTranslations("Loops.reviewVerdict")
  const tActor = useTranslations("Loops.actorKind")
  const tCriterionKind = useTranslations("Loops.criterionKind")
  const tCoverage = useTranslations("Loops.coverage")
  const tCheckVerdict = useTranslations("Loops.checkVerdict")
  const tGateOutcome = useTranslations("Loops.gateOutcome")
  const tGate = useTranslations("Loops.inbox")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [busy, setBusy] = useState(false)
  const [rejecting, setRejecting] = useState<Gate | null>(null)
  const [comment, setComment] = useState("")

  // The artifact's issue is immutable, so narrow the match the instant we learn
  // it — right after getLoopArtifact, BEFORE the optional getLoopIssue fetch.
  // Broad before the first load (over-fetch, never a miss). A ref keeps the
  // match closure free of an async-data dependency.
  // EXCEPTION to useLoopResource's "never key the match on loaded data" rule:
  // sound ONLY because issue_id is immutable for a given artifact — do not copy
  // this for mutable scope.
  const issueRef = useRef<number | null>(null)
  const { data, loading, refetch } = useLoopResource<ArtifactDrawerData>(
    async () => {
      const detail = await getLoopArtifact(artifactId)
      if (detail) issueRef.current = detail.issue_id // immutable → narrow now
      let issue: LoopIssueDetail | null = null
      let coverage: CoverageView | null = null
      let checks: Map<number, LoopCriterionCheckRow> = new Map()
      let gateDecision: LoopGateDecisionRow | null = null
      if (detail && detail.kind === "result") {
        issue = await getLoopIssue(detail.issue_id).catch(() => null)
      }
      // The coverage matrix, the per-criterion verdict trace, and the gate
      // decision all read the issue DAG; load it for the kinds that surface them.
      if (
        detail &&
        (detail.kind === "requirement" ||
          detail.kind === "task" ||
          detail.kind === "result")
      ) {
        const dag = await getLoopDag(detail.issue_id).catch(() => null)
        if (dag) {
          // Per-criterion verdict (latest check) + this target's gate decision
          // (a task's review gate / a result's integration finalize gate).
          checks = criterionCheckMap(dag.criterion_checks, dag.gate_decisions)
          gateDecision = latestDecisionFor(
            dag.gate_decisions,
            detail.id,
            detail.kind === "result" ? "finalize" : "review"
          )
          if (detail.kind === "requirement") {
            const coveredBy: Record<number, string[]> = {}
            for (const c of detail.criteria) {
              if (c.kind === "acceptance") {
                coveredBy[c.id] = coveringTaskTitles(
                  c.id,
                  dag.coverage,
                  dag.artifacts
                )
              }
            }
            coverage = { coveredBy, covers: [] }
          } else if (detail.kind === "task") {
            // Done requirements only — matches the backend ordinal source, so the
            // R{i}.AC{j} shown here lines up with what `covers` recorded.
            const reqIds = dag.artifacts
              .filter((a) => a.kind === "requirement" && a.status === "done")
              .map((a) => a.id)
            const reqDetails = (
              await Promise.all(
                reqIds.map((id) => getLoopArtifact(id).catch(() => null))
              )
            ).filter((d): d is LoopArtifactDetail => d != null)
            coverage = {
              coveredBy: {},
              covers: taskCovers(
                detail.id,
                dag.coverage,
                acceptanceOrdinalMap(reqDetails)
              ),
            }
          }
        }
      }
      return { detail, issue, coverage, checks, gateDecision }
    },
    {
      match: (e) => issueRef.current == null || e.issue_id === issueRef.current,
      initial: EMPTY_DRAWER,
      deps: [artifactId],
    }
  )
  const detail = data.detail
  const issue = data.issue

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
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      // Reconcile with backend truth after every action (success OR failure), so a
      // stale gate / status converges even when the action was a no-op conflict.
      refetch()
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
    <>
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
          <p className="pt-1 text-sm text-muted-foreground">{t("noContent")}</p>
        ) : (
          <div className="space-y-5 pt-1">
            <Section title={t("contentHeading")}>
              {latest && latest.content.trim().length > 0 ? (
                // Agent/human-authored markdown, rendered through the same
                // safe Streamdown pipeline as chat (no raw HTML, links routed
                // through link-safety) — never raw `dangerouslySetInnerHTML`.
                <div className="break-words text-sm">
                  <MessageResponse>{latest.content}</MessageResponse>
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  {t("noContent")}
                </p>
              )}
            </Section>

            {detail.criteria.length > 0 && (
              <Section title={t("criteriaHeading")}>
                <ul className="space-y-2">
                  {detail.criteria.map((c) => {
                    // Coverage line: only meaningful for a requirement's
                    // acceptance criteria (which tasks claim them).
                    const tasks =
                      detail.kind === "requirement" && c.kind === "acceptance"
                        ? data.coverage?.coveredBy[c.id]
                        : undefined
                    const check =
                      c.kind === "acceptance"
                        ? data.checks.get(c.id)
                        : undefined
                    return (
                      <li key={c.id} className="text-sm">
                        <div className="flex items-center gap-1.5">
                          <Badge variant="outline" className="text-[10px]">
                            {tCriterionKind(c.kind)}
                          </Badge>
                          <span className="font-medium">{c.label}</span>
                          {check && (
                            <CriterionVerdict
                              check={check}
                              label={tCheckVerdict(check.verdict)}
                            />
                          )}
                        </div>
                        {c.text ? (
                          <p className="text-muted-foreground">{c.text}</p>
                        ) : null}
                        {tasks !== undefined &&
                          (tasks.length > 0 ? (
                            <p className="text-xs text-muted-foreground">
                              {tCoverage("coveredBy", {
                                tasks: tasks.join(", "),
                              })}
                            </p>
                          ) : (
                            <p className="text-xs text-amber-600 dark:text-amber-400">
                              {tCoverage("uncovered")}
                            </p>
                          ))}
                      </li>
                    )
                  })}
                </ul>
              </Section>
            )}

            {detail.kind === "task" &&
              data.coverage &&
              data.coverage.covers.length > 0 && (
                <Section title={tCoverage("covers")}>
                  <ul className="space-y-1">
                    {data.coverage.covers.map((c) => (
                      <li key={c.ordinal} className="text-sm">
                        <span className="font-medium">{c.ordinal}</span>
                        <span className="text-muted-foreground">
                          {" — "}
                          {c.text}
                        </span>
                      </li>
                    ))}
                  </ul>
                </Section>
              )}

            {/* Gate decision: the canonical per-criterion outcome the engine
                recorded for this target (a task's review gate / a result's
                integration finalize gate). */}
            {data.gateDecision && (
              <Section title={tCoverage("gateDecision")}>
                <div className="flex items-center gap-2 text-sm">
                  <Badge
                    variant="outline"
                    className={
                      data.gateDecision.outcome === "pass"
                        ? "border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
                        : data.gateDecision.outcome === "fail"
                          ? "border-red-500/40 text-red-600 dark:text-red-400"
                          : "text-muted-foreground"
                    }
                  >
                    {tGateOutcome(data.gateDecision.outcome)}
                  </Badge>
                  <span className="text-muted-foreground">
                    {tCoverage("aggregatedChecks", {
                      count: data.gateDecision.input_check_ids.length,
                    })}
                  </span>
                </div>
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
            <Button size="sm" className="h-8" disabled={busy} onClick={approve}>
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
    </>
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
              {line.text || " "}
            </div>
          ))}
        </pre>
      )}
    </div>
  )
}
