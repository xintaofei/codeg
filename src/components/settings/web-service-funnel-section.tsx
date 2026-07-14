"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Check, Copy, ExternalLink } from "lucide-react"
import { openUrl } from "@/lib/platform"
import { toast } from "sonner"

import { Switch } from "@/components/ui/switch"
import {
  getTailscaleFunnelStatus,
  openTailscaleLogin,
  setTailscaleFunnelEnabled,
  type TailscaleFunnelStatus,
} from "@/lib/api"
import { copyTextToClipboard } from "@/lib/utils"
import { useCopiedFlag } from "@/hooks/use-copied-flag"

const TRANSITIONAL = new Set([
  "starting",
  "needs_login",
  "connecting",
  "online",
  "funnel_enabling",
  "stopping",
])

function errorMessage(
  t: ReturnType<typeof useTranslations>,
  status: TailscaleFunnelStatus | null
): string | null {
  if (!status) return null
  const key = status.errorKey
  if (key === "tailscale.sidecar_missing") return t("funnelErrors.sidecarMissing")
  if (key === "tailscale.start_failed") return t("funnelErrors.startFailed")
  if (key === "tailscale.login_timeout") return t("funnelErrors.loginTimeout")
  if (key === "tailscale.funnel_denied") return t("funnelErrors.funnelDenied")
  if (key === "tailscale.funnel_failed") return t("funnelErrors.funnelFailed")
  if (key === "tailscale.authkey_required")
    return t("funnelErrors.authkeyRequired")
  if (key === "tailscale.unsupported") return t("funnelErrors.unsupported")
  return status.lastError ?? null
}

export function WebServiceFunnelSection({
  webRunning,
}: {
  webRunning: boolean
}) {
  const t = useTranslations("WebServiceSettings")
  const [status, setStatus] = useState<TailscaleFunnelStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [copied, markCopied] = useCopiedFlag()
  const [error, setError] = useState("")

  const refresh = useCallback(async () => {
    try {
      const next = await getTailscaleFunnelStatus()
      setStatus(next)
      setError("")
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh, webRunning])

  useEffect(() => {
    if (!status || !TRANSITIONAL.has(status.state)) return
    const id = window.setInterval(() => {
      void refresh()
    }, 1500)
    return () => window.clearInterval(id)
  }, [status, refresh])

  const mappedError = useMemo(() => errorMessage(t, status), [status, t])

  async function handleToggle(enabled: boolean) {
    setBusy(true)
    setError("")
    try {
      const next = await setTailscaleFunnelEnabled(enabled)
      setStatus(next)
      if (next.loginUrl && next.state === "needs_login") {
        try {
          await openUrl(next.loginUrl)
        } catch {
          // ignore opener failures; login button remains available
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function handleLogin() {
    setBusy(true)
    try {
      const res = await openTailscaleLogin()
      const url = res.loginUrl || status?.loginUrl
      if (url) {
        await openUrl(url)
      } else {
        toast.error(t("funnelErrors.loginTimeout"))
      }
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  async function handleCopy() {
    if (!status?.funnelUrl) return
    try {
      const ok = await copyTextToClipboard(status.funnelUrl)
      if (!ok) {
        toast.error(t("copyFailed"))
        return
      }
      markCopied()
    } catch {
      toast.error(t("copyFailed"))
    }
  }

  const supported = status?.supported ?? true
  const enabled = status?.enabled ?? false
  const switchDisabled = busy || !webRunning || supported === false

  return (
    <div className="space-y-3 rounded-md border border-border/70 p-3">
      <div>
        <div className="text-sm font-medium">{t("funnelTitle")}</div>
        <p className="mt-1 text-xs text-muted-foreground">
          {t("funnelDescription")}
        </p>
      </div>

      <div className="flex items-center gap-4">
        <label className="w-28 text-sm font-medium">{t("funnelEnable")}</label>
        <div className="flex min-w-0 items-center gap-3">
          <Switch
            checked={enabled}
            disabled={switchDisabled}
            onCheckedChange={(checked) => {
              void handleToggle(checked)
            }}
            aria-label={t("funnelEnable")}
          />
          <span className="text-sm text-muted-foreground">
            {t("funnelEnableHint")}
          </span>
        </div>
      </div>

      <div className="flex items-center gap-4">
        <label className="w-28 text-sm font-medium">{t("funnelState")}</label>
        <div className="flex items-center gap-2">
          <span className="inline-flex items-center rounded-full border px-2 py-0.5 text-xs">
            {status?.state ?? "stopped"}
          </span>
          {status?.hostname ? (
            <span className="text-xs text-muted-foreground">
              {status.hostname}
              {status.ipv4 ? ` · ${status.ipv4}` : ""}
            </span>
          ) : null}
        </div>
      </div>

      {status?.state === "needs_login" || status?.loginUrl ? (
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => void handleLogin()}
            disabled={busy || !status?.loginUrl}
            className="inline-flex h-8 items-center gap-1.5 rounded-md border border-input bg-background px-3 text-xs font-medium hover:bg-accent disabled:opacity-50"
          >
            <ExternalLink className="h-3.5 w-3.5" />
            {t("funnelLogin")}
          </button>
        </div>
      ) : null}

      {status?.funnelUrl ? (
        <div className="space-y-1">
          <div className="text-xs font-medium text-muted-foreground">
            {t("funnelUrl")}
          </div>
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1 text-xs">
              {status.funnelUrl}
            </code>
            <button
              type="button"
              onClick={() => void handleCopy()}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-input bg-background hover:bg-accent"
              aria-label={t("funnelUrl")}
            >
              {copied ? (
                <Check className="h-3.5 w-3.5" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
            </button>
            <button
              type="button"
              onClick={() => void openUrl(status.funnelUrl!)}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-input bg-background hover:bg-accent"
            >
              <ExternalLink className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      ) : null}

      {!supported ? (
        <p className="text-sm text-destructive">{t("funnelUnsupported")}</p>
      ) : null}
      {(mappedError || error) && (
        <p className="text-sm text-destructive">{mappedError || error}</p>
      )}

      <p className="text-xs text-muted-foreground">{t("funnelTokenNote")}</p>
      <p className="text-xs text-muted-foreground">
        {t("funnelPrivateNodeNote")}
      </p>
    </div>
  )
}
