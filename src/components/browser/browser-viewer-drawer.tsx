"use client"

import { useEffect, useSyncExternalStore } from "react"
import { useTranslations } from "next-intl"
import { PanelRightOpen } from "lucide-react"

import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerTitle,
  SIDE_PANEL_CONTENT_CLASS,
} from "@/components/ui/drawer"
import { useOptionalWorkbenchRoute } from "@/contexts/workbench-route-context"
import {
  useWorkspaceActions,
  useWorkspaceFileTabs,
} from "@/contexts/workspace-context"
import { normalizeUrlForDedupe } from "@/lib/browser/browser-url"
import {
  DEFAULT_BROWSER_PROFILE_ID,
  browserProfileExists,
  useBrowserPrefs,
} from "@/lib/browser/browser-prefs"

import { BrowserTabView } from "./browser-tab-view"

/**
 * The built-in browser inside the transcript's side panel — where an http(s)
 * link lands when a full-page route (task board, canvas, forge) covers the
 * file column. Underneath it is the same workspace browser tab: the drawer
 * opens (or re-uses) the tab record without activating the file column, shows
 * its view here, and "open in workspace" leads back to the column.
 */

// Which workspace record each drawer URL was opened as (see
// `BrowserViewerBody`). Module-level: the drawer is one component, and the
// value has to be written from an effect and read while rendering.
const openedTabs = new Map<string, string | null>()
const openedListeners = new Set<() => void>()

function subscribeOpened(listener: () => void): () => void {
  openedListeners.add(listener)
  return () => {
    openedListeners.delete(listener)
  }
}

function rememberOpened(url: string, id: string | null): void {
  if (openedTabs.get(url) === id) return
  openedTabs.set(url, id)
  for (const listener of [...openedListeners]) listener()
}

export function BrowserViewerDrawer({
  url,
  open,
  onOpenChange,
}: {
  url: string
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const t = useTranslations("Browser.drawer")
  return (
    <Drawer open={open} onOpenChange={onOpenChange} swipeDirection="right">
      <DrawerContent
        closeButtonClassName="top-2.5 right-3"
        className={SIDE_PANEL_CONTENT_CLASS}
        nativeSurfaceHost
      >
        <DrawerTitle className="sr-only">{t("title")}</DrawerTitle>
        <DrawerDescription className="sr-only">
          {t("description")}
        </DrawerDescription>
        {open ? (
          <BrowserViewerBody url={url} onOpenChange={onOpenChange} />
        ) : null}
      </DrawerContent>
    </Drawer>
  )
}

function BrowserViewerBody({
  url,
  onOpenChange,
}: {
  url: string
  onOpenChange: (open: boolean) => void
}) {
  const t = useTranslations("Browser.drawer")
  const { openBrowserTab, switchFileTab } = useWorkspaceActions()
  const { fileTabs } = useWorkspaceFileTabs()
  const route = useOptionalWorkbenchRoute()

  // Open (or re-use) the workspace tab without activating the file column,
  // and remember WHICH record that was: the same page can be open in two
  // profiles, and the one to show is the one this drawer asked for, whatever
  // the preference says later. The id lives in a small store outside React
  // (an effect may not set state, and a ref may not be read while
  // rendering); it is keyed by URL so a drawer reused for another address
  // starts over.
  useEffect(() => {
    rememberOpened(url, openBrowserTab(url, { activate: false }))
  }, [openBrowserTab, url])
  const openedId = useSyncExternalStore(
    subscribeOpened,
    () => openedTabs.get(url) ?? null,
    () => null
  )

  // Before the effect has run (first paint) the record is found the way
  // `openBrowserTab` itself resolves an address with no opener: by URL in
  // the profile new tabs use. Never a tab of another profile.
  const prefs = useBrowserPrefs()
  const wantedProfile = browserProfileExists(prefs, prefs.newTabProfile)
    ? prefs.newTabProfile
    : DEFAULT_BROWSER_PROFILE_ID
  const wanted = normalizeUrlForDedupe(url)
  const tab =
    (openedId ? fileTabs.find((it) => it.id === openedId) : undefined) ??
    fileTabs.find(
      (it) =>
        it.kind === "browser" &&
        it.browser.profile === wantedProfile &&
        normalizeUrlForDedupe(it.browser.initialUrl) === wanted
    )
  const tabId = tab?.id ?? null

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-border/60 px-3 pr-12 text-xs text-muted-foreground">
        <span className="min-w-0 flex-1 truncate">{url}</span>
        {route && tabId ? (
          <button
            type="button"
            className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-border px-2 text-xs text-foreground hover:bg-primary/8"
            onClick={() => {
              route.openConversations()
              switchFileTab(tabId)
              onOpenChange(false)
            }}
          >
            <PanelRightOpen className="h-3.5 w-3.5" />
            {t("openInWorkspace")}
          </button>
        ) : null}
      </div>
      <div className="min-h-0 flex-1">
        {tab?.kind === "browser" ? (
          <BrowserTabView key={tab.id} tab={tab} />
        ) : (
          <div className="flex h-full items-center justify-center px-6 text-center text-sm text-muted-foreground">
            {t("cannotOpen")}
          </div>
        )}
      </div>
    </div>
  )
}
