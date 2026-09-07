"use client"

import { ExternalLink, RotateCw, ShieldAlert, X } from "lucide-react"
import { useTranslations } from "next-intl"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import { browserReload } from "@/lib/browser/browser-api"
import {
  setBrowserTabNotice,
  useBrowserTabNotice,
} from "@/lib/browser/browser-tab-store"
import { displayHostPort } from "@/lib/browser/browser-url"
import type { BrowserErrorInfo, BrowserTabState } from "@/lib/browser/types"
import { browserTabBackendId } from "@/lib/file-tab-id"
import { openUrl } from "@/lib/platform"

/** Bars that sit OUTSIDE the native surface's rect (a native view paints over
 *  any DOM placed on top of it): blocked popups, remote-egress banner. */
export function BrowserNoticeBar({
  tab,
  state,
}: {
  tab: BrowserWorkspaceTab
  state: BrowserTabState | null
}) {
  const t = useTranslations("Browser.status")
  const notice = useBrowserTabNotice(tab.id)
  if (!notice && !state?.remoteHost) return null
  return (
    <div className="flex flex-col">
      {state?.remoteHost ? (
        <div className="flex h-7 items-center gap-2 border-b border-border/60 bg-muted/60 px-3 text-xs text-muted-foreground">
          {t("remoteBanner", { host: state.remoteHost })}
        </div>
      ) : null}
      {notice ? (
        <div className="flex h-8 items-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-3 text-xs text-foreground">
          <ShieldAlert className="h-3.5 w-3.5 shrink-0 text-amber-600" />
          <span className="min-w-0 flex-1 truncate">
            {t("popupDenied", {
              host: displayHostPort(notice.url) ?? notice.url,
            })}
            {notice.reason === "no-gesture"
              ? ` · ${t("popupDeniedNoGesture")}`
              : notice.reason === "blocked-scheme"
                ? ` · ${t("popupDeniedBlockedScheme")}`
                : ""}
          </span>
          <button
            type="button"
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded hover:bg-primary/8"
            title={t("dismiss")}
            aria-label={t("dismiss")}
            onClick={() => setBrowserTabNotice(tab.id, null)}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : null}
    </div>
  )
}

function errorLabel(
  t: ReturnType<typeof useTranslations<"Browser.status">>,
  error: BrowserErrorInfo
): string {
  switch (error.kind) {
    case "dns":
      return t("errorDns")
    case "tls":
      return t("errorTls")
    case "blocked":
      return t("errorBlocked")
    case "popup-denied":
      return t("errorPopupDenied")
    default:
      return t("errorFailed")
  }
}

/** DOM error page shown INSTEAD of the native surface (the host hides it). */
export function BrowserErrorPage({
  tab,
  error,
  url,
}: {
  tab: BrowserWorkspaceTab
  error: BrowserErrorInfo
  url: string
}) {
  const t = useTranslations("Browser.status")
  const backendId = browserTabBackendId(tab.id)
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
      <ShieldAlert className="h-8 w-8 text-muted-foreground/60" />
      <p className="text-sm font-medium text-foreground">
        {errorLabel(t, error)}
      </p>
      <p className="max-w-md break-all text-xs text-muted-foreground">
        {error.url ?? url}
      </p>
      {error.message ? (
        <p className="max-w-md text-xs text-muted-foreground/80">
          {error.message}
        </p>
      ) : null}
      <div className="mt-1 flex items-center gap-2">
        <button
          type="button"
          className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border px-2.5 text-xs hover:bg-primary/8"
          onClick={() => backendId && void browserReload(backendId)}
        >
          <RotateCw className="h-3.5 w-3.5" />
          {t("retry")}
        </button>
        <button
          type="button"
          className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border px-2.5 text-xs hover:bg-primary/8"
          onClick={() => void openUrl(error.url ?? url)}
        >
          <ExternalLink className="h-3.5 w-3.5" />
          {t("openInSystem")}
        </button>
      </div>
    </div>
  )
}

/** Placeholder shown in the pane when the page lives in an owned window
 *  (Linux, or the fallback surface): nothing is embedded here. */
export function BrowserOwnedWindowCard({
  url,
  onShow,
}: {
  url: string
  onShow: () => void
}) {
  const t = useTranslations("Browser.status")
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
      <p className="text-sm text-muted-foreground">{t("ownedWindow")}</p>
      <p className="max-w-md break-all text-xs text-muted-foreground/80">
        {url}
      </p>
      <button
        type="button"
        className="inline-flex h-7 items-center gap-1.5 rounded-md border border-border px-2.5 text-xs hover:bg-primary/8"
        onClick={onShow}
      >
        {t("ownedWindowShow")}
      </button>
    </div>
  )
}
