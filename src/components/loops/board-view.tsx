"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"

import type { LoopArtifactKind, LoopArtifactRow } from "@/lib/types"
import { ArtifactStatusBadge } from "@/components/loops/issue-badges"

// One read-only column per artifact kind (the issue root is the container, not a
// board card). Order mirrors the write pipeline.
const COLUMNS: LoopArtifactKind[] = [
  "requirement",
  "design",
  "task",
  "review",
  "result",
]

/**
 * Read-only kanban of an issue's artifacts, one column per kind. A filter view,
 * not a workflow tool — there is no drag-and-drop; the engine owns every status
 * transition. Clicking a card opens it in the drawer via `onSelect`.
 */
export function BoardView({
  artifacts,
  onSelect,
}: {
  artifacts: LoopArtifactRow[]
  onSelect: (id: number) => void
}) {
  const t = useTranslations("Loops.boardView")
  const tKind = useTranslations("Loops.artifactKind")

  const byKind = useMemo(() => {
    const map = new Map<LoopArtifactKind, LoopArtifactRow[]>()
    for (const a of artifacts) {
      if (a.kind === "issue") continue
      const list = map.get(a.kind) ?? []
      list.push(a)
      map.set(a.kind, list)
    }
    for (const list of map.values())
      list.sort((a, b) => a.sort - b.sort || a.id - b.id)
    return map
  }, [artifacts])

  const total = useMemo(
    () => artifacts.filter((a) => a.kind !== "issue").length,
    [artifacts]
  )

  if (total === 0) {
    return (
      <p className="px-1 py-6 text-center text-xs text-muted-foreground">
        {t("empty")}
      </p>
    )
  }

  return (
    <div className="flex gap-3 overflow-x-auto pb-2">
      {COLUMNS.map((kind) => {
        const cards = byKind.get(kind) ?? []
        return (
          <div key={kind} className="flex w-56 shrink-0 flex-col">
            <div className="mb-1.5 flex items-center justify-between px-1">
              <span className="text-xs font-medium">{tKind(kind)}</span>
              <span className="font-mono text-[11px] text-muted-foreground">
                {cards.length}
              </span>
            </div>
            <div className="space-y-1.5">
              {cards.map((a) => (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => onSelect(a.id)}
                  className="flex w-full flex-col gap-1.5 rounded-md border bg-card p-2 text-left hover:bg-accent"
                >
                  <span className="line-clamp-2 text-xs">{a.title}</span>
                  <div className="flex items-center gap-1.5">
                    <span className="font-mono text-[10px] text-muted-foreground">
                      #{a.issue_seq}
                    </span>
                    <ArtifactStatusBadge status={a.status} />
                  </div>
                </button>
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}
