"use client"

/**
 * What the main window's close button does: hide behind the tray icon and keep
 * running, or quit. Persisted in the `app_metadata` row the desktop shell reads
 * at startup, so it is a local-Tauri-only setting — the page gates this section
 * off any other transport.
 *
 * The tray hint is the interesting part. The backend can only guess whether a
 * tray icon is really visible (a Linux session may show one through a legacy
 * mechanism nothing on the bus reports), so the guess is advisory: hide-to-tray
 * stays selectable, and the action is still subject to the hard requirement that
 * a tray icon was successfully installed — this warns rather than blocks.
 */

import { useCallback, useEffect, useState } from "react"
import { PanelBottomClose } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import {
  SettingsError,
  SettingsSection,
} from "@/components/shared/settings-section"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { getSystemCloseSettings, updateSystemCloseSettings } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type { CloseAction } from "@/lib/types"

const CONTROL_ID = "close-behavior-action"

export function CloseBehaviorSettingsSection() {
  const t = useTranslations("GeneralSettings")

  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  // Until the load lands the control shows the option that is safe everywhere,
  // rather than a guess at the platform default that could flip under the user.
  const [action, setAction] = useState<CloseAction>("exit")
  const [trayAvailable, setTrayAvailable] = useState(true)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const settings = await getSystemCloseSettings()
        if (cancelled) return
        setAction(settings.action)
        setTrayAvailable(settings.tray_available)
        setLoadError(null)
      } catch (err) {
        if (cancelled) return
        // Reported in the section rather than swallowed: without this the
        // fallback value above would read as the user's stored choice.
        setLoadError(toErrorMessage(err))
        console.error("[Settings] load close behavior failed:", err)
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const save = useCallback(
    async (next: CloseAction, previous: CloseAction) => {
      setSaving(true)
      setAction(next)
      try {
        const result = await updateSystemCloseSettings({ action: next })
        // Mirror what was actually persisted, and refresh the hint: the probe
        // runs per call, so the session may have gained a tray since the load.
        setAction(result.action)
        setTrayAvailable(result.tray_available)
        // Clear any stale load error: the save succeeded, so the broken row that
        // caused the load failure has been repaired.
        setLoadError(null)
      } catch (err) {
        setAction(previous)
        toast.error(
          t("closeActionSaveFailed", { message: toErrorMessage(err) })
        )
      } finally {
        setSaving(false)
      }
    },
    [t]
  )

  return (
    <SettingsSection
      icon={PanelBottomClose}
      title={t("closeActionTitle")}
      description={t("closeActionDescription")}
      htmlFor={CONTROL_ID}
      control={
        <Select
          value={action}
          onValueChange={(next) => void save(next as CloseAction, action)}
          disabled={loading || saving}
        >
          <SelectTrigger
            id={CONTROL_ID}
            size="sm"
            className="w-52 bg-background text-xs"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="end">
            <SelectItem value="hide_to_tray">
              {t("closeActionHideToTray")}
            </SelectItem>
            <SelectItem value="exit">{t("closeActionExit")}</SelectItem>
          </SelectContent>
        </Select>
      }
    >
      {loadError && (
        <SettingsError>
          {t("closeActionLoadFailed", { message: loadError })}
        </SettingsError>
      )}

      {/* Only worth saying when the choice it qualifies is the one selected. */}
      {!loading && !trayAvailable && action === "hide_to_tray" && (
        <p className="text-xs leading-5 text-muted-foreground">
          {t("closeActionTrayUnavailableHint")}
        </p>
      )}
    </SettingsSection>
  )
}
