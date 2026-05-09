"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import {
  ArrowUpCircle,
  CheckCircle2,
  Languages,
  Loader2,
  MonitorCog,
  RefreshCw,
  SquareTerminal,
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
  getAvailableTerminalShells,
  getSystemProxySettings,
  getSystemRenderingSettings,
  getSystemTerminalSettings,
  probeTerminalShellPath,
  updateSystemLanguageSettings,
  updateSystemProxySettings,
  updateSystemRenderingSettings,
  updateSystemTerminalSettings,
} from "@/lib/api"
import { isDesktop, openUrl } from "@/lib/platform"
import type {
  AppLocale,
  AvailableTerminalShells,
  TerminalShellOption,
} from "@/lib/types"
import { usePlatform } from "@/hooks/use-platform"
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
import { toErrorMessage } from "@/lib/app-error"

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

const PROXY_EXAMPLE = "http://127.0.0.1:7890"
const APP_LANGUAGE_VALUES = APP_LOCALES

type LanguageSelectValue = "system" | AppLocale

const TERMINAL_SHELL_OPTION_SYSTEM = "system"
const TERMINAL_SHELL_OPTION_CUSTOM = "custom"

function isAppLocale(value: string): value is AppLocale {
  return APP_LANGUAGE_VALUES.includes(value as AppLocale)
}

/// Pick which dropdown row matches a stored `default_shell` value:
/// - null  → "system"
/// - matches a predefined option's `value` → that option's id
/// - anything else → "custom" (user-supplied path)
function resolveSelectedShellId(
  storedShell: string | null,
  options: TerminalShellOption[]
): string {
  if (!storedShell) return TERMINAL_SHELL_OPTION_SYSTEM
  const matched = options.find(
    (opt) => opt.value !== null && opt.value === storedShell
  )
  return matched?.id ?? TERMINAL_SHELL_OPTION_CUSTOM
}

type UpdateAction = "check" | "install"

// Captured the first time settings page loads: represents the value that the
// running webview process was launched with. Survives settings-shell remounts
// so the "Restart now" banner doesn't vanish if the user navigates away and
// back without restarting.
let processStartDisableHwAccel: boolean | null = null

