"use client"

import { useCallback, useEffect, useState } from "react"
import { PanelBottomClose } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { getSystemCloseSettings, updateSystemCloseSettings } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type { CloseAction } from "@/lib/types"

export function CloseBehaviorSettings() {
  const t = useTranslations("GeneralSettings")
  const tDynamic = t as unknown as (
    key: string,
    values?: Record<string, string>
  ) => string

  const [loading, setLoading] = useState(true)
  const [action, setAction] = useState<CloseAction>("hide_to_tray")
  const [saving, setSaving] = useState(false)

  const loadSettings = useCallback(async () => {
    setLoading(true)
    try {
      const settings = await getSystemCloseSettings()
      setAction(settings.action)
    } catch (err) {
      console.error("[Settings] load close behavior failed:", err)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    loadSettings().catch((err) => {
      console.error("[Settings] load close behavior failed:", err)
    })
  }, [loadSettings])

  const save = useCallback(
    async (next: CloseAction) => {
      setSaving(true)
      setAction(next)
      try {
        await updateSystemCloseSettings({ action: next })
      } catch (err) {
        setAction(action)
        const message = toErrorMessage(err)
        toast.error(tDynamic("closeActionSaveFailed", { message }))
      } finally {
        setSaving(false)
      }
    },
    [action, tDynamic]
  )

  if (loading) return null

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <PanelBottomClose className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-sm font-semibold">{t("closeActionTitle")}</h2>
      </div>

      <p className="text-xs text-muted-foreground leading-5">
        {t("closeActionDescription")}
      </p>

      <div className="space-y-2">
        <label className="inline-flex items-center gap-2 text-sm">
          <input
            type="radio"
            name="close-action"
            value="hide_to_tray"
            checked={action === "hide_to_tray"}
            disabled={saving}
            onChange={() => save("hide_to_tray")}
          />
          {t("closeActionHideToTray")}
        </label>
        <label className="inline-flex items-center gap-2 text-sm">
          <input
            type="radio"
            name="close-action"
            value="exit"
            checked={action === "exit"}
            disabled={saving}
            onChange={() => save("exit")}
          />
          {t("closeActionExit")}
        </label>
      </div>
    </section>
  )
}
