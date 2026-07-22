"use client"

import type { ReactNode } from "react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { openUrl } from "@/lib/platform"
import { getActiveRemoteConnectionId, isDesktop } from "@/lib/transport"
import { toErrorMessage } from "@/lib/app-error"
import type { LinkSafetyConfig, LinkSafetyModalProps } from "streamdown"
import { toast } from "sonner"
import { FilePathContextMenu } from "@/components/shared/file-path-context-menu"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useWorkspaceActions } from "@/contexts/workspace-context"
import { isHomeRelativePath } from "@/lib/file-open-target"
import {
  isAbsoluteFilePath,
  toNativeAbsoluteFilePath,
} from "@/lib/file-path-display"
import { cn } from "@/lib/utils"

interface LocalFileTarget {
  path: string
  line: number | null
}

const WINDOWS_ABSOLUTE_PATH = /^[a-zA-Z]:[\\/]/
const URL_SCHEME = /^[a-zA-Z][a-zA-Z\d+\-.]*:/
const ALLOWED_EXTERNAL_PROTOCOLS = new Set([
  "http:",
  "https:",
  "mailto:",
  "tel:",
])
// Protocols handled by the OS (mail client, dialer) rather than a browser
// page load. They must NOT be opened via `window.open(_, "_blank")` — most
// browsers leave behind an empty `about:blank` tab once the OS handler fires.
const OS_HANDLER_PROTOCOLS = new Set(["mailto:", "tel:"])

function normalizeSlashPath(path: string): string {
  return path.replace(/\\/g, "/")
}

/** Strip leading slash before Windows drive letter: /C:/foo → C:/foo */
function stripLeadingSlashOnWindows(p: string): string {
  if (p.startsWith("/") && WINDOWS_ABSOLUTE_PATH.test(p.slice(1))) {
    return p.slice(1)
  }
  return p
}

