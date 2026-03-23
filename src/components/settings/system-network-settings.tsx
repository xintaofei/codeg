"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import {
  ArrowUpCircle,
  CheckCircle2,
  Languages,
  Loader2,
  RefreshCw,
  Wifi,
} from "lucide-react"
import type { Update } from "@tauri-apps/plugin-updater"
import { useLocale, useTranslations } from "next-intl"
import { toast } from "sonner"
import { useAppI18n } from "@/components/i18n-provider"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  getSystemProxySettings,
  updateSystemLanguageSettings,
  updateSystemProxySettings,
} from "@/lib/tauri"
import type { AppLocale } from "@/lib/types"
import {
  checkAppUpdate,
  closeAppUpdate,
  getCurrentAppVersion,
  installAppUpdate,
  normalizeAppUpdateError,
  relaunchApp,
} from "@/lib/updater"
import { APP_LOCALES } from "@/lib/i18n"

const PROXY_EXAMPLE = "http://127.0.0.1:7890"
const APP_LANGUAGE_VALUES = APP_LOCALES

type LanguageSelectValue = "system" | AppLocale

function isAppLocale(value: string): value is AppLocale {
  return APP_LANGUAGE_VALUES.includes(value as AppLocale)
}

type UpdateAction = "check" | "install"

export function SystemNetworkSettings() {
  const t = useTranslations("SystemSettings")
  const tLanguage = useTranslations("Language")
  const locale = useLocale()
  const { languageSettings, languageSettingsLoaded, setLanguageSettings } =
    useAppI18n()

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [savingLanguage, setSavingLanguage] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [proxyUrl, setProxyUrl] = useState("")
  const [loadError, setLoadError] = useState<string | null>(null)
  const [currentVersion, setCurrentVersion] = useState<string>("")
  const [availableUpdate, setAvailableUpdate] = useState<Update | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [installingUpdate, setInstallingUpdate] = useState(false)
  const [updateError, setUpdateError] = useState<string | null>(null)
  const [lastCheckedAt, setLastCheckedAt] = useState<Date | null>(null)

  const [appLanguage, setAppLanguage] = useState<LanguageSelectValue>(
    languageSettings.mode === "system" ? "system" : languageSettings.language
  )

  useEffect(() => {
    setAppLanguage(
      languageSettings.mode === "system" ? "system" : languageSettings.language
    )
  }, [languageSettings])

  const languageLabels = useMemo(
    () => ({
      en: tLanguage("english"),
      zh_cn: tLanguage("simplifiedChinese"),
      zh_tw: tLanguage("traditionalChinese"),
      ja: tLanguage("japanese"),
      ko: tLanguage("korean"),
      es: tLanguage("spanish"),
      de: tLanguage("german"),
      fr: tLanguage("french"),
      pt: tLanguage("portuguese"),
      ar: tLanguage("arabic"),
    }),
    [tLanguage]
  )

  const formattedLastCheckedAt = useMemo(() => {
    if (!lastCheckedAt) return null
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(lastCheckedAt)
  }, [lastCheckedAt, locale])

  const formattedUpdateDate = useMemo(() => {
    if (!availableUpdate?.date) return null

    const parsed = new Date(availableUpdate.date)
    if (Number.isNaN(parsed.getTime())) return availableUpdate.date

    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
    }).format(parsed)
  }, [availableUpdate?.date, locale])

  const updateNotes = useMemo(
    () => availableUpdate?.body?.trim() || t("none"),
    [availableUpdate?.body, t]
  )

  const updateStatusMessage = useMemo(() => {
    if (checkingUpdate) return t("checking")
    if (installingUpdate) return t("updating")
    if (availableUpdate) return null
    if (lastCheckedAt) return t("alreadyLatest")
    return null
  }, [availableUpdate, checkingUpdate, installingUpdate, lastCheckedAt, t])

  const loadSettings = useCallback(async () => {
    setLoading(true)
    setLoadError(null)

    try {
      const [proxySettings, version] = await Promise.all([
        getSystemProxySettings(),
        getCurrentAppVersion(),
      ])

      setEnabled(proxySettings.enabled)
      setProxyUrl(proxySettings.proxy_url ?? "")
      setCurrentVersion(version)
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setLoadError(message)
      console.error("[Settings] load system settings failed:", err)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    loadSettings().catch((err) => {
      console.error("[Settings] load system settings failed:", err)
    })
    checkForUpdates().catch((err) => {
      console.error("[Settings] auto check update failed:", err)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    return () => {
      if (!availableUpdate) return
      closeAppUpdate(availableUpdate).catch((err) => {
        console.error("[Settings] release updater resource failed:", err)
      })
    }
  }, [availableUpdate])

  const saveProxySettings = useCallback(
    async (nextEnabled: boolean, nextProxyUrl: string) => {
      if (nextEnabled && !nextProxyUrl.trim()) return

      setSaving(true)
      try {
        const next = await updateSystemProxySettings({
          enabled: nextEnabled,
          proxy_url: nextProxyUrl.trim() || null,
        })
        setEnabled(next.enabled)
        setProxyUrl(next.proxy_url ?? "")
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(t("saveFailed", { message }))
      } finally {
        setSaving(false)
      }
    },
    [t]
  )

  const saveLanguage = useCallback(
    async (lang: LanguageSelectValue) => {
      setSavingLanguage(true)

      try {
        const next = await updateSystemLanguageSettings({
          mode: lang === "system" ? "system" : "manual",
          language: lang === "system" ? languageSettings.language : lang,
        })

        setLanguageSettings(next)
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(t("languageSaveFailed", { message }))
      } finally {
        setSavingLanguage(false)
      }
    },
    [languageSettings.language, setLanguageSettings, t]
  )

  const formatUpdateError = useCallback(
    (error: unknown, action: UpdateAction): string => {
      const { kind, rawMessage } = normalizeAppUpdateError(error)

      switch (kind) {
        case "source_unreachable":
          return t("updateErrors.sourceUnavailable")
        case "network":
          return t("updateErrors.network")
        case "download_failed":
          return t("updateErrors.downloadFailed")
        case "install_failed":
          return t("updateErrors.installFailed")
        case "unknown":
        default:
          if (action === "install") {
            return t("updateErrors.installFailed")
          }
          console.error("[Settings] updater unknown error:", rawMessage)
          return t("updateErrors.unknown")
      }
    },
    [t]
  )

  const checkForUpdates = useCallback(async () => {
    setCheckingUpdate(true)
    setUpdateError(null)

    try {
      const previousUpdate = availableUpdate
      const result = await checkAppUpdate()
      setCurrentVersion(result.currentVersion)
      setLastCheckedAt(new Date())

      if (result.update) {
        setAvailableUpdate(result.update)
      } else {
        setAvailableUpdate(null)
      }

      if (previousUpdate && previousUpdate !== result.update) {
        await closeAppUpdate(previousUpdate)
      }
    } catch (err) {
      const message = formatUpdateError(err, "check")
      setUpdateError(message)
      toast.error(t("checkUpdateFailed", { message }))
      console.error("[Settings] check app update failed:", err)
    } finally {
      setCheckingUpdate(false)
    }
  }, [availableUpdate, formatUpdateError, t])

  const installUpdate = useCallback(async () => {
    if (!availableUpdate) return

    setInstallingUpdate(true)
    setUpdateError(null)

    try {
      await installAppUpdate(availableUpdate)
      toast.success(t("installSuccess"))
      await relaunchApp()
    } catch (err) {
      const message = formatUpdateError(err, "install")
      setUpdateError(message)
      toast.error(t("installFailed", { message }))
      console.error("[Settings] install app update failed:", err)
    } finally {
      setInstallingUpdate(false)
    }
  }, [availableUpdate, formatUpdateError, t])

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  return (
    <div className="h-full overflow-auto">
      <div className="w-full space-y-4">
        <section className="space-y-1">
          <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
          <p className="text-xs text-muted-foreground">
            {t("sectionDescription")}
          </p>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            {checkingUpdate ? (
              <RefreshCw className="h-4 w-4 text-muted-foreground animate-spin" />
            ) : availableUpdate ? (
              <ArrowUpCircle className="h-4 w-4 text-muted-foreground" />
            ) : lastCheckedAt ? (
              <CheckCircle2 className="h-4 w-4 text-green-500" />
            ) : (
              <RefreshCw className="h-4 w-4 text-muted-foreground" />
            )}
            <h2 className="text-sm font-semibold">{t("versionTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("updateDescription")}
          </p>

          <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-2">
            <div className="flex items-center justify-between gap-3">
              <p className="text-muted-foreground">
                {t("currentVersion")}：
                {currentVersion ? `v${currentVersion}` : "-"}
              </p>
              {checkingUpdate ? (
                <Button
                  key="checking-update"
                  size="sm"
                  disabled
                  aria-busy="true"
                  className="w-[9.5rem] justify-center transition-none"
                >
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("checking")}
                </Button>
              ) : availableUpdate ? (
                <Button
                  size="sm"
                  onClick={installUpdate}
                  disabled={installingUpdate}
                >
                  {installingUpdate ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("updating")}
                    </>
                  ) : (
                    <>
                      <ArrowUpCircle className="h-3.5 w-3.5" />
                      {t("upgradeTo", { version: availableUpdate.version })}
                    </>
                  )}
                </Button>
              ) : (
                <Button
                  key="check-update"
                  size="sm"
                  onClick={checkForUpdates}
                  disabled={installingUpdate}
                  className="w-[9.5rem] justify-center transition-none"
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("checkUpdate")}
                </Button>
              )}
            </div>

            {!availableUpdate && formattedLastCheckedAt && (
              <p className="text-muted-foreground">
                {t("lastChecked", { time: formattedLastCheckedAt })}
              </p>
            )}

            {updateStatusMessage && (
              <p className="text-muted-foreground">{updateStatusMessage}</p>
            )}

            {availableUpdate && (
              <div className="space-y-2 pt-2 border-t border-border/70">
                <div className="flex items-center justify-between gap-3">
                  <span className="font-medium">
                    {t("upgradableVersion")}：v{availableUpdate.version}
                  </span>
                  {formattedUpdateDate && (
                    <span className="text-muted-foreground text-[11px]">
                      {formattedUpdateDate}
                    </span>
                  )}
                </div>
                <div className="mt-3 max-h-28 overflow-auto rounded-md border bg-background/70 px-3 py-3 leading-6 whitespace-pre-wrap break-words text-muted-foreground">
                  {updateNotes}
                </div>
              </div>
            )}
          </div>

          {updateError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {t("updateError", { message: updateError })}
            </div>
          )}
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Wifi className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("proxyTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("proxyDescription")}
          </p>

          {loadError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {t("loadFailed", { message: loadError })}
            </div>
          )}

          <label className="inline-flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={enabled}
              disabled={saving}
              onChange={(event) => {
                const next = event.target.checked
                setEnabled(next)
                saveProxySettings(next, proxyUrl)
              }}
            />
            {t("enableProxy")}
          </label>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("proxyAddress")}
            </label>
            <Input
              value={proxyUrl}
              onChange={(event) => setProxyUrl(event.target.value)}
              onBlur={() => saveProxySettings(enabled, proxyUrl)}
              placeholder={PROXY_EXAMPLE}
              disabled={saving}
            />
            <p className="text-[11px] text-muted-foreground">
              {t("proxyHint", { example: PROXY_EXAMPLE })}
            </p>
          </div>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Languages className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("languageTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("languageDescription")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("appLanguage")}
            </label>
            <Select
              value={appLanguage}
              onValueChange={(value) => {
                let nextLang: LanguageSelectValue
                if (value === "system") {
                  nextLang = "system"
                } else if (isAppLocale(value)) {
                  nextLang = value
                } else {
                  return
                }
                setAppLanguage(nextLang)
                saveLanguage(nextLang)
              }}
              disabled={savingLanguage || !languageSettingsLoaded}
            >
              <SelectTrigger className="w-full sm:w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="system">
                  {tLanguage("followSystem")}
                </SelectItem>
                <SelectItem value="en">{languageLabels.en}</SelectItem>
                <SelectItem value="zh_cn">{languageLabels.zh_cn}</SelectItem>
                <SelectItem value="zh_tw">{languageLabels.zh_tw}</SelectItem>
                <SelectItem value="ja">{languageLabels.ja}</SelectItem>
                <SelectItem value="ko">{languageLabels.ko}</SelectItem>
                <SelectItem value="es">{languageLabels.es}</SelectItem>
                <SelectItem value="de">{languageLabels.de}</SelectItem>
                <SelectItem value="fr">{languageLabels.fr}</SelectItem>
                <SelectItem value="pt">{languageLabels.pt}</SelectItem>
                <SelectItem value="ar">{languageLabels.ar}</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </section>
      </div>
    </div>
  )
}
