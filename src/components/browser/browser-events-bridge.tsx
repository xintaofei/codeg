"use client"

import { useEffect } from "react"

import { useWorkspaceActions } from "@/contexts/workspace-context"
import {
  browserCapabilities,
  browserClose,
  browserListTabs,
} from "@/lib/browser/browser-api"
import {
  browserWorkspaceTabId,
  removeBrowserTabState,
  setBrowserTabNotice,
  setBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import {
  BROWSER_CLOSED_EVENT,
  BROWSER_OPEN_REQUEST_EVENT,
  BROWSER_POPUP_EVENT,
  BROWSER_STATE_EVENT,
  type BrowserClosedPayload,
  type BrowserOpenRequestPayload,
  type BrowserPopupPayload,
  type BrowserTabState,
} from "@/lib/browser/types"
import { getTransport } from "@/lib/transport"
import { getCurrentWindowLabel } from "@/lib/browser/window-label"

/**
 * The one subscriber to the backend's `browser://*` streams. Mounted once
 * inside the workspace providers (it needs the workspace actions to add and
 * remove tab records); renders nothing.
 *
 * - `browser://state`  → the tab store (toolbar, status layer, tab title)
 * - `browser://popup`  → an adopted popup becomes a tab next to its opener
 * - `browser://closed` → a surface the backend tore down (owned window closed
 *   by the user, owner window gone) drops its tab record
 * - `browser://open-request` → the backend (an agent tool, a deep link, the
 *   dev puppet) asks this window's workspace to open a URL
 *
 * Only subscribes where a built-in browser exists; in web mode there is
 * nothing to hear.
 */
export function BrowserEventsBridge() {
  const { adoptBrowserTab, closeFileTab, openBrowserTab } =
    useWorkspaceActions()

  useEffect(() => {
    let cancelled = false
    const unsubscribers: Array<() => void> = []

    void (async () => {
      const capabilities = await browserCapabilities()
      if (cancelled || !capabilities.available) return
      // Tab records are session-only, so any surface the backend still holds
      // for this window when the bridge first mounts is an orphan of a
      // previous document (a dev reload, a crashed frontend). Close them,
      // or they would stay painted over the new UI with nothing to hide them.
      try {
        const orphans = await browserListTabs()
        await Promise.all(orphans.map((tab) => browserClose(tab.tabId)))
      } catch {
        /* nothing to sweep */
      }
      if (cancelled) return
      const transport = getTransport()
      const subs = await Promise.all([
        transport.subscribe<BrowserTabState>(BROWSER_STATE_EVENT, (state) => {
          setBrowserTabState(state)
        }),
        transport.subscribe<BrowserPopupPayload>(
          BROWSER_POPUP_EVENT,
          (popup) => {
            if (popup.presentation === "denied") {
              setBrowserTabNotice(browserWorkspaceTabId(popup.openerTabId), {
                kind: "popup-denied",
                url: popup.url,
                reason: popup.reason,
              })
              return
            }
            if (!popup.tabId) return
            adoptBrowserTab({
              backendTabId: popup.tabId,
              url: popup.url,
              openerBackendTabId: popup.openerTabId,
            })
          }
        ),
        transport.subscribe<BrowserClosedPayload>(
          BROWSER_CLOSED_EVENT,
          (closed) => {
            const tabId = browserWorkspaceTabId(closed.tabId)
            removeBrowserTabState(tabId)
            closeFileTab(tabId)
          }
        ),
        transport.subscribe<BrowserOpenRequestPayload>(
          BROWSER_OPEN_REQUEST_EVENT,
          (request) => {
            // Every window hears every event; only the addressed one acts.
            const target = request.ownerWindow ?? "main"
            if (target !== getCurrentWindowLabel()) return
            const openerTabId = request.openerTabId
              ? browserWorkspaceTabId(request.openerTabId)
              : undefined
            openBrowserTab(request.url, {
              activate: request.activate,
              openerTabId,
            })
          }
        ),
      ])
      if (cancelled) {
        for (const unsubscribe of subs) unsubscribe()
        return
      }
      unsubscribers.push(...subs)
    })()

    return () => {
      cancelled = true
      for (const unsubscribe of unsubscribers) unsubscribe()
    }
  }, [adoptBrowserTab, closeFileTab, openBrowserTab])

  return null
}
