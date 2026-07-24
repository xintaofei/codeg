"use client"

/**
 * codex-acp #288 (v1.1.3+): the context-compaction lifecycle arrives as an ACP
 * `tool_call` (kind "other") tagged with `_meta.contextCompaction = true` — NOT
 * under the `codex` namespace, unlike codex's other `_meta` extensions. The pair
 * is `Context compacting` (in_progress) → `Context compacted` (completed) sharing
 * one `toolCallId`. Grok's `auto_compact_completed` bridge synthesizes the same
 * shape (with `tokensBefore`/`tokensAfter`).
 *
 * Rendered as a centered, chrome-less divider (a horizontal rule flanking a
 * token-delta label) so it reads as a conversation boundary marker — "context
 * was compacted here" — not a real tool call. Recognition is by `_meta`, so it
 * works for the live stream and DB/snapshot reloads. In history the compaction
 * is hoisted to a dedicated standalone timeline item (see `message-list-view`'s
 * `"compaction"` render kind) so it sits BETWEEN turns rather than folding into
 * the preceding assistant reply.
 */

import { useTranslations } from "next-intl"
import { Archive } from "lucide-react"

import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

// `isContextCompactionMeta` now lives in the dependency-free
// `@/lib/context-compaction` module (shared with the grouping pass); re-exported
// here so existing importers keep resolving it from this file.
export { isContextCompactionMeta } from "@/lib/context-compaction"

/** Read a finite numeric field off the opaque `_meta` pass-through. */
function readTokenCount(meta: unknown, key: string): number | null {
  if (!meta || typeof meta !== "object") return null
  const value = (meta as Record<string, unknown>)[key]
  return typeof value === "number" && Number.isFinite(value) ? value : null
}

interface Props {
  state?: ToolCallState
  /**
   * ACP tool-call `_meta`. grok stamps `tokensBefore`/`tokensAfter` on its
   * `auto_compact_completed` card (codex omits them) so we can show the delta.
   */
  meta?: Record<string, unknown> | null
}

export function ContextCompactionCard({ state, meta }: Props) {
  const t = useTranslations("Folder.chat.contextCompaction")
  const isRunning = state === "input-streaming" || state === "input-available"
  const before = readTokenCount(meta, "tokensBefore")
  const after = readTokenCount(meta, "tokensAfter")
  // Only show the delta when it's a real reduction — a no-op (before === after)
  // would read as a bug, so fall back to the plain label there and for codex
  // (which sends no counts).
  const label = isRunning
    ? t("compacting")
    : before !== null && after !== null && before !== after
      ? t("compactedTokens", {
          before: before.toLocaleString(),
          after: after.toLocaleString(),
        })
      : t("compacted")
  return (
    <div className="flex items-center gap-3 py-1 text-xs text-muted-foreground/80 select-none">
      <div className="h-px flex-1 bg-gradient-to-r from-transparent to-border/70" />
      <div className="flex shrink-0 items-center gap-1.5">
        <Archive className="size-3.5" />
        <span className={isRunning ? "animate-pulse" : undefined}>{label}</span>
      </div>
      <div className="h-px flex-1 bg-gradient-to-l from-transparent to-border/70" />
    </div>
  )
}
