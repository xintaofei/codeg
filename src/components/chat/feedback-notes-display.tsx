"use client"

/**
 * Read-only list of live-feedback notes for the current turn, shown above the
 * composer (styled like the message queue). Each note flips from "waiting"
 * (Clock) to "received" (Check) once the agent reads it via
 * `check_user_feedback`. Notes are sent from the composer "+" menu dialog and
 * are not editable/removable here.
 */

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { Check, Clock } from "lucide-react"
import { cn } from "@/lib/utils"
import type { FeedbackItem } from "@/lib/types"

interface FeedbackNotesDisplayProps {
  notes: FeedbackItem[]
}

export function FeedbackNotesDisplay({ notes }: FeedbackNotesDisplayProps) {
  const t = useTranslations("LiveFeedback")

  // Stable chronological order regardless of snapshot/live arrival order
  // (`created_at` is ISO 8601, so a string compare sorts by time).
  const ordered = useMemo(
    () => [...notes].sort((a, b) => a.created_at.localeCompare(b.created_at)),
    [notes]
  )

  if (ordered.length === 0) return null

  return (
    <div className="max-h-28 overflow-y-auto pb-1">
      <div className="flex flex-col gap-0.5">
        {ordered.map((note) => {
          const delivered = note.status === "delivered"
          return (
            <div
              key={note.id}
              className={cn(
                "flex items-center gap-1 rounded-md border px-1.5 py-1 text-[10px] leading-none select-none [text-box-trim:both] [text-box-edge:cap_alphabetic]",
                "bg-muted/40 border-border/70"
              )}
              title={note.text}
            >
              {delivered ? (
                <Check
                  className="h-3 w-3 shrink-0 text-emerald-500"
                  aria-hidden
                />
              ) : (
                <Clock
                  className="h-3 w-3 shrink-0 text-muted-foreground/70"
                  aria-hidden
                />
              )}
              <span className="min-w-0 flex-1 truncate text-[10px] text-foreground/80">
                {note.text}
              </span>
              <span className="shrink-0 text-muted-foreground">
                {delivered ? t("delivered") : t("pending")}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}
