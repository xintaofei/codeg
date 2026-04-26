"use client"

import { useCallback, useEffect, useState } from "react"
import { ExternalLink, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  getSystemOpenTargetSettings,
  updateSystemOpenTargetSettings,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { OPEN_TARGET_REGISTRY, isSystemOpenTarget } from "@/lib/open-targets"
import { isDesktop } from "@/lib/platform"
import type { SystemOpenTarget, SystemWebFileOpenMethod } from "@/lib/types"

function isSystemWebFileOpenMethod(
  value: string
): value is SystemWebFileOpenMethod {
  return value === "browser" || value === "editor"
}

export function GeneralSettings() {
  const t = useTranslations("GeneralSettings")
  const [loading, setLoading] = useState(true)
  const [savingOpenTarget, setSavingOpenTarget] = useState(false)
  const [savingWebFileOpenMethod, setSavingWebFileOpenMethod] = useState(false)
  const [openTarget, setOpenTarget] = useState<SystemOpenTarget>("file_manager")
  const [webFileOpenMethod, setWebFileOpenMethod] =
    useState<SystemWebFileOpenMethod>("browser")
  const [loadError, setLoadError] = useState<string | null>(null)
  const savingOpenSettings = savingOpenTarget || savingWebFileOpenMethod

  const loadSettings = useCallback(async () => {
    setLoading(true)
    setLoadError(null)

    try {
      const settings = await getSystemOpenTargetSettings()
      setOpenTarget(settings.target)
      setWebFileOpenMethod(settings.web_file_open_method ?? "browser")
    } catch (err) {
      const message = toErrorMessage(err)
      setLoadError(message)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadSettings()
  }, [loadSettings])

  const saveOpenTarget = useCallback(
    async (target: SystemOpenTarget, previous: SystemOpenTarget) => {
      setSavingOpenTarget(true)

      try {
        const next = await updateSystemOpenTargetSettings({
          target,
          web_file_open_method: webFileOpenMethod,
        })
        setOpenTarget(next.target)
        setWebFileOpenMethod(next.web_file_open_method)
      } catch (err) {
        setOpenTarget(previous)
        const message = toErrorMessage(err)
        toast.error(t("openTargetSaveFailed", { message }))
      } finally {
        setSavingOpenTarget(false)
      }
    },
    [t, webFileOpenMethod]
  )

  const saveWebFileOpenMethod = useCallback(
    async (
      method: SystemWebFileOpenMethod,
      previous: SystemWebFileOpenMethod
    ) => {
      setSavingWebFileOpenMethod(true)

      try {
        const next = await updateSystemOpenTargetSettings({
          target: openTarget,
          web_file_open_method: method,
        })
        setOpenTarget(next.target)
        setWebFileOpenMethod(next.web_file_open_method)
      } catch (err) {
        setWebFileOpenMethod(previous)
        const message = toErrorMessage(err)
        toast.error(t("webFileOpenMethodSaveFailed", { message }))
      } finally {
        setSavingWebFileOpenMethod(false)
      }
    },
    [openTarget, t]
  )

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        <section className="space-y-1">
          <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
          <p className="text-xs text-muted-foreground">
            {t("sectionDescription")}
          </p>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <ExternalLink className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("openTargetTitle")}</h2>
          </div>

          {loadError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {t("loadFailed", { message: loadError })}
            </div>
          )}

          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div className="space-y-1">
              <p className="text-sm font-medium">{t("defaultOpenTarget")}</p>
              <p className="text-xs text-muted-foreground leading-5">
                {t("defaultOpenTargetDescription")}
              </p>
            </div>
            <Select
              value={openTarget}
              onValueChange={(value) => {
                if (!isSystemOpenTarget(value)) return
                const previous = openTarget
                setOpenTarget(value)
                void saveOpenTarget(value, previous)
              }}
              disabled={savingOpenSettings || !isDesktop()}
            >
              <SelectTrigger className="w-full sm:w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {OPEN_TARGET_REGISTRY.map((target) => (
                  <SelectItem key={target.id} value={target.id}>
                    {t(target.labelKey)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div className="space-y-1">
              <p className="text-sm font-medium">{t("webFileOpenMethod")}</p>
              <p className="text-xs text-muted-foreground leading-5">
                {t("webFileOpenMethodDescription")}
              </p>
            </div>
            <Select
              value={webFileOpenMethod}
              onValueChange={(value) => {
                if (!isSystemWebFileOpenMethod(value)) return
                const previous = webFileOpenMethod
                setWebFileOpenMethod(value)
                void saveWebFileOpenMethod(value, previous)
              }}
              disabled={savingOpenSettings || !isDesktop()}
            >
              <SelectTrigger className="w-full sm:w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="browser">
                  {t("webFileOpenMethods.browser")}
                </SelectItem>
                <SelectItem value="editor">
                  {t("webFileOpenMethods.editor")}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          {!isDesktop() && (
            <p className="text-[11px] text-muted-foreground">
              {t("openTargetDesktopOnly")}
            </p>
          )}
        </section>
      </div>
    </ScrollArea>
  )
}
