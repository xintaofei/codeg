"use client"

import { useCallback } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { useSessionViewerHost } from "@/components/message/session-viewer-host-context"
import { useOptionalWorkbenchRoute } from "@/contexts/workbench-route-context"
import { useOptionalWorkspaceActions } from "@/contexts/workspace-context"
import { browserCapabilitiesSnapshot } from "@/lib/browser/browser-api"
import {
  getBrowserPrefs,
  markBrowserFirstOpenSeen,
  setAllDefaultLinkTargets,
  type LinkSource,
  type LinkTarget,
} from "@/lib/browser/browser-prefs"
import { displayHostPort } from "@/lib/browser/browser-url"
import { openInSystemBrowser, openWithOsHandler } from "@/lib/link-open"
import {
  resolveLinkAction,
  type LinkAction,
  type LinkSurface,
} from "@/lib/resolve-link-action"
import { isRemoteDesktopMode } from "@/lib/transport"

export interface OpenUrlOptions {
  source: LinkSource
  /** Primary modifier (⌘ on macOS, Ctrl elsewhere) held during the gesture. */
  modifier?: boolean
  /** Explicit target chosen from a menu; ignores the modifier and preference. */
  forceTarget?: LinkTarget
}

/** ⌘ on macOS, Ctrl elsewhere — the "open the other way" modifier. */
export function isPrimaryModifier(event: {
  metaKey?: boolean
  ctrlKey?: boolean
}): boolean {
  const mac =
    typeof navigator !== "undefined" &&
    /Mac|iPhone|iPad/.test(navigator.platform)
  return mac ? Boolean(event.metaKey) : Boolean(event.ctrlKey)
}

/**
 * Decide and execute where an http(s) (or mailto/tel) address goes, INSIDE the
 * caller's call stack. The decision itself (`resolveLinkAction`) is a pure
 * function of a preferences snapshot and this surface; only the execution
 * touches the app: the built-in browser tab (or the transcript's side panel
 * under a full-page route), the system browser, or the OS handler.
 *
 * Returns the action taken so callers can react. Of the rejections only the
 * site-rule block is reported here (its wording is the browser's); the caller
 * owns the toast for an unsupported scheme. Local file paths are not handled:
 * they are `useOpenFileTarget`'s job and never reach this hook.
 */
export function useOpenUrlTarget() {
  const t = useTranslations("Browser.toast")
  // Null outside the workspace (no tab strip to open into): the built-in
  // target is then simply unavailable and links go to the system browser.
  const openBrowserTab = useOptionalWorkspaceActions()?.openBrowserTab ?? null
  const route = useOptionalWorkbenchRoute()
  const viewerHost = useSessionViewerHost()
  const fileColumnVisible = route ? route.isConversations : true

  return useCallback(
    (url: string, options: OpenUrlOptions): LinkAction => {
      const capabilities = browserCapabilitiesSnapshot()
      const surface: LinkSurface = {
        builtinAvailable:
          (capabilities?.available ?? false) &&
          (openBrowserTab !== null || viewerHost !== null),
        fileColumnVisible,
        viewerHostAvailable: viewerHost !== null,
        remoteDesktop: isRemoteDesktopMode(),
      }
      const prefs = getBrowserPrefs()
      const action = resolveLinkAction(url, {
        source: options.source,
        modifier: options.modifier ?? false,
        forceTarget: options.forceTarget,
        surface,
        prefs,
        hostRules: prefs.hostRules,
        managedHostRules: capabilities?.policy.managedRules,
      })
      switch (action.kind) {
        case "system":
          void openInSystemBrowser(action.url)
          break
        case "os-handler":
          void openWithOsHandler(action.url)
          break
        case "builtin": {
          if (action.placement === "drawer" && viewerHost) {
            viewerHost.open({ kind: "browser", url: action.url })
          } else if (openBrowserTab) {
            openBrowserTab(action.url)
          } else if (viewerHost) {
            viewerHost.open({ kind: "browser", url: action.url })
          }
          if (!prefs.firstOpenSeen) {
            markBrowserFirstOpenSeen()
            toast(t("firstOpen"), {
              description: t("firstOpenHint"),
              action: {
                label: t("useSystemAlways"),
                onClick: () => setAllDefaultLinkTargets("system"),
              },
            })
          }
          break
        }
        case "reject":
          // The one rejection this hook owns the wording of: a site rule
          // decided, and the user should learn which host it was.
          if (action.reason === "blocked-host") {
            toast.error(
              t("blockedHost", {
                host: displayHostPort(action.url) ?? action.url,
              })
            )
          }
          break
        case "file":
          break
      }
      return action
    },
    [fileColumnVisible, openBrowserTab, t, viewerHost]
  )
}
