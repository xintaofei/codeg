"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"

import type { LoopArtifactRow } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { ArtifactStatusBadge } from "@/components/loops/issue-badges"

/**
 * Flat list of artifacts annotated with issue, kind, status and producing
 * iteration. Presentational: the caller supplies the rows and hosts the drawer
 * opened via `onSelect`. The issue root node is omitted — it is the container,
 * not a produced artifact.
 */
export function ArtifactList({
  artifacts,
  onSelect,
  showIssue = true,
}: {
  artifacts: LoopArtifactRow[]
  onSelect: (id: number) => void
  showIssue?: boolean
}) {
  const t = useTranslations("Loops.artifactList")
  const tKind = useTranslations("Loops.artifactKind")

  const rows = useMemo(
    () =>
      artifacts
        .filter((a) => a.kind !== "issue")
        .sort(
          (a, b) => a.issue_seq - b.issue_seq || a.sort - b.sort || a.id - b.id
        ),
    [artifacts]
  )

  if (rows.length === 0) {
    return (
      <p className="px-1 py-6 text-center text-xs text-muted-foreground">
        {t("empty")}
      </p>
    )
  }

  return (
    <ul className="space-y-1.5">
      {rows.map((a) => (
        <li key={a.id}>
          <button
            type="button"
            onClick={() => onSelect(a.id)}
            className="flex w-full items-center gap-2 rounded-md border px-2.5 py-1.5 text-left hover:bg-accent"
          >
            {showIssue && (
              <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                #{a.issue_seq}
              </span>
            )}
            <Badge variant="outline" className="shrink-0">
              {tKind(a.kind)}
            </Badge>
            <span className="min-w-0 flex-1 truncate text-sm">{a.title}</span>
            {a.produced_by_iteration_id != null && (
              <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                {t("fromIteration", { id: a.produced_by_iteration_id })}
              </span>
            )}
            <ArtifactStatusBadge status={a.status} />
          </button>
        </li>
      ))}
    </ul>
  )
}
