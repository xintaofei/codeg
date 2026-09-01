"use client"

/**
 * In-stream capsule for the codeg-mcp `check_user_feedback` tool — the agent's
 * mid-turn poll for live-steering notes the user sent while it worked.
 *
 * Only checks that actually received feedback reach this card; the routine
 * "no new feedback" polls (and in-flight ones) are dropped upstream by
 * `dropHiddenFeedbackChecks`, so there is always at least one note to show
 * (except the rare error path). Collapsed by default into a capsule that
 * summarizes the notes; expanding lists each note with its send time. Mirrors
 * the `AskQuestionResultCard` capsule so the two MCP human-in-the-loop tools
 * read consistently in the transcript.
 */

import { useMemo, useState } from "react"
import { useLocale, useTranslations } from "next-intl"
import {
  AlertTriangle,
  ChevronDown,
  ChevronUp,
  MessageSquareMore,
} from "lucide-react"

import { cn } from "@/lib/utils"
import { parseFeedbackCheckOutcome } from "@/lib/feedback-check"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"

interface Props {
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
}

function formatTime(
  fmt: Intl.DateTimeFormat,
  iso: string | null
): string | null {
  if (!iso) return null
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return null
  return fmt.format(date)
}

export function FeedbackCheckResultCard({ output, errorText, state }: Props) {
  const t = useTranslations("Folder.chat.feedbackCheckResult")
  const locale = useLocale()
  const [expanded, setExpanded] = useState(false)

  const timeFormat = useMemo(
    () =>
      new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }),
    [locale]
  )
  const outcome = useMemo(() => parseFeedbackCheckOutcome(output), [output])

  const isError = !!errorText?.trim() || state === "output-error"

  // Error path: a compact, non-expandable capsule (errors are rare here).
  if (isError) {
    return (
      <div
        data-testid="feedback-check-result-card"
        className="mb-2 flex w-full items-center gap-2 rounded-full border border-destructive/30 bg-card px-3 py-1.5 text-xs"
      >
        <AlertTriangle className="size-4 shrink-0 text-destructive" />
        <span className="min-w-0 flex-1 truncate text-destructive">
          {errorText?.trim() || t("errorTitle")}
        </span>
      </div>
    )
  }

  const entries = outcome?.entries ?? []
  // Defensive: the hidden checks are dropped upstream, so this should not happen
  // — but never render an empty card if one slips through.
  if (entries.length === 0) return null

  const capsule = (
    <button
      type="button"
      onClick={() => setExpanded((v) => !v)}
      aria-expanded={expanded}
      // Use `title`, not `aria-label`: the visible summary (count + first note)
      // must stay the button's accessible name, with `aria-expanded` conveying
      // the toggle state. An `aria-label` here would override that summary for
      // screen readers; `title` only adds a supplementary description/tooltip.
      title={expanded ? t("collapse") : t("expand")}
      data-testid="feedback-check-result-card"
      className={cn(
        "flex w-full items-center gap-2 rounded-full border border-primary/30 bg-card px-3 py-1.5 text-left transition-colors hover:bg-muted/40",
        !expanded && "mb-2"
      )}
    >
      <MessageSquareMore className="size-4 shrink-0 text-primary" />
      <span className="min-w-0 flex-1 truncate text-xs">
        <span className="mr-1 text-muted-foreground">
          {t("count", { count: entries.length })}
        </span>
        <span className="text-foreground/90">{entries[0].text}</span>
      </span>
      {expanded ? (
        <ChevronUp className="size-3.5 shrink-0 text-muted-foreground" />
      ) : (
        <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
      )}
    </button>
  )

  if (!expanded) return capsule

  return (
    <div className="mb-2 space-y-1.5">
      {capsule}
      <ul className="overflow-hidden rounded-xl border border-primary/30 bg-card divide-y divide-border/60">
        {entries.map((entry, i) => {
          const time = formatTime(timeFormat, entry.createdAt)
          return (
            <li key={i} className="flex items-start gap-2 px-3 py-2">
              <span className="min-w-0 flex-1 text-xs whitespace-pre-wrap break-words text-foreground/90">
                {entry.text}
              </span>
              {time && (
                <time
                  dateTime={entry.createdAt ?? undefined}
                  className="shrink-0 text-3xs text-muted-foreground tabular-nums"
                >
                  {time}
                </time>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