export function SystemNetworkSettings() {
  const t = useTranslations("SystemSettings")
  // Backend-driven label keys are dynamic strings, so we widen `t` for that
  // single call site rather than casting at every use.
  const tDynamic = t as unknown as (key: string) => string
  const tLanguage = useTranslations("Language")
  const locale = useLocale()
  const { languageSettings, languageSettingsLoaded, setLanguageSettings } =
    useAppI18n()
  const { isWindows } = usePlatform()
  const renderingSettingsLoadable = isDesktop()
  const renderingSectionVisible = renderingSettingsLoadable && isWindows

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [savingLanguage, setSavingLanguage] = useState(false)
  const [savingTerminal, setSavingTerminal] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [proxyUrl, setProxyUrl] = useState("")
  const [proxyUrlError, setProxyUrlError] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [disableHwAccel, setDisableHwAccel] = useState(false)
  const [savingRendering, setSavingRendering] = useState(false)
  const [persistedDisableHwAccel, setPersistedDisableHwAccel] = useState(false)
  const [processStartLoaded, setProcessStartLoaded] = useState(
    processStartDisableHwAccel !== null
  )
  const renderingDirty =
    processStartLoaded && persistedDisableHwAccel !== processStartDisableHwAccel
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
  const [availableShells, setAvailableShells] =
    useState<AvailableTerminalShells | null>(null)
  const [selectedShellId, setSelectedShellId] = useState<string>(
    TERMINAL_SHELL_OPTION_SYSTEM
  )
  const [customShellPath, setCustomShellPath] = useState<string>("")
  const [customPathExists, setCustomPathExists] = useState<boolean | null>(null)

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
      const [
        proxySettings,
        terminalSettings,
        terminalShells,
        version,
        renderingSettings,
      ] = await Promise.all([
        getSystemProxySettings(),
        getSystemTerminalSettings(),
        getAvailableTerminalShells(),
        getCurrentAppVersion(),
        renderingSettingsLoadable
          ? getSystemRenderingSettings()
          : Promise.resolve(null),
      ])

      setEnabled(proxySettings.enabled)
      setProxyUrl(proxySettings.proxy_url ?? "")
      setAvailableShells(terminalShells)
      const initialId = resolveSelectedShellId(
        terminalSettings.default_shell,
        terminalShells.options
      )
      setSelectedShellId(initialId)
      if (initialId === TERMINAL_SHELL_OPTION_CUSTOM) {
        setCustomShellPath(terminalSettings.default_shell ?? "")
        setCustomPathExists(
          terminalSettings.default_shell
            ? await probeTerminalShellPath(terminalSettings.default_shell)
            : null
        )
      } else {
        setCustomShellPath("")
        setCustomPathExists(null)
      }
      setCurrentVersion(version)
      if (renderingSettings) {
        const value = renderingSettings.disable_hardware_acceleration
        setDisableHwAccel(value)
        setPersistedDisableHwAccel(value)
        if (processStartDisableHwAccel === null) {
          processStartDisableHwAccel = value
          setProcessStartLoaded(true)
        }
      }
    } catch (err) {
      const message = toErrorMessage(err)
      setLoadError(message)
      console.error("[Settings] load system settings failed:", err)
    } finally {
      setLoading(false)
    }
  }, [renderingSettingsLoadable])

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

  const persistTerminalShell = useCallback(
    async (defaultShell: string | null) => {
      setSavingTerminal(true)
      try {
        const result = await updateSystemTerminalSettings({
          default_shell: defaultShell,
        })
        // Re-fetch options to refresh `exists` flags (e.g. user just installed
        // pwsh, or backend filter dropped a cross-platform stale value).
        const refreshedShells = await getAvailableTerminalShells()
        setAvailableShells(refreshedShells)
        const nextSelectedId = resolveSelectedShellId(
          result.default_shell,
          refreshedShells.options
        )
        setSelectedShellId(nextSelectedId)
        if (nextSelectedId === TERMINAL_SHELL_OPTION_CUSTOM) {
          setCustomShellPath(result.default_shell ?? "")
          setCustomPathExists(
            result.default_shell
              ? await probeTerminalShellPath(result.default_shell)
              : null
          )
        } else {
          setCustomShellPath("")
          setCustomPathExists(null)
        }
      } catch (err) {
        const message = toErrorMessage(err)
        toast.error(t("terminalSaveFailed", { message }))
      } finally {
        setSavingTerminal(false)
      }
    },
    [t]
  )

  const onShellSelectChange = useCallback(
    (nextId: string) => {
      setSelectedShellId(nextId)
      if (nextId === TERMINAL_SHELL_OPTION_CUSTOM) {
        // Don't persist yet — wait for user to type a path and press Save.
        setCustomShellPath("")
        setCustomPathExists(null)
        return
      }
      const matched = availableShells?.options.find((opt) => opt.id === nextId)
      void persistTerminalShell(matched?.value ?? null)
    },
    [availableShells, persistTerminalShell]
  )

  const onCustomPathSave = useCallback(() => {
    const trimmed = customShellPath.trim()
    if (!trimmed) return
    void persistTerminalShell(trimmed)
  }, [customShellPath, persistTerminalShell])

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
        const message = toErrorMessage(err)
        toast.error(t("saveFailed", { message }))
      } finally {
        setSaving(false)
      }
    },
    [t]
  )

  const saveRenderingSettings = useCallback(
    async (next: boolean, prev: boolean) => {
      setSavingRendering(true)
      try {
        const result = await updateSystemRenderingSettings({
          disable_hardware_acceleration: next,
        })
        setDisableHwAccel(result.disable_hardware_acceleration)
        setPersistedDisableHwAccel(result.disable_hardware_acceleration)
      } catch (err) {
        setDisableHwAccel(prev)
        const message = toErrorMessage(err)
        toast.error(t("renderingSaveFailed", { message }))
      } finally {
        setSavingRendering(false)
      }
    },
    [t]
  )

  const restartNow = useCallback(async () => {
    try {
      await relaunchApp()
    } catch (err) {
      const message = toErrorMessage(err)
      toast.error(t("restartFailed", { message }))
    }
  }, [t])

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
        const message = toErrorMessage(err)
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
                isDesktop() ? (
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
                    size="sm"
                    onClick={() =>
                      openUrl(
                        "https://github.com/xintaofei/codeg/releases/latest"
                      )
                    }
                  >
                    <ArrowUpCircle className="h-3.5 w-3.5" />
                    {t("viewRelease", { version: availableUpdate.version })}
                  </Button>
                )
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
            <SquareTerminal className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("terminalTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("terminalDescription")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("defaultTerminalShell")}
            </label>
            <Select
              value={selectedShellId}
              onValueChange={onShellSelectChange}
              disabled={savingTerminal || !availableShells}
            >
              <SelectTrigger className="w-full sm:w-64">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {availableShells?.options.map((opt) => (
                  <SelectItem key={opt.id} value={opt.id}>
                    <span className="flex items-center gap-2">
                      <span>{tDynamic(opt.label_key)}</span>
                      {!opt.exists && !opt.accepts_custom_path && (
                        <span className="text-[10px] text-muted-foreground">
                          ({t("terminalShellNotInstalled")})
                        </span>
                      )}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {availableShells && (
              <p className="text-[11px] text-muted-foreground">
                {t("terminalCurrentShell", {
                  path: availableShells.resolved_shell,
                })}
              </p>
            )}

            {selectedShellId === TERMINAL_SHELL_OPTION_CUSTOM && (
              <div className="space-y-2 pt-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("terminalShellCustomPath")}
                </label>
                <div className="flex gap-2">
                  <Input
                    value={customShellPath}
                    onChange={(event) => {
                      setCustomShellPath(event.target.value)
                      setCustomPathExists(null)
                    }}
                    placeholder={t("terminalShellCustomPlaceholder")}
                    disabled={savingTerminal}
                    className="flex-1"
                  />
                  <Button
                    size="sm"
                    onClick={onCustomPathSave}
                    disabled={savingTerminal || !customShellPath.trim()}
                  >
                    {t("terminalShellCustomSave")}
                  </Button>
                </div>
                {customPathExists === false && customShellPath.trim() && (
                  <p className="text-[11px] text-amber-500">
                    {t("terminalShellNotFoundWarning")}
                  </p>
                )}
                <p className="text-[11px] text-muted-foreground">
                  {t("terminalShellCustomHint")}
                </p>
              </div>
            )}
          </div>
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
                if (next && !proxyUrl.trim()) {
                  setProxyUrlError(t("proxyRequired"))
                  return
                }
                setProxyUrlError(null)
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
              onChange={(event) => {
                setProxyUrl(event.target.value)
                if (event.target.value.trim()) setProxyUrlError(null)
              }}
              onBlur={() => {
                if (enabled && !proxyUrl.trim()) {
                  setProxyUrlError(t("proxyRequired"))
                  return
                }
                setProxyUrlError(null)
                saveProxySettings(enabled, proxyUrl)
              }}
              placeholder={PROXY_EXAMPLE}
              disabled={saving}
              aria-invalid={proxyUrlError ? true : undefined}
            />
            {proxyUrlError && (
              <p className="text-[11px] text-destructive">{proxyUrlError}</p>
            )}
            <p className="text-[11px] text-muted-foreground">
              {t("proxyHint", { example: PROXY_EXAMPLE })}
            </p>
          </div>
        </section>

        {renderingSectionVisible && (
          <section className="rounded-xl border bg-card p-4 space-y-4">
            <div className="flex items-center gap-2">
              <MonitorCog className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("renderingTitle")}</h2>
            </div>

            <p className="text-xs text-muted-foreground leading-5">
              {t("renderingDescription")}
            </p>

            <label className="inline-flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={disableHwAccel}
                disabled={savingRendering}
                onChange={(event) => {
                  const next = event.target.checked
                  const prev = disableHwAccel
                  setDisableHwAccel(next)
                  saveRenderingSettings(next, prev)
                }}
              />
              {t("disableHardwareAcceleration")}
            </label>

            {renderingDirty && (
              <div className="flex items-center justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-xs">
                <span className="text-muted-foreground">
                  {t("restartRequired")}
                </span>
                <Button
                  size="sm"
                  onClick={restartNow}
                  disabled={savingRendering}
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("restartNow")}
                </Button>
              </div>
            )}
          </section>
        )}

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
