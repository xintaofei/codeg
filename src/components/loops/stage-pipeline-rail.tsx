"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import {
  Check,
  Circle,
  CircleDashed,
  Loader2,
  TriangleAlert,
} from "lucide-react"

import type {
  LoopArtifactKind,
  LoopArtifactRow,
  LoopArtifactStatus,
  LoopIssueRoute,
  LoopIterationRow,
  LoopStage,
} from "@/lib/types"
import { cn } from "@/lib/utils"

export type StageStatus = "done" | "active" | "blocked" | "pending" | "skipped"

/** The loop pipeline in execution order. Always rendered in full so even a fresh
 *  issue (first triage, nothing to graph yet) still shows where it stands. */
export const RAIL_STAGES: LoopStage[] = [
  "triage",
  "refine",
  "design",
  "plan",
  "implement",
  "review",
  "finalize",
  "reflect",
]

// Output-artifact kind each stage lands. `triage` sets the issue route (no
// artifact); `plan`/`implement` both relate to `task`, handled specially below.
const STAGE_KIND: Partial<Record<LoopStage, LoopArtifactKind>> = {
  refine: "requirement",
  design: "design",
  review: "review",
  finalize: "result",
  reflect: "reflection",
}

const isDead = (s: LoopArtifactStatus): boolean =>
  s === "superseded" || s === "cancelled"

/** Which stages a decided route excludes (they never run, so show as skipped). */
function isSkipped(stage: LoopStage, route: LoopIssueRoute): boolean {
  if (stage === "refine") return route === "direct"
  if (stage === "design") return route === "skip_design" || route === "direct"
  return false
}

export interface StageView {
  stage: LoopStage
  status: StageStatus
  /** Count of in-flight (queued|running) iterations for this stage. */
  running: number
}

/**
 * Derive every stage's status from the issue route, its artifacts, and the
 * in-flight iterations. Pure — no rendering — so the precedence is unit-tested.
 *
 * Precedence per stage: active (a live iteration) → skipped (route excludes it) →
 * blocked (existing artifact status, Phase A; no todo aggregation) → done → pending.
 * `done` for `implement` is the task-completion ratio (all tasks terminal), not
 * "a task exists" — and a blocked *task* marks `implement` blocked, never `plan`
 * (plan only produces the tasks). Stages strictly before the furthest one with
 * evidence read as `done` even if their own artifact was superseded.
 */
export function deriveStages(
  route: LoopIssueRoute,
  artifacts: LoopArtifactRow[],
  liveIterations: LoopIterationRow[]
): StageView[] {
  const liveByStage = new Map<LoopStage, number>()
  for (const it of liveIterations) {
    if (it.status !== "queued" && it.status !== "running") continue
    liveByStage.set(it.stage, (liveByStage.get(it.stage) ?? 0) + 1)
  }

  const alive = artifacts.filter((a) => !isDead(a.status))
  const hasKind = (k: LoopArtifactKind) => alive.some((a) => a.kind === k)
  const blockedKind = (k: LoopArtifactKind) =>
    alive.some((a) => a.kind === k && a.status === "blocked")

  const tasks = alive.filter((a) => a.kind === "task")
  const taskTotal = tasks.length
  const taskDone = tasks.filter((t) => t.status === "done").length
  const taskBlocked = tasks.some((t) => t.status === "blocked")
  const taskStarted = tasks.some((t) => t.status !== "pending")

  // Furthest stage with concrete evidence; earlier stages then read as "passed".
  const evidence: Record<LoopStage, boolean> = {
    triage: route !== "undecided",
    refine: hasKind("requirement"),
    design: hasKind("design"),
    plan: taskTotal > 0,
    implement: taskStarted,
    review: hasKind("review"),
    finalize: hasKind("result"),
    reflect: hasKind("reflection"),
  }
  let maxReached = -1
  RAIL_STAGES.forEach((s, i) => {
    if (evidence[s] || liveByStage.has(s)) maxReached = Math.max(maxReached, i)
  })

  const statusOf = (stage: LoopStage, idx: number): StageStatus => {
    if (liveByStage.has(stage)) return "active"
    if (isSkipped(stage, route)) return "skipped"
    if (stage === "triage")
      return route !== "undecided" || idx < maxReached ? "done" : "pending"
    if (stage === "plan")
      return taskTotal > 0 || idx < maxReached ? "done" : "pending"
    if (stage === "implement") {
      if (taskBlocked) return "blocked"
      // Strictly the completion ratio when tasks exist: incomplete tasks stay
      // pending even if a later stage has started (the passed-stage fallback is
      // only for when there are no tasks to judge yet).
      if (taskTotal > 0) return taskDone === taskTotal ? "done" : "pending"
      return idx < maxReached ? "done" : "pending"
    }
    const kind = STAGE_KIND[stage]
    if (kind && blockedKind(kind)) return "blocked"
    if (kind && hasKind(kind)) return "done"
    return idx < maxReached ? "done" : "pending"
  }

  return RAIL_STAGES.map((stage, idx) => ({
    stage,
    status: statusOf(stage, idx),
    running: liveByStage.get(stage) ?? 0,
  }))
}

