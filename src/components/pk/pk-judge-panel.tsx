"use client"

import { useTranslations } from "next-intl"
import { AgentIcon } from "@/components/agent-icon"
import { getAgentLabel } from "@/lib/custom-agents"
import { assignJudgeScoreSlots, contestantForJudgeScore } from "@/lib/pk-judge"
import { cn } from "@/lib/utils"
import type {
  PkContestant,
  PkJudgeResult,
  PkJudgeStatus,
} from "@/stores/pk-arena-store"
import type { AgentType } from "@/lib/types"

/**
 * Judge verdict panel — appears below the scoreboard when a judge agent was
 * configured for the round. Shows the judge's structured scores and rankings
 * once the judge has finished, or a spinner while it runs.
 */

function scoreColor(score: number): string {
  if (score >= 80) return "text-emerald-600 dark:text-emerald-400"
  if (score >= 60) return "text-amber-600 dark:text-amber-400"
  if (score >= 40) return "text-orange-600 dark:text-orange-400"
  return "text-red-600 dark:text-red-400"
}

function rankBadge(rank: number): string {
  if (rank === 1) return "🥇"
  if (rank === 2) return "🥈"
  if (rank === 3) return "🥉"
  return `#${rank}`
}

export function PkJudgePanel({
  judgeStatus,
  judgeResult,
  judgeAgent,
  contestants,
  onRerun,
}: {
  judgeStatus: PkJudgeStatus
  judgeResult: PkJudgeResult | null
  judgeAgent: string
  contestants: readonly PkContestant[]
  onRerun?: () => void
}) {
  const t = useTranslations("PkArena.judge")
  const completedContestants = contestants.filter(
    (contestant) => contestant.status === "done"
  )
  const scores = assignJudgeScoreSlots(
    judgeResult?.scores ?? [],
    completedContestants
  )

  if (judgeStatus === "idle" || judgeStatus === "skipped") return null

  return (
    <div className="border-b border-border bg-muted/30 px-4 py-3">
      <div className="mb-2 flex items-center gap-2">
        <AgentIcon agentType={judgeAgent as AgentType} className="size-4" />
        <span className="text-sm font-medium text-foreground">
          {t("title")}
        </span>
        <span className="text-xs text-muted-foreground">
          {getAgentLabel(judgeAgent as AgentType)}
        </span>
        {judgeStatus === "running" ? (
          <span className="flex items-center gap-1 text-xs text-muted-foreground">
            <span className="size-1.5 animate-pulse rounded-full bg-sky-500" />
            {t("running")}
          </span>
        ) : null}
        {judgeStatus === "error" ? (
          <span className="text-xs text-red-600 dark:text-red-400">
            {t("error")}
          </span>
        ) : null}
        {onRerun ? (
          <button
            type="button"
            onClick={onRerun}
            className="ml-auto inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          >
            {t("rerun")}
          </button>
        ) : null}
      </div>

      {judgeResult ? (
        <div className="space-y-2">
          {scores.length > 0 ? (
            <div className="flex flex-wrap gap-2">
              {scores
                .slice()
                .sort((a, b) => a.rank - b.rank)
                .map((score, index) => {
                  const contestant = contestantForJudgeScore(score, contestants)
                  return (
                    <div
                      key={
                        score.slot ??
                        `${score.agentType}-${score.rank}-${index}`
                      }
                      className="flex items-center gap-2 rounded-lg border border-border bg-background px-3 py-1.5"
                    >
                      <span className="text-sm">{rankBadge(score.rank)}</span>
                      <AgentIcon
                        agentType={score.agentType as AgentType}
                        className="size-4"
                      />
                      <span className="text-sm font-medium text-foreground">
                        {getAgentLabel(score.agentType as AgentType)}
                      </span>
                      {contestant?.label ? (
                        <span className="max-w-40 truncate text-xs text-muted-foreground">
                          {contestant.label}
                        </span>
                      ) : null}
                      <span
                        className={cn(
                          "text-lg font-bold tabular-nums",
                          scoreColor(score.score)
                        )}
                      >
                        {score.score}
                      </span>
                      {score.comment ? (
                        <span className="max-w-xs truncate text-xs text-muted-foreground">
                          {score.comment}
                        </span>
                      ) : null}
                    </div>
                  )
                })}
            </div>
          ) : null}
          {judgeResult.summary ? (
            <p className="text-xs text-muted-foreground">
              {judgeResult.summary}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
