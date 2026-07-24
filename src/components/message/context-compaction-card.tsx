"use client"

/**
 * codex-acp #288 (v1.1.3+): the context-compaction lifecycle arrives as an ACP
 * `tool_call` (kind "other") tagged with `_meta.contextCompaction = true` — NOT
 * under the `codex` namespace, unlike codex's other `_meta` extensions. The pair
 * is `Context compacting` (in_progress) → `Context compacted` (completed) sharing
 * one `toolCallId`. Rendered as a subtle status row (not the generic tool shell)
 * so a routine housekeeping event doesn't read as a real tool call. Recognition
 * is by `_meta`, so it works for both the live stream and DB/snapshot reloads.
 */

import { useTranslations } from "next-intl"
import { Archive } from "lucide-react"

import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

/** True when a tool call's `_meta` marks it as the codex context-compaction item. */
export function isContextCompactionMeta(meta: unknown): boolean {
  return (
    !!meta &&
    typeof meta === "object" &&
    (meta as Record<string, unknown>).contextCompaction === true
  )
}

interface Props {
  state?: ToolCallState
}

export function ContextCompactionCard({ state }: Props) {
  const t = useTranslations("Folder.chat.contextCompaction")
  const isRunning = state === "input-streaming" || state === "input-available"
  return (
    <div className="flex items-center gap-2 rounded-md border border-border/40 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">
      <Archive className="size-3.5 shrink-0" />
      <span className={isRunning ? "animate-pulse" : undefined}>
        {isRunning ? t("compacting") : t("compacted")}
      </span>
    </div>
  )
}
