"use client"

import { useTranslations } from "next-intl"
import type { DbConversationSummary, SessionStats } from "@/lib/types"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { SessionDetailsContent } from "./session-details-content"

interface SessionDetailsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  summary: DbConversationSummary
  /**
   * Pre-loaded session stats. When a value (or `null`) is passed the dialog
   * trusts it and renders without a fetch — the detail panel reads it straight
   * from the live runtime session. When `undefined` (the sidebar card, which
   * only holds the summary) the dialog fetches the conversation detail on open
   * to fill in token usage. Forwarded verbatim to `SessionDetailsContent`.
   */
  stats?: SessionStats | null
  /**
   * Pre-resolved model name, following the same contract as `stats`. A value
   * (or `null`) is trusted as-is — the detail panel resolves it from the live
   * session's turns. `undefined` (the sidebar card) makes the content derive
   * the model from the conversation detail it fetches, since
   * `DbConversationSummary.model` is empty for sessions started live in-app.
   */
  model?: string | null
}

export function SessionDetailsDialog({
  open,
  onOpenChange,
  summary,
  stats,
  model,
}: SessionDetailsDialogProps) {
  const t = useTranslations("Folder.sessionDetails")

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription>{t("subtitle")}</DialogDescription>
        </DialogHeader>

        {/* `active` mirrors `open` so the sidebar cold-fetch only runs while
            the dialog is shown, preserving the pre-extraction behavior. */}
        <SessionDetailsContent
          summary={summary}
          stats={stats}
          model={model}
          active={open}
        />
      </DialogContent>
    </Dialog>
  )
}
