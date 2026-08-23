"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  AlertTriangle,
  CheckCircle2,
  Download,
  Globe2,
  Loader2,
  RefreshCw,
} from "lucide-react"
import { useTranslations } from "next-intl"

import { SettingCard } from "@/components/shared/setting-card"
import { SettingsError } from "@/components/shared/settings-section"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Switch } from "@/components/ui/switch"
import {
  doctorBrowserRuntime,
  getBrowserRuntimeSettings,
  getBrowserRuntimeStatus,
  recoverBrowserRuntime,
  restartBrowserRuntime,
  startBrowserRuntime,
  updateBrowserRuntimeSettings,
  type BrowserRuntimeSettings,
  type BrowserRuntimeStatus,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import { openUrl, subscribe } from "@/lib/platform"
import { cn } from "@/lib/utils"

const CHROME_DOWNLOAD_URL = "https://www.google.com/chrome/"

interface BrowserAvailability {
  detected: boolean
  browser: string | null
}

interface RuntimeTruth {
  status: BrowserRuntimeStatus
  availability: BrowserAvailability | null
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null
}

export function browserAvailabilityFromDoctor(
  report: unknown
): BrowserAvailability {
  const root = asRecord(report)
  const payload = asRecord(root?.data) ?? root
  const checks = Array.isArray(payload?.checks) ? payload.checks : []
  const browserCheck = checks
    .map(asRecord)
    .find((check) => check?.name === "browser")

  if (browserCheck?.ok !== true) {
    return { detected: false, browser: null }
  }

  const detail =
    typeof browserCheck.detail === "string" ? browserCheck.detail : ""
  const executable = detail.toLowerCase()
  const browser = executable.includes("msedge")
    ? "Microsoft Edge"
    : executable.includes("chromium")
      ? "Chromium"
      : executable.includes("chrome")
        ? "Google Chrome"
        : detail || null

  return { detected: true, browser }
}

async function probeExternalBrowser(): Promise<BrowserAvailability> {
  try {
    return browserAvailabilityFromDoctor(await doctorBrowserRuntime())
  } catch {
    return { detected: false, browser: null }
  }
}

async function readRuntimeTruth(
  settings: BrowserRuntimeSettings
): Promise<RuntimeTruth> {
  const [status, availability] = await Promise.all([
    getBrowserRuntimeStatus(),
    settings.backend === "external"
      ? probeExternalBrowser()
      : Promise.resolve(null),
  ])
  return { status, availability }
}

export function BrowserSettings() {
  const t = useTranslations("BrowserSettings")
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [settings, setSettings] = useState<BrowserRuntimeSettings | null>(null)
  const [status, setStatus] = useState<BrowserRuntimeStatus | null>(null)
  const [availability, setAvailability] = useState<BrowserAvailability | null>(
    null
  )
  const [pendingSetting, setPendingSetting] = useState<
    "enabled" | "backend" | null
  >(null)
  const [busyAction, setBusyAction] = useState(false)
  const [operationError, setOperationError] = useState<string | null>(null)
  const settingsMutationPending = useRef(false)

  const load = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const nextSettings = await getBrowserRuntimeSettings()
      const truth = await readRuntimeTruth(nextSettings)
      setSettings(nextSettings)
      setStatus(truth.status)
      setAvailability(truth.availability)
    } catch (error) {
      setSettings(null)
      setStatus(null)
      setAvailability(null)
      setLoadError(toErrorMessage(error))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | null = null
    void subscribe<BrowserRuntimeStatus>("browser://status", (next) => {
      if (!disposed) setStatus(next)
    }).then((unsubscribe) => {
      if (disposed) unsubscribe()
      else unlisten = unsubscribe
    })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const reconcileAuthoritative = useCallback(
    async (nextSettings: BrowserRuntimeSettings) => {
      const truth = await readRuntimeTruth(nextSettings)
      setSettings(nextSettings)
      setStatus(truth.status)
      setAvailability(truth.availability)
    },
    []
  )

  const updateEnabled = useCallback(
    async (enabled: boolean) => {
      if (!settings || settingsMutationPending.current) return
      settingsMutationPending.current = true
      setPendingSetting("enabled")
      setOperationError(null)
      try {
        const confirmed = await updateBrowserRuntimeSettings({
          ...settings,
          enabled,
          autoStart: enabled,
        })
        await reconcileAuthoritative(confirmed)
      } catch (error) {
        setOperationError(toErrorMessage(error))
      } finally {
        settingsMutationPending.current = false
        setPendingSetting(null)
      }
    },
    [reconcileAuthoritative, settings]
  )

  const updateBackend = useCallback(
    async (backend: BrowserRuntimeSettings["backend"]) => {
      if (
        !settings ||
        settings.backend === backend ||
        settingsMutationPending.current
      ) {
        return
      }

      const previousSettings = settings
      const previousStatus = status
      const previousAvailability = availability
      settingsMutationPending.current = true
      setPendingSetting("backend")
      setOperationError(null)
      setSettings({ ...settings, backend })
      if (backend === "embedded") setAvailability(null)

      try {
        const confirmed = await updateBrowserRuntimeSettings({
          ...settings,
          backend,
        })
        await reconcileAuthoritative(confirmed)
      } catch (error) {
        setOperationError(toErrorMessage(error))
        try {
          const confirmed = await getBrowserRuntimeSettings()
          await reconcileAuthoritative(confirmed)
        } catch {
          setSettings(previousSettings)
          setStatus(previousStatus)
          setAvailability(previousAvailability)
        }
      } finally {
        settingsMutationPending.current = false
        setPendingSetting(null)
      }
    },
    [availability, reconcileAuthoritative, settings, status]
  )

  const runRuntimeAction = useCallback(
    async (action: () => Promise<BrowserRuntimeStatus>) => {
      setBusyAction(true)
      setOperationError(null)
      try {
        setStatus(await action())
        if (settings?.backend === "external") {
          setAvailability(await probeExternalBrowser())
        }
      } catch (error) {
        setOperationError(toErrorMessage(error))
        try {
          setStatus(await getBrowserRuntimeStatus())
        } catch {
          // The operation error remains the authoritative user-visible result.
        }
      } finally {
        setBusyAction(false)
      }
    },
    [settings?.backend]
  )

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
        <Loader2
          className="size-4 animate-spin motion-reduce:animate-none"
          aria-hidden="true"
        />
        {t("loading")}
      </div>
    )
  }

  if (!settings || !status) {
    return (
      <div className="mx-auto flex h-full w-full max-w-3xl flex-col justify-center gap-3 p-4 sm:p-6">
        <SettingsError>
          {t("loadFailed", { message: loadError ?? t("unavailable") })}
        </SettingsError>
        <Button
          className="self-start"
          variant="outline"
          onClick={() => void load()}
        >
          <RefreshCw aria-hidden="true" />
          {t("actions.retry")}
        </Button>
      </div>
    )
  }

  const transitional =
    status.state === "starting" || status.state === "recovering"
  const settingsPending = pendingSetting !== null
  const embeddedHealthMatches =
    settings.backend === "embedded" && status.backend === "embedded_webview2"
  const externalDetected = availability?.detected === true

  return (
    <ScrollArea className="h-full">
      <main className="mx-auto w-full max-w-3xl space-y-3 p-4 sm:p-6">
        <SettingCard data-testid="browser-automation-block">
          <div className="flex items-start justify-between gap-4 p-4">
            <div className="flex min-w-0 gap-3">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background">
                <Globe2
                  className="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
              </span>
              <div className="min-w-0 space-y-1">
                <h1 className="text-sm font-semibold">
                  {t("browserBlockTitle")}
                </h1>
                <p className="text-xs leading-5 text-muted-foreground">
                  {t("browserBlockDescription")}
                </p>
              </div>
            </div>
            <Switch
              id="browser-runtime-enabled"
              aria-label={t("enabled")}
              checked={settings.enabled}
              disabled={settingsPending}
              onCheckedChange={(enabled) => void updateEnabled(enabled)}
            />
          </div>

          <div className="space-y-2 p-4">
            <div className="text-xs font-medium">{t("backendChoice")}</div>
            <div
              role="tablist"
              aria-label={t("backendChoice")}
              className="grid grid-cols-2 rounded-lg bg-muted p-1"
            >
              {(["embedded", "external"] as const).map((backend) => (
                <button
                  key={backend}
                  type="button"
                  role="tab"
                  aria-selected={settings.backend === backend}
                  disabled={settingsPending || transitional}
                  className={cn(
                    "rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
                    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    "disabled:cursor-not-allowed disabled:opacity-50",
                    settings.backend === backend
                      ? "bg-background text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground"
                  )}
                  onClick={() => void updateBackend(backend)}
                >
                  {t(`backends.${backend}`)}
                </button>
              ))}
            </div>
          </div>

          {settings.backend === "embedded" && embeddedHealthMatches ? (
            <div
              className="flex items-center justify-between gap-3 p-4"
              aria-live="polite"
            >
              <div className="flex min-w-0 items-center gap-2 text-xs">
                {status.state === "ready" ? (
                  <CheckCircle2
                    className="size-4 shrink-0 text-green-600 dark:text-green-400"
                    aria-hidden="true"
                  />
                ) : status.state === "error" ? (
                  <AlertTriangle
                    className="size-4 shrink-0 text-destructive"
                    aria-hidden="true"
                  />
                ) : (
                  <Loader2
                    className={cn(
                      "size-4 shrink-0 text-muted-foreground",
                      transitional && "animate-spin motion-reduce:animate-none"
                    )}
                    aria-hidden="true"
                  />
                )}
                <span>
                  {status.state === "ready"
                    ? t("embeddedReady")
                    : status.state === "error"
                      ? t("embeddedError", {
                          code: status.lastErrorCode ?? t("unavailable"),
                        })
                      : t("embeddedStopped")}
                </span>
              </div>
              {status.state === "ready" ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busyAction || transitional || !status.installed}
                  onClick={() => void runRuntimeAction(restartBrowserRuntime)}
                >
                  {busyAction ? (
                    <Loader2 className="animate-spin motion-reduce:animate-none" />
                  ) : (
                    <RefreshCw />
                  )}
                  {t("actions.reconnect")}
                </Button>
              ) : status.state === "error" ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busyAction || transitional || !status.installed}
                  onClick={() => void runRuntimeAction(recoverBrowserRuntime)}
                >
                  {busyAction ? (
                    <Loader2 className="animate-spin motion-reduce:animate-none" />
                  ) : (
                    <RefreshCw />
                  )}
                  {t("actions.recover")}
                </Button>
              ) : null}
            </div>
          ) : null}

          {settings.backend === "external" ? (
            <div
              className="flex items-center justify-between gap-3 p-4"
              aria-live="polite"
            >
              <div className="flex min-w-0 items-center gap-2 text-xs">
                {externalDetected ? (
                  <CheckCircle2
                    className="size-4 shrink-0 text-green-600 dark:text-green-400"
                    aria-hidden="true"
                  />
                ) : (
                  <AlertTriangle
                    className="size-4 shrink-0 text-amber-600 dark:text-amber-400"
                    aria-hidden="true"
                  />
                )}
                <span>
                  {externalDetected
                    ? t("externalDetected", {
                        browser: availability.browser ?? t("unavailable"),
                      })
                    : t("externalNotDetected")}
                </span>
              </div>
              {externalDetected ? (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={!settings.enabled || busyAction || transitional}
                  onClick={() =>
                    void runRuntimeAction(
                      status.state === "error"
                        ? recoverBrowserRuntime
                        : startBrowserRuntime
                    )
                  }
                >
                  {busyAction ? (
                    <Loader2 className="animate-spin motion-reduce:animate-none" />
                  ) : (
                    <Globe2 />
                  )}
                  {t("actions.openManagedBrowser")}
                </Button>
              ) : (
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void openUrl(CHROME_DOWNLOAD_URL)}
                >
                  <Download />
                  {t("actions.getChrome")}
                </Button>
              )}
            </div>
          ) : null}

          {operationError ? (
            <div className="p-4">
              <SettingsError>
                {t("operationFailed", { message: operationError })}
              </SettingsError>
            </div>
          ) : null}
        </SettingCard>

        <p className="px-1 text-xs leading-5 text-muted-foreground">
          {t("futureSessionHint")}
        </p>
      </main>
    </ScrollArea>
  )
}
