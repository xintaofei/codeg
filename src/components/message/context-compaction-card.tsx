"use client"

/**
 * codex-acp #288 (v1.1.3+): the context-compaction lifecycle arrives as an ACP
 * `tool_call` tagged with `_meta.contextCompaction` — NOT under the `codex`
 * namespace, unlike codex's other `_meta` extensions. Through 1.2.x the tag is
 * the boolean `true`; 1.3.0 (#396) replaces it with the versioned object
 * `{version: 1, …}` on a claude-aligned lifecycle (kind "think", title
 * "Compact conversation", in_progress → completed sharing one `toolCallId`)
 * whose schema reserves optional `trigger`/`preTokens`/`postTokens`/
 * `durationMs`/`error` fields — 1.3.0 emits none of them yet. Grok's
 * `auto_compact_completed` bridge keeps synthesizing the boolean shape with
 * top-level `tokensBefore`/`tokensAfter`, so the delta label reads both
 * namings.
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
import { contextCompactionPayload } from "@/lib/context-compaction"

// `isContextCompactionMeta` now lives in the dependency-free
// `@/lib/context-compaction` module (shared with the grouping pass); re-exported
// here so existing importers keep resolving it from this file.
export { isContextCompactionMeta } from "@/lib/context-compaction"

/** Read a finite numeric field off an opaque record. */
function readNumber(
  source: Record<string, unknown> | null | undefined,
  key: string
): number | null {
  if (!source) return null
  const value = source[key]
  return typeof value === "number" && Number.isFinite(value) ? value : null
}

/** Read a non-blank string field off an opaque record. */
function readText(
  source: Record<string, unknown> | null | undefined,
  key: string
): string | null {
  if (!source) return null
  const value = source[key]
  return typeof value === "string" && value.trim().length > 0 ? value : null
}

/** `durationMs` as a compact human label ("3.2s" / "45s"), or null. */
function formatDuration(durationMs: number | null): string | null {
  if (durationMs === null || durationMs <= 0) return null
  const seconds = durationMs / 1000
  return `${seconds >= 10 ? Math.round(seconds) : seconds.toFixed(1)}s`
}

interface Props {
  state?: ToolCallState
  /**
   * ACP tool-call `_meta`. grok stamps top-level `tokensBefore`/`tokensAfter`
   * next to its boolean marker; codex 1.3.0+ nests reserved fields inside the
   * versioned `contextCompaction` object (`preTokens`/`postTokens`/
   * `durationMs`/`trigger`/`error`). Every field may be absent — 1.3.0 sends
   * the bare `{version: 1}` — and the card then renders exactly as before.
   */
  meta?: Record<string, unknown> | null
}

export function ContextCompactionCard({ state, meta }: Props) {
  const t = useTranslations("Folder.chat.contextCompaction")
  const isRunning = state === "input-streaming" || state === "input-available"
  const payload = contextCompactionPayload(meta)
  const before =
    readNumber(meta, "tokensBefore") ?? readNumber(payload, "preTokens")
  const after =
    readNumber(meta, "tokensAfter") ?? readNumber(payload, "postTokens")
  const errorText = readText(payload, "error")
  const failed = errorText !== null || state === "output-error"
  const duration = formatDuration(readNumber(payload, "durationMs"))
  // Reserved-field vocabulary is adapter-defined and untranslated; surface
  // trigger/error as a tooltip instead of inline prose.
  const tooltip = errorText ?? readText(payload, "trigger") ?? undefined
  // Only show the delta when it's a real reduction — a no-op (before === after)
  // would read as a bug, so fall back to the plain label there and for codex
  // (which sends no counts).
  const label = failed
    ? t("failed")
    : isRunning
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
      <div
        className={`flex shrink-0 items-center gap-1.5${failed ? " text-destructive/80" : ""}`}
        title={tooltip}
      >
        <Archive className="size-3.5" />
        <span className={isRunning ? "animate-pulse" : undefined}>{label}</span>
        {!failed && !isRunning && duration ? (
          <span className="text-muted-foreground/60">· {duration}</span>
        ) : null}
      </div>
      <div className="h-px flex-1 bg-gradient-to-l from-transparent to-border/70" />
    </div>
  )
}
