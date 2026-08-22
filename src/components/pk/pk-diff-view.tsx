"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { AgentIcon } from "@/components/agent-icon"
import { getAgentLabel } from "@/lib/custom-agents"
import type { AgentType } from "@/lib/types"
import { cn } from "@/lib/utils"

/**
 * Lightweight unified-diff renderer for one contestant's worktree diff —
 * line-level red/green with hunk headers, deliberately NOT the three-pane
 * merge editor (that is for conflict resolution, not comparison). Each
 * contestant renders in its own scrollable column.
 */

interface DiffLine {
  kind: "add" | "del" | "hunk" | "ctx"
  text: string
}

export function parseUnifiedDiff(diff: string): DiffLine[] {
  if (diff.trim() === "") return []
  const lines: DiffLine[] = []
  for (const raw of diff.split("\n")) {
    if (
      raw.startsWith("+++") ||
      raw.startsWith("---") ||
      raw.startsWith("diff ") ||
      raw.startsWith("index ")
    ) {
      continue
    }
    if (raw.startsWith("@@")) {
      lines.push({ kind: "hunk", text: raw })
    } else if (raw.startsWith("+")) {
      lines.push({ kind: "add", text: raw })
    } else if (raw.startsWith("-")) {
      lines.push({ kind: "del", text: raw })
    } else {
      lines.push({ kind: "ctx", text: raw })
    }
  }
  return lines
}

function diffStats(lines: DiffLine[]): { added: number; removed: number } {
  let added = 0
  let removed = 0
  for (const line of lines) {
    if (line.kind === "add") added += 1
    if (line.kind === "del") removed += 1
  }
  return { added, removed }
}

const LINE_STYLE: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-400",
  del: "bg-red-500/10 text-red-700 dark:text-red-400",
  hunk: "bg-muted text-muted-foreground",
  ctx: "text-foreground/80",
}

export function PkDiffView({
  agentType,
  diff,
  loading,
}: {
  agentType: AgentType
  diff: string | null
  loading: boolean
}) {
  const t = useTranslations("PkArena.diff")
  const lines = useMemo(() => (diff ? parseUnifiedDiff(diff) : []), [diff])
  const stats = useMemo(() => diffStats(lines), [lines])
  const empty = diff != null && diff.trim() === ""

  return (
    <div className="flex h-full min-h-0 min-w-80 flex-1 flex-col overflow-hidden rounded-lg border border-border">
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2">
        <AgentIcon agentType={agentType} className="size-4" />
        <span className="text-sm font-medium text-foreground">
          {getAgentLabel(agentType)}
        </span>
        {diff != null && !empty ? (
          <span className="ml-auto flex items-baseline gap-2 text-xs tabular-nums">
            <span className="text-emerald-600 dark:text-emerald-400">
              +{stats.added}
            </span>
            <span className="text-red-600 dark:text-red-400">
              −{stats.removed}
            </span>
          </span>
        ) : null}
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <div className="px-3 py-2 text-xs text-muted-foreground">
            {t("loading")}
          </div>
        ) : empty ? (
          <div className="px-3 py-2 text-xs text-muted-foreground">
            {t("empty")}
          </div>
        ) : (
          <pre className="min-w-max px-0 py-1 font-mono text-[11px] leading-5">
            {lines.map((line, index) => (
              <div
                key={index}
                className={cn("px-3 whitespace-pre", LINE_STYLE[line.kind])}
              >
                {line.text === "" ? " " : line.text}
              </div>
            ))}
          </pre>
        )}
      </div>
    </div>
  )
}