function decodeUriSafely(value: string): string {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function parseLineValue(raw: string | undefined): number | null {
  if (!raw) return null
  const line = Number.parseInt(raw, 10)
  if (!Number.isFinite(line) || line <= 0) return null
  return line
}

function parseHashLine(hash: string): number | null {
  const normalized = hash.startsWith("#") ? hash.slice(1) : hash
  if (!normalized) return null
  // `L<start>` / `L<start>-<end>` / `L<start>-L<end>` (GitHub-style) — a range
  // (e.g. the editor's "add selection" badge `#L10-25`) jumps to its start line.
  return (
    parseLineValue(normalized.match(/^L(\d+)(?:-L?\d+)?$/i)?.[1]) ??
    parseLineValue(normalized.match(/^line=(\d+)$/i)?.[1]) ??
    parseLineValue(normalized.match(/^(\d+)$/)?.[1])
  )
}

function splitPathAndLine(rawPath: string): LocalFileTarget {
  const trimmed = rawPath.trim()
  const match = trimmed.match(/^(.*):(\d+)(?::\d+)?$/)
  if (!match) {
    return { path: trimmed, line: null }
  }

  const maybePath = match[1]
  if (!maybePath || maybePath.endsWith("://")) {
    return { path: trimmed, line: null }
  }

  const line = parseLineValue(match[2])
  if (!line) {
    return { path: trimmed, line: null }
  }

  return { path: maybePath, line }
}

function isLocalPathLike(path: string): boolean {
  // "//host/…" (forward slashes) is protocol-relative — a WEB url, not a
  // local path. It must fall through to the external-URL route, never into
  // local file IO. A "\\server\share" (backslashes) IS a local UNC path
  // (a web url never uses backslashes) — the form remark-file-uri-links
  // emits for file://server/share URIs.
  return (
    (path.startsWith("/") && !path.startsWith("//")) ||
    path.startsWith("\\\\") ||
    path.startsWith("./") ||
    path.startsWith("../") ||
    path.startsWith("~/") ||
    WINDOWS_ABSOLUTE_PATH.test(path)
  )
}

function parseLocalFileTarget(rawUrl: string): LocalFileTarget | null {
  const trimmed = rawUrl.trim()
  if (!trimmed) return null

  if (trimmed.toLowerCase().startsWith("file://")) {
    try {
      const parsed = new URL(trimmed)
      const rawPathname = decodeUriSafely(parsed.pathname)
      // A non-empty host is a UNC authority (file://server/share/x) —
      // preserve it as //server/share/x rather than dropping to /share/x.
      const normalizedPathname = parsed.host
        ? `//${parsed.host}${rawPathname}`
        : stripLeadingSlashOnWindows(rawPathname)
      const pathAndLine = splitPathAndLine(normalizedPathname)
      if (!pathAndLine.path) return null
      return {
        path: normalizeSlashPath(pathAndLine.path),
        line: parseHashLine(parsed.hash) ?? pathAndLine.line,
      }
    } catch {
      return null
    }
  }

  if (URL_SCHEME.test(trimmed) && !WINDOWS_ABSOLUTE_PATH.test(trimmed)) {
    return null
  }

  // Split on raw # / ? before decoding so encoded `%23` / `%3F` inside the
  // path don't get promoted to fragment/query separators (which would point
  // the file opener at the wrong file).
  const hashIndex = trimmed.indexOf("#")
  const rawHash = hashIndex >= 0 ? trimmed.slice(hashIndex) : ""
  const beforeHash = hashIndex >= 0 ? trimmed.slice(0, hashIndex) : trimmed
  const queryIndex = beforeHash.indexOf("?")
  const rawPathPart =
    queryIndex >= 0 ? beforeHash.slice(0, queryIndex) : beforeHash
  const decodedPath = decodeUriSafely(rawPathPart)
  const pathAndLine = splitPathAndLine(decodedPath)
  const normalizedPath = stripLeadingSlashOnWindows(pathAndLine.path)
  if (!isLocalPathLike(normalizedPath)) return null

  return {
    path: normalizeSlashPath(normalizedPath),
    line: parseHashLine(rawHash) ?? pathAndLine.line,
  }
}

function parseExternalUrl(rawUrl: string): URL | null {
  const trimmed = rawUrl.trim()
  if (!trimmed) return null

  if (trimmed.startsWith("//")) {
    // Protocol-relative: pin to https rather than the page protocol — a
    // Tauri webview's own scheme (tauri://localhost) would otherwise
    // classify these as an unsupported protocol, and the desktop opener
    // capability only allows concrete http(s) URLs.
    try {
      return new URL(`https:${trimmed}`)
    } catch {
      return null
    }
  }

  if (!URL_SCHEME.test(trimmed) || WINDOWS_ABSOLUTE_PATH.test(trimmed)) {
    return null
  }

  try {
    return new URL(trimmed)
  } catch {
    return null
  }
}

function getAllowedExternalProtocol(rawUrl: string): string | null {
  const parsed = parseExternalUrl(rawUrl)
  if (!parsed) return null
  const protocol = parsed.protocol.toLowerCase()
  return ALLOWED_EXTERNAL_PROTOCOLS.has(protocol) ? protocol : null
}

/**
 * True when the current window has no access to the Tauri opener plugin
 * (pure web, or a Tauri window bound to a remote codeg-server).
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
  return isWebOpenerEnvironment()
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
 * bare local paths) open in the workspace file panel; http(s)/mailto/tel route
 * to the browser / OS handler. Used by the Streamdown link-safety modal and by
 * standalone clickable file affordances (e.g. user-message resource badges).
 */
export function useOpenLinkOrFile() {
  const t = useTranslations("Folder.chat.linkSafety")
  const { activeFolder: folder } = useActiveFolder()
  const folderPath = folder?.path
  const { openFilePreview } = useWorkspaceActions()

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
          await openFilePreview(localTarget.path.replace(/^\.\/+/, ""), {
            line: localTarget.line ?? undefined,
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
    [folderPath, openFilePreview, t]
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
 * Clickable file-path label that routes the file into the workspace file panel.
 * Right-click opens the shared VS Code-style path menu (copy / reveal / open
 * with / add-to-chat) — same affordance as the message-nav and reply-artifact
 * changed-file rows, so tool reads/writes and diff headers stay consistent.
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
  const { openFilePreview } = useWorkspaceActions()
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
    void openFilePreview(target, {
      line: line ?? undefined,
    })
      .catch((error) => {
        toast.error(t("errorFailedOpen"), {
          description: toErrorMessage(error),
        })
      })
      .finally(() => {
        openingRef.current = false
        setOpening(false)
      })
  }, [filePath, folderPath, line, openFilePreview, t])

  // Always prefer a native absolute path for hover tooltips (matches Edit headers).
  const absoluteTitle =
    title ??
    toNativeAbsoluteFilePath(filePath, folderPath ?? undefined) ??
    filePath

  return (
    <FilePathContextMenu
      filePath={filePath}
      folderPath={folderPath ?? undefined}
      onOpenInCodeg={handleOpen}
      title={absoluteTitle}
      className={cn("min-w-0", className)}
    >
      <button
        type="button"
        title={absoluteTitle}
        aria-busy={opening}
        disabled={opening}
        className="block max-w-full min-w-0 cursor-pointer truncate text-left align-bottom hover:underline focus-visible:underline focus-visible:outline-none disabled:cursor-wait disabled:opacity-70 disabled:hover:no-underline"
        onClick={(e) => {
          e.stopPropagation()
          handleOpen()
        }}
      >
        {children}
      </button>
    </FilePathContextMenu>
  )
}