const STATUS_ICON: Record<StageStatus, typeof Check> = {
  done: Check,
  active: Loader2,
  blocked: TriangleAlert,
  skipped: Circle,
  pending: CircleDashed,
}

// Status not by color alone: each carries a distinct glyph + an accessible label.
const STATUS_STYLE: Record<StageStatus, string> = {
  done: "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-400",
  active: "border-sky-500/40 bg-sky-500/10 text-sky-700 dark:text-sky-400",
  blocked:
    "border-amber-500/50 bg-amber-500/10 text-amber-700 dark:text-amber-400",
  skipped: "border-dashed text-muted-foreground/60",
  pending: "text-muted-foreground",
}

const STATUS_LABEL_KEY = {
  done: "statusDone",
  active: "statusActive",
  blocked: "statusBlocked",
  skipped: "statusSkipped",
  pending: "statusPending",
} as const satisfies Record<StageStatus, string>

/**
 * Always-on 8-stage pipeline rail above the graph/board (spec D4). Unlike the
 * DAG — which has nothing to draw during the very first triage — the rail always
 * shows the issue's position and surfaces a stalled stage (`blocked`) that would
 * otherwise look merely pending.
 */
export function StagePipelineRail({
  route,
  artifacts,
  liveIterations,
}: {
  route: LoopIssueRoute
  artifacts: LoopArtifactRow[]
  liveIterations: LoopIterationRow[]
}) {
  const tStage = useTranslations("Loops.stage")
  const tRail = useTranslations("Loops.stageRail")
  const stages = useMemo(
    () => deriveStages(route, artifacts, liveIterations),
    [route, artifacts, liveIterations]
  )

  return (
    <ul role="list" className="flex items-center gap-1 overflow-x-auto pb-1">
      {stages.map(({ stage, status, running }) => {
        const Icon = STATUS_ICON[status]
        const statusLabel = tRail(STATUS_LABEL_KEY[status])
        const runningLabel =
          status === "active" && running > 0
            ? tRail("runningCount", { n: running })
            : null
        return (
          <li
            key={stage}
            title={runningLabel ?? statusLabel}
            className={cn(
              "inline-flex shrink-0 items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-medium",
              STATUS_STYLE[status]
            )}
          >
            <Icon
              aria-hidden
              className={cn(
                "h-3 w-3 shrink-0",
                status === "active" && "animate-spin"
              )}
            />
            <span className={cn(status === "skipped" && "line-through")}>
              {tStage(stage)}
            </span>
            {status === "active" && running > 0 && (
              <span className="font-mono tabular-nums">{running}</span>
            )}
            {/* Status as text for assistive tech — never color/icon alone. */}
            <span className="sr-only">{statusLabel}</span>
          </li>
        )
      })}
    </ul>
  )
}
