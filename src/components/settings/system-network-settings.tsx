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
import { Github } from "@lobehub/icons"
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Update = any
import { useLocale, useTranslations } from "next-intl"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { toast } from "sonner"
import { useAppI18n } from "@/components/i18n-provider"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  acpClearBinaryCache,
  getAcpCacheInventory,
  getRuntimeDiagnostics,
  getSystemProxySettings,
  updateSystemLanguageSettings,
  updateSystemProxySettings,
} from "@/lib/api"
import { openUrl } from "@/lib/platform"
import { AGENT_LABELS } from "@/lib/types"
import type {
  AcpCacheInventory,
  AgentType,
  AppLocale,
  RuntimeDiagnostics,
} from "@/lib/types"
import {
  checkAppUpdate,
  closeAppUpdate,
  getCurrentAppVersion,
  installAppUpdate,
  normalizeAppUpdateError,
  relaunchApp,
} from "@/lib/updater"
import type { DownloadEvent } from "@/lib/updater"
import { APP_LOCALES } from "@/lib/i18n"

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

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
  const [downloadProgress, setDownloadProgress] = useState<{
    downloaded: number
    total: number | null
    phase: "downloading" | "installing"
  } | null>(null)

  const [appLanguage, setAppLanguage] = useState<LanguageSelectValue>(
    languageSettings.mode === "system" ? "system" : languageSettings.language
  )
  const [diagnostics, setDiagnostics] = useState<RuntimeDiagnostics | null>(
    null
  )
  const [cacheInventory, setCacheInventory] =
    useState<AcpCacheInventory | null>(null)
  const [loadingRuntime, setLoadingRuntime] = useState(false)
  const [runtimeError, setRuntimeError] = useState<string | null>(null)
  const [clearingCache, setClearingCache] = useState<AgentType | "all" | null>(
    null
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
    () => availableUpdate?.body?.trim() ?? "",
    [availableUpdate?.body]
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

  const loadRuntimeData = useCallback(async () => {
    setLoadingRuntime(true)
    setRuntimeError(null)
    try {
      const [runtimeSnapshot, cacheSnapshot] = await Promise.all([
        getRuntimeDiagnostics(),
        getAcpCacheInventory(),
      ])
      setDiagnostics(runtimeSnapshot)
      setCacheInventory(cacheSnapshot)
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setRuntimeError(message)
    } finally {
      setLoadingRuntime(false)
    }
  }, [])

  useEffect(() => {
    loadSettings().catch((err) => {
      console.error("[Settings] load system settings failed:", err)
    })
    loadRuntimeData().catch((err) => {
      console.error("[Settings] load runtime diagnostics failed:", err)
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
    setDownloadProgress(null)

    let downloaded = 0

    try {
      await installAppUpdate(availableUpdate, (event: DownloadEvent) => {
        switch (event.event) {
          case "Started":
            setDownloadProgress({
              downloaded: 0,
              total: event.data.contentLength ?? null,
              phase: "downloading",
            })
            break
          case "Progress":
            downloaded += event.data.chunkLength
            setDownloadProgress((prev) => ({
              downloaded,
              total: prev?.total ?? null,
              phase: "downloading",
            }))
            break
          case "Finished":
            setDownloadProgress((prev) => ({
              downloaded: prev?.downloaded ?? downloaded,
              total: prev?.total ?? null,
              phase: "installing",
            }))
            break
        }
      })
      toast.success(t("installSuccess"))
      await relaunchApp()
    } catch (err) {
      const message = formatUpdateError(err, "install")
      setUpdateError(message)
      toast.error(t("installFailed", { message }))
      console.error("[Settings] install app update failed:", err)
    } finally {
      setInstallingUpdate(false)
      setDownloadProgress(null)
    }
  }, [availableUpdate, formatUpdateError, t])

  const formatTimestamp = useCallback(
    (value?: string | null) => {
      if (!value) return "-"
      const parsed = new Date(value)
      if (Number.isNaN(parsed.getTime())) return value
      return new Intl.DateTimeFormat(locale, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(parsed)
    },
    [locale]
  )

  const clearCache = useCallback(
    async (agentType: AgentType | "all") => {
      if (!cacheInventory) return
      setClearingCache(agentType)
      try {
        if (agentType === "all") {
          await Promise.all(
            cacheInventory.entries.map((entry) =>
              acpClearBinaryCache(entry.agent_type)
            )
          )
        } else {
          await acpClearBinaryCache(agentType)
        }
        await loadRuntimeData()
        toast.success("ACP binary cache cleared")
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(`Failed to clear cache: ${message}`)
      } finally {
        setClearingCache(null)
      }
    },
    [cacheInventory, loadRuntimeData]
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
          <div className="flex items-center justify-between">
            <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
            <Button
              variant="ghost"
              className="size-5 rounded-full"
              onClick={() => openUrl("https://github.com/xintaofei/codeg")}
            >
              <Github className="size-5" />
            </Button>
          </div>
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

            {updateStatusMessage && !downloadProgress && (
              <p className="text-muted-foreground">{updateStatusMessage}</p>
            )}

            {downloadProgress && (
              <div className="space-y-1.5">
                <div className="flex items-center justify-between text-muted-foreground">
                  <span>
                    {downloadProgress.phase === "downloading"
                      ? t("downloading")
                      : t("updating")}
                  </span>
                  <span>
                    {formatBytes(downloadProgress.downloaded)}
                    {downloadProgress.total
                      ? ` / ${formatBytes(downloadProgress.total)}`
                      : ""}
                  </span>
                </div>
                <div className="h-1.5 rounded-full bg-muted overflow-hidden">
                  <div
                    className="h-full rounded-full bg-primary transition-all duration-300"
                    style={{
                      width:
                        downloadProgress.total && downloadProgress.total > 0
                          ? `${Math.min(100, (downloadProgress.downloaded / downloadProgress.total) * 100)}%`
                          : "30%",
                    }}
                  />
                </div>
              </div>
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
                <div
                  className={
                    "mt-3 max-h-72 overflow-auto rounded-md border bg-background/70 px-3 py-3 leading-6 break-words text-muted-foreground " +
                    "[&_h1]:text-sm [&_h1]:font-semibold [&_h1]:mb-2 [&_h1]:text-foreground " +
                    "[&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-2 [&_h2]:text-foreground " +
                    "[&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 [&_h3]:text-foreground " +
                    "[&_p]:mb-2 [&_p:last-child]:mb-0 " +
                    "[&_ul]:list-disc [&_ul]:pl-5 [&_ul]:mb-2 [&_ol]:list-decimal [&_ol]:pl-5 [&_ol]:mb-2 [&_li]:mb-1 " +
                    "[&_code]:font-mono [&_code]:text-[11px] [&_code]:bg-muted [&_code]:rounded [&_code]:px-1 " +
                    "[&_pre]:bg-muted [&_pre]:rounded-md [&_pre]:p-2 [&_pre]:overflow-x-auto [&_pre]:mb-2 " +
                    "[&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2 " +
                    "[&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground/80 " +
                    "[&_hr]:my-2 [&_hr]:border-border"
                  }
                >
                  {updateNotes ? (
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>
                      {updateNotes}
                    </ReactMarkdown>
                  ) : (
                    t("none")
                  )}
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
          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1">
              <h2 className="text-sm font-semibold">Runtime Diagnostics</h2>
              <p className="text-xs text-muted-foreground leading-5">
                ACP connections, web clients, build consistency and recent
                runtime events.
              </p>
            </div>
            <Button
              size="sm"
              variant="outline"
              onClick={() => loadRuntimeData()}
              disabled={loadingRuntime}
            >
              {loadingRuntime ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
              Refresh
            </Button>
          </div>

          {runtimeError && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {runtimeError}
            </div>
          )}

          {diagnostics && (
            <>
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-1">
                  <div className="font-medium">Build</div>
                  <div>Status: {diagnostics.build?.status ?? "-"}</div>
                  <div className="text-muted-foreground">
                    {diagnostics.build?.message ?? "Unavailable"}
                  </div>
                </div>
                <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-1">
                  <div className="font-medium">Security</div>
                  <div>
                    Remote access:{" "}
                    {diagnostics.security?.allow_remote_access ? "yes" : "no"}
                  </div>
                  <div>
                    Auth:{" "}
                    {diagnostics.security?.auth_enabled
                      ? "enabled"
                      : "disabled"}
                  </div>
                  <div className="text-muted-foreground">
                    {diagnostics.security?.insecure
                      ? "Insecure remote exposure detected"
                      : "No insecure startup warning"}
                  </div>
                </div>
                <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-1">
                  <div className="font-medium">Sessions</div>
                  <div>ACP: {diagnostics.connections.length}</div>
                  <div>Terminals: {diagnostics.terminals.length}</div>
                  <div>Web clients: {diagnostics.web_clients.length}</div>
                </div>
                <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-1">
                  <div className="font-medium">Circuit Breaker</div>
                  <div>
                    {diagnostics.connection_limits.breaker_open
                      ? "Open"
                      : "Closed"}
                  </div>
                  <div>
                    Failures: {diagnostics.connection_limits.recent_failures}/
                    {diagnostics.connection_limits.breaker_threshold}
                  </div>
                  <div className="text-muted-foreground">
                    Global limit: {diagnostics.connection_limits.current_total}/
                    {diagnostics.connection_limits.global_limit}
                  </div>
                </div>
              </div>

              <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-2">
                <div className="font-medium">Active ACP connections</div>
                {diagnostics.connections.length === 0 ? (
                  <div className="text-muted-foreground">
                    No active ACP connections.
                  </div>
                ) : (
                  diagnostics.connections.map((connection) => (
                    <div
                      key={connection.id}
                      className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b last:border-b-0 pb-2 last:pb-0"
                    >
                      <span className="font-medium">
                        {AGENT_LABELS[connection.agent_type]}
                      </span>
                      <span className="text-muted-foreground">
                        {connection.status}
                      </span>
                      <span className="text-muted-foreground">
                        owner {connection.owner_window_label}
                      </span>
                      <span className="text-muted-foreground">
                        {connection.working_dir ?? "-"}
                      </span>
                    </div>
                  ))
                )}
              </div>

              <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-2">
                <div className="font-medium">Recent runtime logs</div>
                {diagnostics.recent_logs.length === 0 ? (
                  <div className="text-muted-foreground">
                    No recent runtime logs.
                  </div>
                ) : (
                  diagnostics.recent_logs.slice(-12).map((entry, index) => (
                    <div
                      key={`${entry.timestamp}-${index}`}
                      className="grid gap-1 border-b last:border-b-0 pb-2 last:pb-0"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium uppercase">
                          {entry.level}
                        </span>
                        <span className="text-muted-foreground">
                          {entry.scope}
                        </span>
                        <span className="text-muted-foreground">
                          {formatTimestamp(entry.timestamp)}
                        </span>
                      </div>
                      <div>{entry.message}</div>
                    </div>
                  ))
                )}
              </div>
            </>
          )}
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1">
              <h2 className="text-sm font-semibold">Cache Governance</h2>
              <p className="text-xs text-muted-foreground leading-5">
                Inspect and clear ACP binary caches without leaving Settings.
              </p>
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={!cacheInventory || clearingCache !== null}
              onClick={() => clearCache("all")}
            >
              {clearingCache === "all" && (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              )}
              Clear all
            </Button>
          </div>

          {cacheInventory && (
            <div className="rounded-md border bg-muted/20 px-3 py-3 text-xs space-y-3">
              <div className="flex flex-wrap items-center gap-3 text-muted-foreground">
                <span>
                  Total size: {formatBytes(cacheInventory.total_size_bytes)}
                </span>
                <span>Root: {cacheInventory.root_path}</span>
                <span>
                  Generated: {formatTimestamp(cacheInventory.generated_at)}
                </span>
              </div>

              {cacheInventory.entries.map((entry) => (
                <div
                  key={entry.agent_type}
                  className="flex flex-col gap-2 rounded-md border bg-background/70 px-3 py-3"
                >
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="space-y-1">
                      <div className="font-medium">{entry.label}</div>
                      <div className="text-muted-foreground">
                        {formatBytes(entry.size_bytes)} · {entry.file_count}{" "}
                        files
                      </div>
                      <div className="text-muted-foreground truncate">
                        {entry.path}
                      </div>
                    </div>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={clearingCache !== null}
                      onClick={() => clearCache(entry.agent_type)}
                    >
                      {clearingCache === entry.agent_type && (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      )}
                      Clear
                    </Button>
                  </div>
                  <div className="text-muted-foreground">
                    Installed: {entry.installed_version ?? "-"} · Recommended:{" "}
                    {entry.recommended_version ?? "-"}
                  </div>
                  {entry.versions.length > 0 && (
                    <div className="text-muted-foreground">
                      Versions: {entry.versions.join(", ")}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
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
    </ScrollArea>
  )
}
