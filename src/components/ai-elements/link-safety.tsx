"use client"

import type { ReactNode } from "react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { openUrl } from "@/lib/platform"
import { getActiveRemoteConnectionId, isDesktop } from "@/lib/transport"
import { toErrorMessage } from "@/lib/app-error"
import type { LinkSafetyConfig, LinkSafetyModalProps } from "streamdown"
import { toast } from "sonner"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useOpenFileTarget } from "@/hooks/use-open-file-target"
import { isHomeRelativePath } from "@/lib/file-open-target"
import { isAbsoluteFilePath } from "@/lib/file-path-display"
import { cn } from "@/lib/utils"

import {
  OS_HANDLER_PROTOCOLS,
  getAllowedExternalProtocol,
  normalizeSlashPath,
  parseLocalFileTarget,
  type LocalFileTarget,
} from "@/lib/link-classify"

// The parsing helpers live in `@/lib/link-classify` now (shared with the
// built-in browser's link decision and the terminal); re-exported here so the
// transcript-side importers keep their historical entry point.
export { parseLocalFileTarget }
export type { LocalFileTarget }

/**
 * True when `window.open` actually opens something — i.e. a real browser.
 *
 * NOT the same question as `isWebOpenerEnvironment` below. A Tauri window bound
 * to a remote codeg-server is still a TAURI WEBVIEW, and a webview that
 * registers no new-window handler opens nothing at all for `window.open` (wry
 * answers with nil on macOS, `SetHandled(true)` on Windows). Lumping remote
 * windows in with web mode here left every http(s) link in a remote workspace
 * silently dead; they must take the opener-plugin path instead, which
 * `capabilities/default.json` grants to the `remote-*` windows.
 */
function windowOpenReachesABrowser(): boolean {
  return !isDesktop()
}

/**
 * True when a `mailto:`/`tel:` URL should be handed to the OS through a
 * synthetic anchor rather than the Tauri opener plugin — pure web, or a Tauri
 * window bound to a remote codeg-server.
 *
 * The remote arm stays deliberately: unlike `window.open`, a synthetic anchor
 * DOES reach the OS handler from inside a webview, and it sidesteps the
 * question of whether the opener capability covers non-http(s) schemes.
 */
function isWebOpenerEnvironment(): boolean {
  return !isDesktop() || getActiveRemoteConnectionId() !== null
}

function shouldLetStreamdownOpenExternalUrl(rawUrl: string): boolean {
  if (parseLocalFileTarget(rawUrl)) return false
  const protocol = getAllowedExternalProtocol(rawUrl)
  if (!protocol) return false
  // OS-handler protocols always go through our own path so we can dispatch
  // them via a synthetic anchor click — streamdown's `window.open(_, "_blank")`
  // would otherwise leave a blank tab behind.
  if (OS_HANDLER_PROTOCOLS.has(protocol)) return false
  // Streamdown opens what it is allowed to open with `window.open`, so hand it
  // a link only where that reaches a browser.
  return windowOpenReachesABrowser()
}

/**
 * Trigger an OS-registered protocol handler (mail client, dialer) from a
 * browser without leaving an empty tab. The synthetic anchor has no
 * `target`, so the browser hands the URL to the OS handler and stays on
 * the current page.
 */
function dispatchOsHandlerUrl(url: string): void {
  const anchor = document.createElement("a")
  anchor.href = url
  anchor.rel = "noreferrer noopener"
  document.body.appendChild(anchor)
  try {
    anchor.click()
  } finally {
    anchor.remove()
  }
}

/**
 * Open an external URL in a new tab. Callers MUST invoke this inside the
 * click's own call stack — see `openLinkWithSafety`.
 */
export function openExternalTab(url: string): void {
  // `noreferrer` (which implies `noopener`) matters for AI-authored links: the
  // opened page gets no `window.opener` handle back into the app and no
  // Referer. It also makes `window.open` return null even on success (HTML
  // window open steps 12 and 17), so the return value carries no signal —
  // don't test it for a "popup blocked" check, it would fire on every success.
  window.open(url, "_blank", "noreferrer")
}

