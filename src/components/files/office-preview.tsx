"use client"

import { useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { FileWarning, Loader2 } from "lucide-react"

import {
  startOfficeWatch,
  stopOfficeWatch,
  openSettingsWindow,
} from "@/lib/api"
import {
  isDesktop,
  isRemoteDesktopMode,
  getServerBaseUrl,
} from "@/lib/transport"
import { extractAppCommandError } from "@/lib/app-error"

// Machine code the backend stamps into `i18n_params.watchCode` for a missing
// officecli (mirrors `WatchError::NotInstalled.code()` in office_watch/mod.rs).
const NOT_INSTALLED = "NOT_INSTALLED"

// One-liner that installs OfficeCLI on the *server* host — shown to web/remote
// users, for whom an "open Settings" desktop link would point at the wrong
// machine. Mirrors the command the backend's own installer runs
// (`commands/office_tools.rs`).
const SERVER_INSTALL_CMD =
  "curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh | bash"

function watchCodeOf(err: unknown): string | null {
  return extractAppCommandError(err)?.i18n_params?.watchCode ?? null
}

/**
 * Preview a .docx/.xlsx/.pptx file via a long-lived `officecli watch` server.
 *
 * The backend spawns one `officecli watch <file> --port N` process per file
 * (shared across tabs by ref-count) and we point an iframe at its loopback HTTP
 * server. officecli drives live refresh over its own SSE channel, so — unlike
 * the old one-shot `view html` render that re-read the whole file on every
 * change — the preview and the agent's officecli edits no longer contend for
 * the file on disk (the Windows file-lock bug this change fixes).
 *
 * ## Where the iframe points (and why the sandbox differs)
 *
 * - **Local desktop** (`isDesktop() && !isRemoteDesktopMode()`): the Tauri
 *   webview reaches `http://127.0.0.1:{port}` directly. That iframe gets its
 *   real loopback origin (≠ the app's `tauri://localhost`), so it keeps
 *   `allow-same-origin` — it needs same-origin to talk to its own SSE channel,
 *   and it still can't read the app's storage (different origin).
 * - **Web / remote-desktop**: the browser can't reach the server's loopback, so
 *   the iframe loads `{server}/api/office-watch-proxy/{port}/?cap=…`. Here we
 *   **drop `allow-same-origin`** → the iframe runs in an opaque origin and
 *   physically cannot read `localStorage` (the master token never leaks to a
 *   hypothetical malicious office file). The proxy injects a shim that routes
 *   officecli's root-absolute requests back through itself, and answers the
 *   resulting cross-origin CORS. Auth is the per-watch `cap`, not the token.
 */
export function OfficePreview({
  rootPath,
  relPath,
}: {
  // Backend watch target as a (directory, relative file) pair — the file
  // tab's absolute path split by the panel. No workspace folder needed.
  rootPath: string | null
  relPath: string | null
}) {
  const t = useTranslations("Folder.fileWorkspacePanel")
  const [port, setPort] = useState<number | null>(null)
  const [cap, setCap] = useState<string>("")
  const [errorCode, setErrorCode] = useState<string | null>(null)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [retryKey, setRetryKey] = useState(0)

  const path = relPath ?? ""

  // True only for a local desktop window: it loads the watch's loopback URL
  // directly. Web windows go through the proxy.
  const loopbackDirect = isDesktop() && !isRemoteDesktopMode()
  // A Tauri window bound to a remote server can't load the preview at all: its
  // webview is a secure context that mixed-content-blocks the remote `http://`
  // proxy URL, and (unlike JSON calls) a raw iframe can't be tunnelled through
  // Rust. Rather than spawn a remote watch nobody can see, we show a hint to
  // open the preview in the server's web UI instead.
  const remoteDesktop = isDesktop() && isRemoteDesktopMode()

  // Start the watch server on mount (and on retry); stop it on unmount. The
  // component is keyed by tab id upstream, so a different file remounts fresh.
  // State is only set inside the async callbacks, never synchronously in the
  // effect body (the repo lints against that).
  useEffect(() => {
    let cancelled = false
    // Whether *our* start committed a ref-count. Drives exactly-one release so
    // an unmount-before-start race neither leaks a ref (we release once the
    // start resolves) nor over-releases a watch another tab still shares (we
    // never release a ref we didn't acquire).
    let acquired = false
    const root = rootPath ?? ""
    // Don't spawn a watch we can't display (remote-desktop, see above).
    if (!root || !path || remoteDesktop) return
    startOfficeWatch(root, path)
      .then((res) => {
        acquired = true
        if (cancelled) {
          // Unmounted before start resolved — release the ref we just took.
          void stopOfficeWatch(root, path).catch(() => {})
          return
        }
        setPort(res.port)
        setCap(res.cap)
        setErrorCode(null)
        setErrorMessage(null)
      })
      .catch((err) => {
        if (cancelled) return
        setErrorCode(watchCodeOf(err) ?? "START_FAILED")
        setErrorMessage(extractAppCommandError(err)?.message ?? String(err))
      })
    return () => {
      cancelled = true
      if (acquired) {
        void stopOfficeWatch(root, path).catch(() => {})
      }
    }
  }, [path, rootPath, retryKey, remoteDesktop])

  const watchUrl = useMemo(() => {
    if (port == null) return null
    if (loopbackDirect) {
      // The Tauri webview loads loopback directly (no mixed-content: the host
      // page is tauri://localhost, also loopback).
      return `http://127.0.0.1:${port}/`
    }
    // Web: route through the reverse proxy on the server that backs this
    // window, carrying the per-watch capability the proxy validates.
    const base = getServerBaseUrl()
    return `${base}/api/office-watch-proxy/${port}/?cap=${encodeURIComponent(cap)}`
  }, [port, cap, loopbackDirect])

  const retry = () => {
    setErrorCode(null)
    setErrorMessage(null)
    setPort(null)
    setCap("")
    setRetryKey((k) => k + 1)
  }

  if (remoteDesktop) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
        <FileWarning className="h-8 w-8 text-muted-foreground" />
        <div className="text-sm font-medium text-foreground">
          {t("officePreviewTitle")}
        </div>
        <div className="max-w-sm text-xs text-muted-foreground">
          {t("officeRemoteDesktopUnsupported")}
        </div>
      </div>
    )
  }

  if (errorCode === NOT_INSTALLED) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
        <FileWarning className="h-8 w-8 text-muted-foreground" />
        <div className="text-sm font-medium text-foreground">
          {t("officeNotInstalled")}
        </div>
        {loopbackDirect ? (
          <>
            <div className="max-w-sm text-xs text-muted-foreground">
              {t("officeNotInstalledHint")}
            </div>
            <button
              type="button"
              onClick={() => {
                openSettingsWindow().catch(() => {})
              }}
              className="mt-1 rounded-md border border-border bg-card px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-primary/8"
            >
              {t("officeOpenSettings")}
            </button>
          </>
        ) : (
          <>
            {/* Web/remote: officecli must be installed on the server host, not
                the user's machine — show the exact command to run there. */}
            <div className="max-w-sm text-xs text-muted-foreground">
              {t("officeServerInstallHint")}
            </div>
            <code className="block max-w-sm select-all rounded-md bg-muted px-3 py-2 text-left text-2xs text-foreground">
              {SERVER_INSTALL_CMD}
            </code>
            <button
              type="button"
              onClick={retry}
              className="mt-1 rounded-md border border-border bg-card px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-primary/8"
            >
              {t("officeWatchRetry")}
            </button>
          </>
        )}
      </div>
    )
  }

  if (errorCode) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
        <FileWarning className="h-8 w-8 text-muted-foreground" />
        <div className="text-sm font-medium text-foreground">
          {t("officeWatchFailed")}
        </div>
        {errorMessage && (
          <div className="max-w-sm break-words text-xs text-muted-foreground">
            {errorMessage}
          </div>
        )}
        <button
          type="button"
          onClick={retry}
          className="mt-1 rounded-md border border-border bg-card px-3 py-1.5 text-xs font-medium text-foreground transition-colors hover:bg-primary/8"
        >
          {t("officeWatchRetry")}
        </button>
      </div>
    )
  }

  // No filename header here — the workspace tab already shows the file name.
  return (
    <div className="relative h-full min-h-0">
      {watchUrl == null ? (
        <div className="flex h-full items-center justify-center gap-2 text-xs text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("loading")}
        </div>
      ) : (
        <iframe
          title={t("officePreviewTitle")}
          src={watchUrl}
          // Desktop loopback keeps its real (loopback) origin so officecli's
          // own same-origin SSE works. Web/proxy mode runs opaque-origin (no
          // allow-same-origin) so the page can't read the app's storage; its
          // sub-requests are rewritten + CORS-allowed by the proxy.
          sandbox={
            loopbackDirect
              ? "allow-scripts allow-same-origin allow-popups allow-forms"
              : "allow-scripts allow-popups allow-forms"
          }
          // The proxy URL carries the watch capability; never let it ride a
          // Referer to any external resource officecli's page might load. The
          // injected shim routes officecli's own requests by absolute path, so
          // it doesn't depend on Referer.
          referrerPolicy="no-referrer"
          className="absolute inset-0 h-full w-full border-0 bg-white"
        />
      )}
    </div>
  )
}
