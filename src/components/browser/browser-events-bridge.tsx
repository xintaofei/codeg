"use client"

import { useEffect } from "react"

import { useWorkspaceActions } from "@/contexts/workspace-context"
import {
  browserCapabilities,
  browserClose,
  browserListDownloads,
  browserListTabs,
  browserSetHostRules,
} from "@/lib/browser/browser-api"
import {
  getBrowserPrefs,
  subscribeBrowserPrefs,
} from "@/lib/browser/browser-prefs"
import {
  hydrateBrowserDownloads,
  setBrowserDownload,
} from "@/lib/browser/browser-downloads-store"
import {
  browserWorkspaceTabId,
  removeBrowserTabState,
  requestBrowserFind,
  setBrowserTabNotice,
  setBrowserTabState,
} from "@/lib/browser/browser-tab-store"
import {
  BROWSER_CLOSED_EVENT,
  BROWSER_DOWNLOAD_EVENT,
  BROWSER_NAVIGATION_BLOCKED_EVENT,
  BROWSER_OPEN_REQUEST_EVENT,
  BROWSER_POPUP_EVENT,
  BROWSER_SHORTCUT_EVENT,
  BROWSER_STATE_EVENT,
  type BrowserClosedPayload,
  type BrowserDownload,
  type BrowserNavigationBlockedPayload,
  type BrowserOpenRequestPayload,
  type BrowserPopupPayload,
  type BrowserShortcutPayload,
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
 * - `browser://download` → the download bar of the tab that started it
 * - `browser://shortcut` → a browser shortcut the page had focus for (⌘F)
 * - `browser://navigation-blocked` → a notice on the tab whose navigation
 *   policy refused
 *
 * It also carries the user's site rules the other way: the backend enforces
 * `block` on every navigation a tab attempts, and learns the table from here
 * at startup and whenever the preference changes (the settings window writes
 * it; the storage event brings it over).
 *
 * Only subscribes where a built-in browser exists; in web mode there is
 * nothing to hear.
 */
/** Delay before the one retry of a failed site-rule push. */
const HOST_RULES_RETRY_MS = 1000

export function BrowserEventsBridge() {
  const { adoptBrowserTab, closeFileTab, openBrowserTab } =
    useWorkspaceActions()

  useEffect(() => {
    let cancelled = false
    const unsubscribers: Array<() => void> = []

    // The user's site rules go to the backend, which enforces `block` on
    // every navigation. Subscribed BEFORE the first await: a change written
    // by the settings window while the capabilities round trip is in flight
    // must reach this document's cache (the subscription is what installs
    // the cross-window listener) and then the backend. The first push
    // happens once capabilities say a browser exists, and carries whatever
    // is current then. A push that fails is retried once — the command has
    // no reason to fail except the app shutting down, and a silent
    // divergence would be an unenforced rule.
    let ready = false
    const push = (retry: boolean) => {
      // Before the backend is known to exist a change only invalidates the
      // cache (the subscription did that); the first push below picks up
      // whatever is current by then.
      if (!ready) return
      void browserSetHostRules(getBrowserPrefs().hostRules).catch(() => {
        if (cancelled) return
        if (retry) {
          window.setTimeout(() => {
            if (!cancelled) push(false)
          }, HOST_RULES_RETRY_MS)
        } else {
          console.warn("[browser] site rules could not be sent to the backend")
        }
      })
    }
    unsubscribers.push(subscribeBrowserPrefs(() => push(true)))

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
      // Downloads outlive the tabs that started them (and this document): a
      // reload must not lose the record of a file that is still arriving.
      try {
        hydrateBrowserDownloads(await browserListDownloads())
      } catch {
        /* no downloads to show */
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
        transport.subscribe<BrowserShortcutPayload>(
          BROWSER_SHORTCUT_EVENT,
          (payload) => {
            if (payload.shortcut === "find") {
              requestBrowserFind(browserWorkspaceTabId(payload.tabId))
            }
          }
        ),
        transport.subscribe<BrowserDownload>(
          BROWSER_DOWNLOAD_EVENT,
          (download) => {
            setBrowserDownload(download)
          }
        ),
        transport.subscribe<BrowserNavigationBlockedPayload>(
          BROWSER_NAVIGATION_BLOCKED_EVENT,
          (blocked) => {
            setBrowserTabNotice(browserWorkspaceTabId(blocked.tabId), {
              kind: "navigation-blocked",
              url: blocked.url,
              reason: blocked.reason,
            })
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
      ready = true
      // The first push, whether or not a change arrived meanwhile: the
      // backend starts with an empty table.
      push(true)
    })()

    return () => {
      cancelled = true
      for (const unsubscribe of unsubscribers) unsubscribe()
    }
  }, [adoptBrowserTab, closeFileTab, openBrowserTab])

  return null
}