/**
 * Route a link click through the link-safety config. `decline` runs whenever
 * the config wants its modal hook instead (local files, mailto/tel, and every
 * link on local desktop, which routes to the Tauri opener).
 *
 * A synchronous verdict — the only kind `useStreamdownLinkSafety` returns —
 * opens the tab inside the CALLER'S OWN CALL STACK. That is the whole point of
 * this helper: WebKit's popup blocker keys off the user-gesture stack rather
 * than the spec's transient activation, so a single microtask hop (an `await`
 * on a plain boolean) is enough for Safari and WKWebView-backed webviews to
 * swallow the popup with no error at all. See issue #410.
 */
export function openLinkWithSafety(
  url: string,
  linkSafety: LinkSafetyConfig,
  decline: () => void
): void {
  const verdict = linkSafety.onLinkCheck?.(url)
  if (verdict === true) {
    openExternalTab(url)
    return
  }
  if (verdict === false || verdict === undefined) {
    decline()
    return
  }
  // Streamdown's contract also allows a promise. codeg's own config never
  // returns one; if that ever changes, this branch must synchronously reserve
  // a tab BEFORE awaiting and navigate it afterwards — by the time the promise
  // settles the gesture is gone and this open will be blocked. A rejected check
  // declines (second `then` argument, so it can't swallow errors thrown by the
  // fulfilment path) rather than leaving the click with no outcome at all.
  void verdict.then(
    (allowed) => (allowed ? openExternalTab(url) : decline()),
    decline
  )
}

// True when the opener needs no folder context at all: absolute paths and
// `~/` paths are self-locating (openFilePreview expands the home dir and
// routes them by absolute path — inside a registered folder or not).
function isSelfLocatingPath(path: string): boolean {
  return isAbsoluteFilePath(path) || isHomeRelativePath(path)
}

/**
 * Streamdown's link-safety contract renders this component whenever
 * `onLinkCheck` declines a click. We render nothing — instead we hijack
 * the `isOpen` transition to run our open-target action immediately, then
 * call `onClose()` so streamdown's internal `isOpen` flag flips back to
 * `false` and the next click on the same link is accepted.
 *
 * The handler identities are pinned through refs so a parent re-render
 * mid-flight (translator function, workspace context, etc.) cannot tear
 * down the effect and leave streamdown stuck with `isOpen === true`.
 */
function DirectLinkOpen({
  url,
  isOpen,
  onClose,
  onAction,
}: LinkSafetyModalProps & {
  onAction: (url: string) => Promise<void>
}) {
  const lastOpenedUrlRef = useRef<string | null>(null)
  const onActionRef = useRef(onAction)
  const onCloseRef = useRef(onClose)

  // Sync the latest handler identities into refs after each render so the
  // trigger effect below can stay scoped to `[isOpen, url]` and survive
  // mid-flight parent re-renders.
  useEffect(() => {
    onActionRef.current = onAction
    onCloseRef.current = onClose
  })

  useEffect(() => {
    if (!isOpen) {
      lastOpenedUrlRef.current = null
      return
    }
    if (lastOpenedUrlRef.current === url) return
    lastOpenedUrlRef.current = url
    void onActionRef.current(url).finally(() => {
      onCloseRef.current()
    })
  }, [isOpen, url])

  return null
}

/**
 * Hook returning an async opener for a link or local-file uri: `file://` (and
 * bare local paths) open in the workspace file panel — or, where that panel is
 * covered by a full-page route, in the transcript's own file viewer (see
 * `useOpenFileTarget`); http(s)/mailto/tel route to the browser / OS handler.
 * Used by the Streamdown link-safety modal and by standalone clickable file
 * affordances (e.g. user-message resource badges).
 */
