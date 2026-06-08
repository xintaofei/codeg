"use client"

/**
 * Live user-feedback ("steering") settings panel — a single feature kill
 * switch persisted as `feedback.enabled` on the Rust side.
 *
 * When enabled, `codeg-mcp` exposes the `check_user_feedback` tool so an agent
 * can pull mid-turn notes/corrections the user types in the conversation view,
 * and the conversation UI shows the "send a note to the agent" bar while a turn
 * is in flight. Mounted under `/settings/general` next to the multi-agent
 * delegation section, because it's a global feature, not per-agent.
 */

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, MessageSquarePlus } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import {
  type FeedbackSettings,
  getFeedbackSettings,
  setFeedbackSettings,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { primeFeedbackEnabled } from "@/hooks/use-feedback-enabled"

export function SessionFeedbackSettingsSection() {
  const t = useTranslations("SessionFeedbackSettings")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getFeedbackSettings()
      .then((s) => {
        if (cancelled) return
        setEnabled(s.enabled)
        setLoadError(null)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setLoadError(toErrorMessage(err))
      })
      .finally(() => {
        if (cancelled) return
        setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const save = useCallback(async () => {
    const payload: FeedbackSettings = { enabled }
    setSaving(true)
    try {
      const applied = await setFeedbackSettings(payload)
      setEnabled(applied.enabled)
      // Refresh the module-cached flag so open conversations show/hide the
      // feedback bar without a full reload.
      primeFeedbackEnabled(applied.enabled)
      toast.success(t("saved"))
    } catch (err: unknown) {
      toast.error(t("saveFailed"), { description: toErrorMessage(err) })
    } finally {
      setSaving(false)
    }
  }, [enabled, t])

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <MessageSquarePlus
          className="h-4 w-4 text-muted-foreground"
          aria-hidden
        />
        <h2 className="text-sm font-semibold">{t("title")}</h2>
      </div>
      <p className="text-xs text-muted-foreground leading-5">
        {t("description")}
      </p>

      {loadError && (
        <p className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
          {t("loadFailed", { detail: loadError })}
        </p>
      )}

      <div className="flex items-center justify-between gap-3">
        <div className="space-y-1 min-w-0">
          <label htmlFor="feedback-enabled" className="text-sm font-medium">
            {t("enable")}
          </label>
          <p className="text-xs text-muted-foreground">{t("enableHint")}</p>
        </div>
        <Switch
          id="feedback-enabled"
          checked={enabled}
          onCheckedChange={setEnabled}
          disabled={loading}
          className="shrink-0"
        />
      </div>

      <div className="flex justify-end pt-2">
        <Button onClick={save} disabled={loading || saving} size="sm">
          {saving ? (
            <>
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {t("saving")}
            </>
          ) : (
            t("save")
          )}
        </Button>
      </div>
    </section>
  )
}
