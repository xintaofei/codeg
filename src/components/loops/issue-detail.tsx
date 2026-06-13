"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, Play, Settings2 } from "lucide-react"

import { getLoopDag, getLoopIssue } from "@/lib/loops-api"
import type { LoopArtifactRow, LoopIssueDetail } from "@/lib/types"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import {
  IssuePriorityBadge,
  IssueRouteBadge,
  IssueStatusBadge,
} from "@/components/loops/issue-badges"

export function IssueDetail({ issueId }: { issueId: number | null }) {
  const t = useTranslations("Loops.issueDetail")
  const tList = useTranslations("Loops.issueList")

  const [issue, setIssue] = useState<LoopIssueDetail | null>(null)
  const [artifacts, setArtifacts] = useState<LoopArtifactRow[]>([])
  const [loading, setLoading] = useState(false)

  const refresh = useCallback(async () => {
    if (issueId == null) {
      setIssue(null)
      setArtifacts([])
      return
    }
    setLoading(true)
    try {
      const [detail, dag] = await Promise.all([
        getLoopIssue(issueId),
        getLoopDag(issueId),
      ])
      setIssue(detail)
      setArtifacts(dag.artifacts)
    } finally {
      setLoading(false)
    }
  }, [issueId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  }, issue?.space_id)

  if (issueId == null) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center text-sm text-muted-foreground">
        {t("selectPrompt")}
      </div>
    )
  }

  if (loading && !issue) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        <Loader2 className="h-5 w-5 animate-spin" />
      </div>
    )
  }

  if (!issue) return null

  const budget = issue.token_budget
  const tokenText =
    budget != null
      ? t("tokenWithBudget", {
          used: issue.token_used.toLocaleString(),
          budget: budget.toLocaleString(),
        })
      : issue.token_used.toLocaleString()

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {/* Row ① — title + token usage + actions */}
      <div className="flex shrink-0 items-start justify-between gap-3 px-5 pt-4 pb-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="shrink-0 font-mono text-xs text-muted-foreground">
              #{issue.seq_no}
            </span>
            <h2 className="truncate text-base font-semibold">{issue.title}</h2>
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            <IssueStatusBadge status={issue.status} />
            <IssuePriorityBadge priority={issue.priority} />
            <IssueRouteBadge route={issue.route} />
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <div className="text-right text-xs text-muted-foreground">
            <div>{t("tokenUsage")}</div>
            <div className="font-mono text-sm text-foreground">{tokenText}</div>
          </div>
          {/* Engine actions arrive in the next phase; shown disabled for shape. */}
          <Tooltip>
            <TooltipTrigger asChild>
              <span tabIndex={0}>
                <Button size="sm" className="h-8" disabled>
                  <Play className="mr-1 h-3.5 w-3.5" />
                  {tList("trigger")}
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent>{tList("triggerComingSoon")}</TooltipContent>
          </Tooltip>
          <Button size="icon" variant="ghost" className="h-8 w-8" disabled>
            <Settings2 className="h-4 w-4" />
            <span className="sr-only">{t("settings")}</span>
          </Button>
        </div>
      </div>

      {/* Row ② — graph / board */}
      <div className="min-h-0 flex-1 border-t">
        <Tabs defaultValue="graph" className="flex h-full min-h-0 flex-col">
          <TabsList className="mx-auto mt-2 self-center">
            <TabsTrigger value="graph">{t("subtabGraph")}</TabsTrigger>
            <TabsTrigger value="board">{t("subtabBoard")}</TabsTrigger>
          </TabsList>
          <TabsContent
            value="graph"
            className="min-h-0 flex-1 overflow-auto p-5 data-[state=inactive]:hidden"
          >
            <div className="flex flex-col items-center gap-4">
              <ArtifactNode label={t("rootArtifact")} title={issue.title} />
              <p className="text-center text-xs text-muted-foreground">
                {t("graphPlaceholder")}
              </p>
            </div>
          </TabsContent>
          <TabsContent
            value="board"
            className="min-h-0 flex-1 overflow-auto p-5 data-[state=inactive]:hidden"
          >
            <p className="text-center text-xs text-muted-foreground">
              {t("boardPlaceholder")}
            </p>
          </TabsContent>
        </Tabs>
      </div>

      {/* Row ③ — this issue's iterations / artifacts */}
      <div className="h-48 shrink-0 border-t">
        <Tabs
          defaultValue="iterations"
          className="flex h-full min-h-0 flex-col"
        >
          <TabsList className="mx-5 mt-2 self-start">
            <TabsTrigger value="iterations">
              {t("subtabIterations")}
            </TabsTrigger>
            <TabsTrigger value="artifacts">{t("subtabArtifacts")}</TabsTrigger>
          </TabsList>
          <TabsContent
            value="iterations"
            className="min-h-0 flex-1 overflow-y-auto px-5 py-2 data-[state=inactive]:hidden"
          >
            <p className="text-xs text-muted-foreground">{t("noIterations")}</p>
          </TabsContent>
          <TabsContent
            value="artifacts"
            className="min-h-0 flex-1 overflow-y-auto px-5 py-2 data-[state=inactive]:hidden"
          >
            {artifacts.length <= 1 ? (
              <p className="text-xs text-muted-foreground">
                {t("noArtifacts")}
              </p>
            ) : (
              <ul className="space-y-1 text-sm">
                {artifacts.map((a) => (
                  <li key={a.id} className="flex items-center gap-2">
                    <span className="text-xs text-muted-foreground">
                      {a.kind}
                    </span>
                    <span className="truncate">{a.title}</span>
                  </li>
                ))}
              </ul>
            )}
          </TabsContent>
        </Tabs>
      </div>
    </div>
  )
}

function ArtifactNode({ label, title }: { label: string; title: string }) {
  return (
    <div className="flex min-w-40 max-w-xs flex-col gap-1 rounded-lg border bg-card px-3 py-2 shadow-sm">
      <span className="text-[0.625rem] uppercase tracking-wide text-muted-foreground">
        {label}
      </span>
      <span className="truncate text-sm font-medium">{title}</span>
    </div>
  )
}
