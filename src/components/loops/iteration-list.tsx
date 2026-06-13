"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronDown, ChevronRight, Loader2, MessageSquare } from "lucide-react"

import {
  listLoopArtifacts,
  listLoopIterations,
  listLoopValidations,
} from "@/lib/loops-api"
import type {
  LoopArtifactRow,
  LoopIterationRow,
  LoopValidationRunRow,
} from "@/lib/types"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import {
  ArtifactStatusBadge,
  IterationStatusBadge,
} from "@/components/loops/issue-badges"
import { IterationDialog } from "@/components/loops/iteration-dialog"

/**
 * List of loop iterations — every iteration in a space, or one issue's when
 * `issueId` is given. Each row carries its issue, stage, status and token usage
 * and expands to the artifacts it produced and the validation runs it recorded.
 * A row with a conversation opens it read-only in the iteration viewer.
 */
export function IterationList({
  spaceId,
  issueId,
}: {
  spaceId: number
  issueId?: number
}) {
  const t = useTranslations("Loops.iterationList")
  const tStage = useTranslations("Loops.stage")

  const [iterations, setIterations] = useState<LoopIterationRow[]>([])
  const [artifacts, setArtifacts] = useState<LoopArtifactRow[]>([])
  const [validations, setValidations] = useState<LoopValidationRunRow[]>([])
  const [loading, setLoading] = useState(true)
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [openConversationId, setOpenConversationId] = useState<number | null>(
    null
  )

  const refresh = useCallback(async () => {
    try {
      const [its, arts, vals] = await Promise.all([
        listLoopIterations(spaceId, issueId),
        listLoopArtifacts(spaceId),
        listLoopValidations(spaceId),
      ])
      setIterations(its)
      setArtifacts(arts)
      setValidations(vals)
    } catch {
      // non-fatal; the empty state covers a listing failure
    } finally {
      setLoading(false)
    }
  }, [spaceId, issueId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useLoopChanged(() => {
    void refresh()
  }, spaceId)

  // Group produced artifacts + validation runs by iteration for the expansions.
  const byIteration = useMemo(() => {
    const arts = new Map<number, LoopArtifactRow[]>()
    for (const a of artifacts) {
      if (a.produced_by_iteration_id == null) continue
      const list = arts.get(a.produced_by_iteration_id) ?? []
      list.push(a)
      arts.set(a.produced_by_iteration_id, list)
    }
    const vals = new Map<number, LoopValidationRunRow[]>()
    for (const v of validations) {
      if (v.iteration_id == null) continue
      const list = vals.get(v.iteration_id) ?? []
      list.push(v)
      vals.set(v.iteration_id, list)
    }
    return { arts, vals }
  }, [artifacts, validations])

  // Newest first.
  const sorted = useMemo(
    () => [...iterations].sort((a, b) => b.id - a.id),
    [iterations]
  )

  const toggle = (id: number) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  if (loading) {
    return (
      <div className="flex h-24 items-center justify-center text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
      </div>
    )
  }

  if (sorted.length === 0) {
    return (
      <p className="px-1 py-6 text-center text-xs text-muted-foreground">
        {t("empty")}
      </p>
    )
  }

  return (
    <>
      <ul className="space-y-1.5">
        {sorted.map((it) => {
          const isOpen = expanded.has(it.id)
          const producedArts = byIteration.arts.get(it.id) ?? []
          const runs = byIteration.vals.get(it.id) ?? []
          return (
            <li key={it.id} className="rounded-md border">
              <div className="flex items-center gap-2 px-2.5 py-1.5">
                <button
                  type="button"
                  onClick={() => toggle(it.id)}
                  className="text-muted-foreground hover:text-foreground"
                  aria-label={isOpen ? t("collapse") : t("expand")}
                >
                  {isOpen ? (
                    <ChevronDown className="h-4 w-4" />
                  ) : (
                    <ChevronRight className="h-4 w-4" />
                  )}
                </button>
                <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                  #{it.issue_seq}
                </span>
                <span className="text-sm font-medium">{tStage(it.stage)}</span>
                <IterationStatusBadge status={it.status} />
                {it.attempt > 0 && (
                  <span className="text-[11px] text-muted-foreground">
                    {t("attempt", { n: it.attempt })}
                  </span>
                )}
                <span className="ml-auto shrink-0 font-mono text-[11px] text-muted-foreground">
                  {t("tokens", { count: it.tokens_used.toLocaleString() })}
                </span>
                {it.conversation_id != null && (
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="h-7 w-7 shrink-0"
                    onClick={() => setOpenConversationId(it.conversation_id)}
                    aria-label={t("openConversation")}
                  >
                    <MessageSquare className="h-3.5 w-3.5" />
                  </Button>
                )}
              </div>

              {isOpen && (
                <div className="space-y-2 border-t px-3 py-2">
                  <div>
                    <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                      {t("producedArtifacts")}
                    </div>
                    {producedArts.length === 0 ? (
                      <p className="text-xs text-muted-foreground/70">
                        {t("nothingProduced")}
                      </p>
                    ) : (
                      <ul className="mt-1 space-y-1">
                        {producedArts.map((a) => (
                          <li
                            key={a.id}
                            className="flex items-center gap-2 text-xs"
                          >
                            <ArtifactStatusBadge status={a.status} />
                            <span className="truncate">{a.title}</span>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                  {runs.length > 0 && (
                    <div>
                      <div className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                        {t("validationRuns")}
                      </div>
                      <ul className="mt-1 space-y-1">
                        {runs.map((r) => (
                          <li key={r.id} className="text-xs">
                            <span
                              className={
                                r.passed
                                  ? "font-medium text-emerald-600 dark:text-emerald-400"
                                  : "font-medium text-red-600 dark:text-red-400"
                              }
                            >
                              {r.passed ? t("passed") : t("failed")}
                            </span>
                            {r.commands.length > 0 && (
                              <span className="ml-2 font-mono text-muted-foreground">
                                {r.commands.join(" · ")}
                              </span>
                            )}
                          </li>
                        ))}
                      </ul>
                    </div>
                  )}
                </div>
              )}
            </li>
          )
        })}
      </ul>

      <IterationDialog
        open={openConversationId != null}
        onOpenChange={(o) => {
          if (!o) setOpenConversationId(null)
        }}
        conversationId={openConversationId ?? 0}
        connectionId={null}
        agentType={null}
      />
    </>
  )
}