export function useOpenLinkOrFile() {
  const t = useTranslations("Folder.chat.linkSafety")
  const { activeFolder: folder } = useActiveFolder()
  const folderPath = folder?.path
  const openFileTarget = useOpenFileTarget()

  return useCallback(
    async (url: string) => {
      const localTarget = parseLocalFileTarget(url)
      if (localTarget) {
        // Absolute and ~ paths open with no folder context (works in chat
        // mode too); only folder-relative paths still need an active
        // folder to resolve against.
        if (!isSelfLocatingPath(localTarget.path) && !folderPath) {
          toast.error(t("errorCannotOpen"), {
            description: t("errorNoWorkspace"),
          })
          return
        }

        try {
          await openFileTarget(localTarget.path.replace(/^\.\/+/, ""), {
            line: localTarget.line,
          })
        } catch (error) {
          toast.error(t("errorFailedOpen"), {
            description: toErrorMessage(error),
          })
        }
        return
      }

      const protocol = getAllowedExternalProtocol(url)
      if (!protocol) {
        toast.error(t("errorFailedLink"), {
          description: t("errorUnsupportedLinkProtocol"),
        })
        return
      }

      // Dispatch the CANONICAL form: a protocol-relative "//host/…" must
      // reach the desktop opener as a concrete https URL — the opener
      // capability only allows http(s), and raw "//…" would resolve
      // against the webview's own scheme.
      const openTarget = url.trim().startsWith("//")
        ? `https:${url.trim()}`
        : url

      try {
        if (OS_HANDLER_PROTOCOLS.has(protocol) && isWebOpenerEnvironment()) {
          dispatchOsHandlerUrl(openTarget)
        } else {
          await openUrl(openTarget)
        }
      } catch (error) {
        toast.error(t("errorFailedLink"), {
          description: toErrorMessage(error),
        })
      }
    },
    [folderPath, openFileTarget, t]
  )
}

export function useStreamdownLinkSafety(): LinkSafetyConfig {
  const handleOpenTarget = useOpenLinkOrFile()

  const handleLinkCheck = useCallback(
    (url: string) => shouldLetStreamdownOpenExternalUrl(url),
    []
  )

  const renderModal = useCallback(
    (props: LinkSafetyModalProps) => (
      <DirectLinkOpen {...props} onAction={handleOpenTarget} />
    ),
    [handleOpenTarget]
  )

  return useMemo(
    () => ({
      enabled: true,
      onLinkCheck: handleLinkCheck,
      renderModal,
    }),
    [handleLinkCheck, renderModal]
  )
}

/**
 * Normalize a tool-call file path (absolute, `~/`, workspace-relative, or a
 * bare relative path) into something `openFilePreview` can consume. Only a
 * relative path still depends on the active folder — the caller checks that.
 */
function resolveToolFilePath(rawPath: string): string | null {
  const normalized = normalizeSlashPath(rawPath.trim())
  if (!normalized) return null
  if (isSelfLocatingPath(normalized)) return normalized
  return normalized.replace(/^\.\/+/, "")
}

/**
 * Clickable file-path label that routes the file into the workspace file panel
 * — or the transcript's own file viewer when that panel is covered by a
 * full-page route (see `useOpenFileTarget`).
 */
export function FilePathLink({
  filePath,
  line,
  className,
  title,
  children,
}: {
  filePath: string
  line?: number | null
  className?: string
  title?: string
  children: ReactNode
}) {
  const t = useTranslations("Folder.chat.linkSafety")
  const { activeFolder: folder } = useActiveFolder()
  const folderPath = folder?.path ?? null
  const openFileTarget = useOpenFileTarget()
  // `opening` drives the visual busy state. `openingRef` is the synchronous
  // gate that survives rapid double-fires within a single event tick —
  // React batches the `setOpening(true)` commit, so relying purely on the
  // `disabled` attribute would leave a window where two clicks dispatched
  // before commit could both pass the early-return check.
  const [opening, setOpening] = useState(false)
  const openingRef = useRef(false)

  const handleOpen = useCallback(() => {
    if (openingRef.current) return
    const target = resolveToolFilePath(filePath)
    if (!target) return
    // Only folder-relative paths need an active folder; absolute and ~
    // paths are self-locating.
    if (!isSelfLocatingPath(target) && !folderPath) {
      toast.error(t("errorCannotOpen"), {
        description: t("errorNoWorkspace"),
      })
      return
    }

    openingRef.current = true
    setOpening(true)
    void openFileTarget(target, { line })
      .catch((error) => {
        toast.error(t("errorFailedOpen"), {
          description: toErrorMessage(error),
        })
      })
      .finally(() => {
        openingRef.current = false
        setOpening(false)
      })
  }, [filePath, folderPath, line, openFileTarget, t])

  return (
    <span className={cn("block min-w-0", className)}>
      <button
        type="button"
        title={title ?? filePath}
        aria-busy={opening}
        disabled={opening}
        className="max-w-full cursor-pointer truncate text-left align-bottom hover:underline focus-visible:underline focus-visible:outline-none disabled:cursor-wait disabled:opacity-70 disabled:hover:no-underline"
        onClick={(e) => {
          e.stopPropagation()
          handleOpen()
        }}
      >
        {children}
      </button>
    </span>
  )
}
