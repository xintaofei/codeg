"use client"

import { useCallback, useEffect, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Switch } from "@/components/ui/switch"
import { getChatEventFilter, setChatEventFilter } from "@/lib/api"

const ALL_EVENT_TYPES = [
  {
    id: "session_started",
    labelKey: "sessionStarted",
    descKey: "sessionStartedDesc",
  },
  {
    id: "turn_complete",
    labelKey: "turnComplete",
    descKey: "turnCompleteDesc",
  },
  { id: "error", labelKey: "error", descKey: "errorDesc" },
  {
    id: "status_disconnected",
    labelKey: "statusDisconnected",
    descKey: "statusDisconnectedDesc",
  },
  { id: "git_push", labelKey: "gitPush", descKey: "gitPushDesc" },
  { id: "git_commit", labelKey: "gitCommit", descKey: "gitCommitDesc" },
] as const

const ALL_IDS = ALL_EVENT_TYPES.map((e) => e.id)

function parseFilter(arr: string[] | null): Set<string> {
  if (!arr) return new Set(ALL_IDS)
  return new Set(arr)
}

export function ChannelEventsTab() {
  const t = useTranslations("ChatChannelSettings.events")
  const [enabledEvents, setEnabledEvents] = useState<Set<string>>(
    new Set(ALL_IDS)
  )
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    getChatEventFilter()
      .then((arr) => setEnabledEvents(parseFilter(arr)))
      .catch(() => {})
      .finally(() => setLoading(false))
  }, [])

  const allEnabled = enabledEvents.size === ALL_EVENT_TYPES.length

  const handleToggle = useCallback(
    async (eventId: string, checked: boolean) => {
      setSaving(true)
      try {
        const next = new Set(enabledEvents)
        if (checked) {
          next.add(eventId)
        } else {
          next.delete(eventId)
        }
        const isAll = next.size === ALL_EVENT_TYPES.length
        await setChatEventFilter(isAll ? null : [...next])
        setEnabledEvents(next)
        toast.success(t("saved"))
      } catch {
        toast.error(t("saveFailed"))
      } finally {
        setSaving(false)
      }
    },
    [enabledEvents, t]
  )

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {allEnabled && (
        <p className="text-xs text-muted-foreground">{t("allEnabled")}</p>
      )}

      <section className="space-y-1">
        {ALL_EVENT_TYPES.map((evt) => (
          <div
            key={evt.id}
            className="flex items-center justify-between rounded-lg border bg-card px-4 py-3"
          >
            <div className="min-w-0">
              <div className="text-sm font-medium">{t(evt.labelKey)}</div>
              <div className="text-xs text-muted-foreground">
                {t(evt.descKey)}
              </div>
            </div>
            <Switch
              checked={enabledEvents.has(evt.id)}
              disabled={saving}
              onCheckedChange={(checked) => handleToggle(evt.id, checked)}
            />
          </div>
        ))}
      </section>
    </div>
  )
}
