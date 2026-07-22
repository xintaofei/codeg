"use client"

import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import type {
  AutoReplyPendingState,
  AutoReplyStopNotice,
} from "@/hooks/use-auto-reply-engine"

interface AutoReplyBannerProps {
  pending: AutoReplyPendingState | null
  stopNotice: AutoReplyStopNotice | null
  onCancel: () => void
  onDismissStopNotice: () => void
  className?: string
}

export function AutoReplyBanner({
  pending,
  stopNotice,
  onCancel,
  onDismissStopNotice,
  className,
}: AutoReplyBannerProps) {
  const t = useTranslations("Folder.chat.autoReply")

  if (pending) {
    const seconds = Math.max(1, Math.ceil(pending.remainingMs / 1000))
    return (
      <div
        className={cn(
          "mb-2 rounded-lg border border-sky-500/30 bg-sky-500/10 px-3 py-2 text-sm text-foreground",
          className
        )}
        role="status"
        aria-live="polite"
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-0.5">
            <div className="font-medium">
              {t("bannerTitle", {
                reply: pending.replyText,
                seconds,
              })}
            </div>
            <div className="text-xs text-muted-foreground">
              {t("bannerMatched", { label: pending.matchedLabel })}
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="shrink-0"
            onClick={onCancel}
          >
            {t("cancel")}
          </Button>
        </div>
      </div>
    )
  }

  if (stopNotice) {
    return (
      <div
        className={cn(
          "mb-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-foreground",
          className
        )}
        role="status"
      >
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 space-y-0.5">
            <div>{t("stopNotice")}</div>
            <div className="text-xs text-muted-foreground">
              {t("bannerMatched", { label: stopNotice.matchedLabel })}
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="shrink-0"
            onClick={onDismissStopNotice}
          >
            {t("dismiss")}
          </Button>
        </div>
      </div>
    )
  }

  return null
}
